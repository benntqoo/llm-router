//! T16 重放引擎 —— 端到端验收测试（mock 上游）。
//!
//! 验收判据（03 §2 T16）：「变量注入、请求重放、响应断言（状态码 + JSON 路径）、错误分类。
//! 验收：**单测 3 场景（成功/断言失败/网络失败）**」。
//!
//! 本文件把 **T15 的真实产物**接到 **T16 的引擎**上：
//! 真实抓包夹具 → T15 保存模板（真实值进保险库）→ T16 取回模板 → 注入 → 重放 → 断言 → 落结果。
//!
//! 覆盖场景：
//! - **成功**：状态码 + JSON 路径断言全过 → `success`，`checkin_result.ok = 1`；
//! - **断言失败**：JSON 路径不符 → `assert_failed`；上游 401 → `credential_invalid`；
//! - **网络失败**：连不上 → `network_error`（可重试）；
//! - **缺凭据**：凭据被删 → `credential_missing`，且**一个请求都不该发出去**。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;
use llm_router_lib::checkin::har::{JsonPathAssert, MarkedVariable, SiteTemplate, VarTarget};
use llm_router_lib::checkin::replay::{
    ReplayClass, ReplayEngine, ReplayOutcome, ResolveError, SecretResolver,
};
use llm_router_lib::commands::checkin::{checkin_save_template_of, SaveTemplateInput};
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::store::checkin;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;

/// 真实抓包脱敏后的夹具（T15 产物）。
const FIXTURE: &str = "tests/fixtures/modelscope-checkin.har";

/// 夹具里被标记变量的原始取值（脱敏后的假值）。
const ACW_VALUE: &str = "REDACTED-acw_tc";
const CNA_VALUE: &str = "REDACTED-cna";

// ── mock 上游 ──────────────────────────────────────────────────────────

#[derive(Default)]
struct MockState {
    hits: AtomicUsize,
    /// 收到的 Cookie 头（原样）。
    cookie: RwLock<Option<String>>,
}

async fn mock_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
    request: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    let cookie = request
        .headers()
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    *state.cookie.write().expect("锁") = cookie;
    // 路径决定上游行为：/fail401 → 401；其余 → 200 + data.ok=true
    if request.uri().path().ends_with("/fail401") {
        return (StatusCode::UNAUTHORIZED, "{\"message\":\"unauthorized\"}").into_response();
    }
    if request.uri().path().ends_with("/wrongjson") {
        return (StatusCode::OK, "{\"data\":{\"ok\":false,\"credits\":0}}").into_response();
    }
    (
        StatusCode::OK,
        "{\"data\":{\"ok\":true,\"credits\":400},\"message\":\"签到成功\"}",
    )
        .into_response()
}

async fn start_mock() -> (String, Arc<MockState>) {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .fallback(any(mock_handler))
        .with_state(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定 mock 端口");
    let addr = listener.local_addr().expect("读取 mock 地址");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), state)
}

// ── 夹具 ──────────────────────────────────────────────────────────────

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

/// 从保险库取凭据的解析器（T16 的 `SecretResolver` 接到真实保险库上）。
struct VaultResolver {
    vault: Arc<RwLock<Vault>>,
}

impl SecretResolver for VaultResolver {
    fn resolve(&self, credential_id: &str) -> Result<String, ResolveError> {
        let vault = self
            .vault
            .read()
            .map_err(|err| ResolveError::Unavailable(err.to_string()))?;
        match vault.get_credential(credential_id) {
            Ok(secret) => Ok(String::from_utf8_lossy(secret.as_bytes()).to_string()),
            Err(_) => Err(ResolveError::NotFound),
        }
    }
}

fn cookie_part(name: &str, part: &str) -> MarkedVariable {
    MarkedVariable {
        name: name.to_string(),
        target: VarTarget::HeaderPart {
            field: "Cookie".to_string(),
            part: part.to_string(),
        },
    }
}

/// 用 T15 的保存路径落一个真实模板（真实值进保险库），返回站点 id。
fn save_template(state: &AppState) -> String {
    let input = SaveTemplateInput {
        path: FIXTURE.to_string(),
        entry_index: 0,
        site_name: "ModelScope".to_string(),
        channel_id: None,
        channel_name: Some("ModelScope·签到".to_string()),
        variables: vec![cookie_part("acw_tc", "acw_tc"), cookie_part("cna", "cna")],
    };
    checkin_save_template_of(state, &input)
        .expect("保存模板失败")
        .id
}

/// 取回模板并把 URL 指向 mock（真实站点 URL 留在模板里，重放时改指 mock 上游）。
fn load_template(state: &AppState, site_id: &str, url: &str) -> SiteTemplate {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, site_id))
        .expect("读站点")
        .expect("站点应存在");
    let json = row.config_json.expect("模板 JSON");
    let mut template: SiteTemplate = serde_json::from_str(&json).expect("模板应能解析");
    template.request.url = url.to_string();
    template
}

/// 落一行签到结果（T17 调度层会做同样的事）。
fn record(state: &AppState, site_id: &str, outcome: &ReplayOutcome) {
    state
        .store
        .with_conn(|conn| {
            checkin::insert_result(
                conn,
                site_id,
                1_700_000_000,
                outcome.ok,
                None,
                Some(&outcome.message),
            )
        })
        .expect("写结果失败");
}

fn results(state: &AppState, site_id: &str) -> Vec<checkin::ResultRow> {
    state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(site_id), 10))
        .expect("列结果失败")
}

