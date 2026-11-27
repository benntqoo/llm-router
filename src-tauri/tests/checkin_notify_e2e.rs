//! T19 通知 —— 端到端验收测试（真保险库 + 真库文件 + 本地 mock 接收器）。
//!
//! 覆盖：
//! - **成功通知含站点名与本次到账**、**失败通知含站点名与原因**（M3 验收判据）；
//! - **webhook 地址零明文**：设置表里只有凭据 id，地址在保险库（密文）；
//! - **通知失败不影响签到结果**（结果照常落库）；
//! - **应用关闭时那条路径也能通知**——CLI / 计划任务走的也是 `scheduler::run_site`
//!   （本文件直接调 `run_now` = 与 UI「立即执行」同一条路径）。
//!
//! **不出网**：webhook 指向本地 axum mock 接收器。

use std::sync::{Arc, RwLock};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use llm_router_lib::checkin::notify::{self, NotifyConfigInput};
use llm_router_lib::checkin::plugin::PluginSiteConfig;
use llm_router_lib::checkin::replay::ReplayEngine;
use llm_router_lib::checkin::scheduler;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::store::checkin;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;
use serde_json::Value;

// ── mock 上游（签到接口）+ mock 接收器（webhook） ──────────────────────────

/// mock 收到的 webhook 请求体 + 上游行为开关。
#[derive(Default)]
struct Receiver {
    bodies: std::sync::Mutex<Vec<Value>>,
    /// 上游签到接口回 401（模拟登录态失效）。
    unauthorized: bool,
}

async fn webhook_sink(
    axum::extract::State(state): axum::extract::State<Arc<Receiver>>,
    body: String,
) -> impl IntoResponse {
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    state.bodies.lock().expect("锁 bodies 失败").push(parsed);
    (StatusCode::OK, r#"{"errcode":0,"errmsg":"ok"}"#)
}

async fn signin_handler(
    axum::extract::State(state): axum::extract::State<Arc<Receiver>>,
) -> impl IntoResponse {
    if state.unauthorized {
        (
            StatusCode::UNAUTHORIZED,
            r#"{"Code":10019901001,"Message":"禁止访问","Success":false}"#,
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            r#"{"Code":200,"Data":{"UserName":"demo"},"Message":"success","Success":true}"#,
        )
            .into_response()
    }
}

/// 起两个本地服务：签到上游 + webhook 接收器。
async fn start_mocks(signin_unauthorized: bool) -> (String, Arc<Receiver>) {
    let receiver = Arc::new(Receiver {
        bodies: std::sync::Mutex::new(Vec::new()),
        unauthorized: signin_unauthorized,
    });
    let app = Router::new()
        .route("/api/v1/users/login/info", get(signin_handler))
        .route("/hook", post(webhook_sink))
        .with_state(Arc::clone(&receiver));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定 mock 端口");
    let addr = listener.local_addr().expect("读取 mock 地址");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), receiver)
}

// ── 夹具 ──────────────────────────────────────────────────────────────────

fn app_state() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault = Arc::new(RwLock::new(
        Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
    ));
    let circuit = Arc::new(CircuitBreaker::with_persistence(Arc::new(
        StoreCircuitPersistence::new(Arc::clone(&store)),
    )));
    let state = AppState {
        gateway_addr: "127.0.0.1:8787".to_string(),
        gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
        store,
        vault,
        limiter: Arc::new(RateLimiter::new()),
        circuit,
    };
    (dir, state)
}

/// 建一个指向 mock 上游的 ModelScope 站点，登录态填一个假 Cookie。
fn modelscope_site(state: &AppState, mock_url: &str) -> String {
    let summary = llm_router_lib::commands::checkin::checkin_add_builtin_site_of(state, "modelscope")
        .expect("建站失败");
    llm_router_lib::commands::checkin::checkin_set_credential_of(
        state,
        &summary.id,
        "cookie",
        "csrf_session=SECRET-COOKIE",
    )
    .expect("填登录态失败");
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let mut config: PluginSiteConfig = serde_json::from_str(
        row.config_json.as_deref().expect("配置"),
    )
    .expect("配置应能解析");
    config.base_url = Some(mock_url.to_string());
    let mut updated = row.clone();
    updated.config_json = Some(config.to_json());
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &updated))
        .expect("写站点失败");
    summary.id
}

/// 打开通知并把 webhook 指向 mock 接收器。
fn enable_notifications(state: &AppState, hook_url: &str) {
    let input = NotifyConfigInput {
        enabled: true,
        // **测试环境关掉系统通知**（弹窗会打扰桌面）；系统通知那条路在真机验。
        system_enabled: false,
        notify_success: true,
        webhook_kind: Some("wecom".to_string()),
        webhook_url: Some(hook_url.to_string()),
        telegram_chat_id: None,
    };
    let saved = notify::save_settings(&state.store, &state.vault, &input).expect("保存通知配置失败");
    assert!(saved.enabled);
    assert!(saved.webhook_configured);
}

fn received_texts(receiver: &Receiver) -> Vec<String> {
    receiver
        .bodies
        .lock()
        .expect("锁 bodies 失败")
        .iter()
        .map(|body| {
            body.get("text")
                .and_then(|text| text.get("content"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

// ── 用例 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_successful_checkin_sends_a_notification_with_the_site_name() {
    let (mock_url, receiver) = start_mocks(false).await;
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);
    let hook_url = format!("{mock_url}/hook");
    enable_notifications(&state, &hook_url);

    let engine = ReplayEngine::new().expect("构造引擎");
    let outcome = scheduler::run_now(&state.store, &state.vault, &engine, &site_id)
        .await
        .expect("跑站点失败");
    assert!(outcome.ok, "{}", outcome.message);

    // 接收器收到了通知
    let texts = received_texts(&receiver);
    assert_eq!(texts.len(), 1, "应当收到一条通知：{texts:?}");
    let text = &texts[0];
    // **必含站点名**
    assert!(text.contains("ModelScope"), "通知里要有站点名：{text}");
    // **必含分类与结果**
    assert!(text.contains("success"), "通知里要有分类：{text}");
    assert!(text.contains("签到成功"), "通知里要有结果：{text}");
}

#[tokio::test]
async fn a_failed_checkin_sends_a_notification_with_the_reason() {
    let (mock_url, receiver) = start_mocks(true).await;
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);
    let hook_url = format!("{mock_url}/hook");
    enable_notifications(&state, &hook_url);

    let engine = ReplayEngine::new().expect("构造引擎");
    let outcome = scheduler::run_now(&state.store, &state.vault, &engine, &site_id)
        .await
        .expect("跑站点失败");
    assert!(!outcome.ok);
    assert_eq!(outcome.class, llm_router_lib::checkin::replay::ReplayClass::CredentialInvalid);

    let texts = received_texts(&receiver);
    assert_eq!(texts.len(), 1, "失败也必须通知：{texts:?}");
    let text = &texts[0];
    // **必含站点名与原因**（M3 验收判据）
    assert!(text.contains("ModelScope"), "通知里要有站点名：{text}");
    assert!(text.contains("签到失败"), "通知里要标明失败：{text}");
    assert!(
        text.contains("登录态无效") || text.contains("禁止访问"),
        "通知里要有原因：{text}"
    );
}

#[tokio::test]
async fn the_webhook_address_is_never_stored_in_plaintext() {
    let (mock_url, _receiver) = start_mocks(false).await;
    let (_dir, state) = app_state();
    let hook_url = format!("{mock_url}/hook?key=SECRET-WEBHOOK-KEY");
    enable_notifications(&state, &hook_url);

    // 设置表里只有凭据 id
    let settings_dump: String = state
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT key, value FROM app_setting")?;
            let mut rows = stmt.query([])?;
            let mut out = String::new();
            while let Some(row) = rows.next()? {
                out.push_str(&format!(
                    "{}={}\n",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?
                ));
            }
            Ok(out)
        })
        .expect("读设置失败");
    assert!(
        !settings_dump.contains("SECRET-WEBHOOK-KEY"),
        "设置表里不该有地址明文：{settings_dump}"
    );

    // 凭据表里只有密文
    let credential_dump: String = state
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, kind, ciphertext FROM credential")?;
            let mut rows = stmt.query([])?;
            let mut out = String::new();
            while let Some(row) = rows.next()? {
                out.push_str(&format!(
                    "{} {} {}\n",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
                ));
            }
            Ok(out)
        })
        .expect("读凭据失败");
    assert!(
        !credential_dump.contains("SECRET-WEBHOOK-KEY"),
        "凭据表里不该有地址明文"
    );
    // 但**确实存了**一条 webhook 凭据
    assert!(credential_dump.contains("webhook"), "{credential_dump}");
}

#[tokio::test]
async fn a_broken_webhook_does_not_break_the_checkin_result() {
    let (mock_url, _receiver) = start_mocks(false).await;
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);
    // 指向一个**没人监听的端口** → webhook 必失败
    enable_notifications(&state, "http://127.0.0.1:9/hook");

    let engine = ReplayEngine::new().expect("构造引擎");
    let outcome = scheduler::run_now(&state.store, &state.vault, &engine, &site_id)
        .await
        .expect("跑站点失败");
    // 签到照常成功（通知失败**不影响签到结果**）
    assert!(outcome.ok, "{}", outcome.message);

    // 结果照常落库
    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(&site_id), 10))
        .expect("列结果失败");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ok);
}

#[tokio::test]
async fn turning_notifications_off_sends_nothing() {
    let (mock_url, receiver) = start_mocks(false).await;
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);
    let hook_url = format!("{mock_url}/hook");
    enable_notifications(&state, &hook_url);
    // 关掉总开关
    let input = NotifyConfigInput {
        enabled: false,
        system_enabled: false,
        notify_success: true,
        webhook_kind: Some("wecom".to_string()),
        webhook_url: None,
        telegram_chat_id: None,
    };
    notify::save_settings(&state.store, &state.vault, &input).expect("保存通知配置失败");

    let engine = ReplayEngine::new().expect("构造引擎");
    let outcome = scheduler::run_now(&state.store, &state.vault, &engine, &site_id)
        .await
        .expect("跑站点失败");
    assert!(outcome.ok);
    assert!(received_texts(&receiver).is_empty(), "关了就不该发");
}