// ── 用例 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn replay_succeeds_and_records_the_result() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);

    let template = load_template(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/users/login/info"),
    );
    // 期望：200 + JSON 路径 data.ok == true（02 §7.1.3 §3）
    let mut template = template;
    template.assert.status = Some(200);
    template.assert.json_paths = vec![JsonPathAssert {
        path: "data.ok".to_string(),
        equals: Some(serde_json::json!(true)),
    }];

    let resolver = VaultResolver {
        vault: Arc::clone(&state.vault),
    };
    let engine = ReplayEngine::new().expect("构造引擎");
    let outcome = engine.replay(&template, &resolver).await;

    assert!(outcome.ok, "应签到成功：{}", outcome.message);
    assert_eq!(outcome.class, ReplayClass::Success);
    assert_eq!(outcome.status, Some(200));
    assert!(!outcome.retryable);
    assert!(outcome.message.contains("签到成功"));
    assert_eq!(mock.hits.load(Ordering::SeqCst), 1, "上游应收到一次请求");

    // **变量真的被替换了**：上游看到的 Cookie 是保险库里的真实值（不是占位符）
    let cookie = mock.cookie.read().expect("锁").clone().expect("应有 Cookie");
    assert!(cookie.contains(ACW_VALUE), "Cookie 里应是真值：{cookie}");
    assert!(cookie.contains(CNA_VALUE), "Cookie 里应是真值：{cookie}");
    assert!(!cookie.contains("{{"), "占位符不该发出去：{cookie}");

    // 结果落库
    record(&state, &site_id, &outcome);
    let rows = results(&state, &site_id);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ok);
    assert_eq!(rows[0].site_id, site_id);
    assert!(rows[0].message.as_deref().unwrap().contains("签到成功"));
    // 结果里不许出现真实取值
    assert!(!rows[0].message.as_deref().unwrap().contains(ACW_VALUE));
}

#[tokio::test]
async fn assertion_failure_is_classified_and_recorded() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);

    // 场景 A：JSON 路径不符 → assert_failed
    let mut template = load_template(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/wrongjson"),
    );
    template.assert.status = Some(200);
    template.assert.json_paths = vec![JsonPathAssert {
        path: "data.ok".to_string(),
        equals: Some(serde_json::json!(true)),
    }];
    let resolver = VaultResolver {
        vault: Arc::clone(&state.vault),
    };
    let engine = ReplayEngine::new().expect("构造引擎");
    let outcome = engine.replay(&template, &resolver).await;

    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::AssertFailed);
    assert!(outcome.message.contains("data.ok"), "{}", outcome.message);
    assert!(!outcome.retryable, "断言失败不可重试");

    // 场景 B：上游 401 → credential_invalid（比"状态码不符"更可操作）
    let mut template = load_template(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/fail401"),
    );
    template.assert.status = Some(200);
    let outcome = engine.replay(&template, &resolver).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::CredentialInvalid);
    assert!(outcome.message.contains("401"));
    assert!(!outcome.retryable);

    assert_eq!(mock.hits.load(Ordering::SeqCst), 2);

    // 两次失败都要落库，且 ok = 0
    record(&state, &site_id, &outcome);
    let rows = results(&state, &site_id);
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].ok);
    assert!(rows[0].message.as_deref().unwrap().contains("凭证可能已失效"));
}

#[tokio::test]
async fn network_failure_is_classified_as_retryable() {
    let (_mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);

    // 指向一个没人监听的端口（先起再关，拿到一个确定没人要的端口）
    let dead_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定临时端口");
        let addr = listener.local_addr().expect("读取端口");
        drop(listener);
        addr.port()
    };
    let template = load_template(
        &state,
        &site_id,
        &format!("http://127.0.0.1:{dead_port}/api/v1/users/login/info"),
    );

    let resolver = VaultResolver {
        vault: Arc::clone(&state.vault),
    };
    // 把超时压到毫秒级（真实默认是 10s/30s，测试里等太久）
    let engine = ReplayEngine::with_timeouts(Duration::from_millis(500), Duration::from_millis(800))
        .expect("构造引擎");
    let outcome = engine.replay(&template, &resolver).await;

    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::NetworkError);
    assert!(outcome.retryable, "网络失败可重试（02 §3.3）");
    assert!(outcome.message.contains("网络失败"), "{}", outcome.message);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0, "连不上就不该有请求");

    record(&state, &site_id, &outcome);
    let rows = results(&state, &site_id);
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].ok);
}

#[tokio::test]
async fn missing_credential_stops_before_sending_anything() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);

    // 删掉一个凭据（模拟"用户没录"或"凭据被清理了"）
    let template = load_template(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/users/login/info"),
    );
    let credential_id = template.variables[0]
        .credential_id
        .clone()
        .expect("应有凭据 id");
    {
        let vault = state.vault.read().expect("读保险库");
        assert!(vault.delete_credential(&credential_id).expect("删凭据失败"));
    }

    let resolver = VaultResolver {
        vault: Arc::clone(&state.vault),
    };
    let engine = ReplayEngine::new().expect("构造引擎");
    let outcome = engine.replay(&template, &resolver).await;

    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::CredentialMissing);
    assert!(!outcome.retryable);
    assert!(outcome.message.contains("缺少凭据"), "{}", outcome.message);
    assert_eq!(
        mock.hits.load(Ordering::SeqCst),
        0,
        "缺凭据时一个请求都不该发出去"
    );

    record(&state, &site_id, &outcome);
    assert_eq!(results(&state, &site_id).len(), 1);
}
