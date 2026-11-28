//! T18 站点插件 —— 端到端验收测试（mock 上游 + 真保险库 + 真库文件）。
//!
//! 验收判据（03 §2 T18）：「ModelScope 与 MiniMax Code 两个站点插件；
//! 验收：**两个站点都能签到并把积分回写到账号**」。
//!
//! 覆盖：
//! - **内置插件分派**：`plugin = modelscope` 走 `checkin::sites::modelscope`，
//!   不走 HAR 重放；未知插件被判 `template_invalid`；
//! - **断言走 JSON 业务字段**：登录态无效（401）判 `credential_invalid`，
//!   **不是**「成功」也不是「HTTP 错误」；
//! - **渐进增强**：没有开放平台 token / 查不到魔粒 → **签到仍然成功**，只是没有积分；
//! - **积分三处落点**：`checkin_result.reward_json` / `account.quota_json` / 返回值；
//! - **密文边界**：登录态明文只在保险库里，站点行与库里都不出现明文。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use llm_router_lib::checkin::plugin::PluginSiteConfig;
use llm_router_lib::checkin::replay::ReplayClass;
use llm_router_lib::checkin::replay::ReplayEngine;
use llm_router_lib::checkin::scheduler;
use llm_router_lib::commands::checkin::{
    checkin_add_builtin_site_of, checkin_set_credential_of,
};
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::store::checkin;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;

// ── mock 上游 ──────────────────────────────────────────────────────────

/// mock 的行为开关。
#[derive(Clone, Copy, PartialEq, Default)]
enum MockMode {
    /// 签到成功 + 魔粒可查（**签到后才出现当日 `daily_active` 流水**）。
    #[default]
    Full,
    /// 签到成功，但魔粒接口 401（**没有开放平台 token 的情形**）。
    NoBalance,
    /// 登录态无效（匿名态 401）。
    Unauthorized,
    /// **今天已领过**：当日 `daily_active` 流水**签到前就已经在**（差值 = 0）。
    AlreadyClaimed,
}

#[derive(Default)]
struct MockState {
    hits: AtomicUsize,
    signin_hits: AtomicUsize,
    /// 魔粒流水接口被打了多少次（**签到前基线 + 签到后各一次**）。
    transactions_hits: AtomicUsize,
    /// 收到的 Cookie 头（**明文**——测试里用来断言真的把登录态发出去了）。
    seen_cookie: RwLock<Option<String>>,
    mode: RwLock<MockMode>,
}

impl MockState {
    fn set_mode(&self, mode: MockMode) {
        *self.mode.write().expect("写模式失败") = mode;
    }
    fn mode(&self) -> MockMode {
        *self.mode.read().expect("读模式失败")
    }
}

async fn signin_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    state.signin_hits.fetch_add(1, Ordering::SeqCst);
    *state.seen_cookie.write().expect("写 Cookie 失败") = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    match state.mode() {
        MockMode::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            r#"{"Code":10019901001,"Message":"禁止访问","Success":false}"#,
        )
            .into_response(),
        _ => (
            StatusCode::OK,
            r#"{"Code":200,"Data":{"UserName":"demo"},"Message":"success","Success":true}"#,
        )
            .into_response(),
    }
}

/// 魔粒流水接口（**真机实测形状**：2026-11-27 登录态实测）。
///
/// - 金额字段 = `total_amount`；日期字段 = `gmt_created`；类型 = `type`；规则 = `rule_key`；
/// - **当日 `daily_active` 那条 = 200 魔粒**；资料类一次性奖励（`infocomplement_*`）不算签到奖励。
async fn transactions_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    state.transactions_hits.fetch_add(1, Ordering::SeqCst);
    match state.mode() {
        // 签到成功但**查不到魔粒**（没有开放平台 token 的情形）与登录态失效都回 401
        MockMode::NoBalance | MockMode::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            r#"{"success":false,"code":"InvalidAuthentication","message":"Invalid authentication"}"#,
        )
            .into_response(),
        // **签到前**还没有当日流水（签到才触发到账）→ 基线 = 0
        MockMode::Full if state.signin_hits.load(Ordering::SeqCst) == 0 => (
            StatusCode::OK,
            r#"{"data":{"page":1,"page_size":20,"total":0,"records":[]},"request_id":"r0","success":true}"#,
        )
            .into_response(),
        // 签到后 → 当日 `daily_active` 200 魔粒（+ 两条一次性资料奖励，**不算进签到奖励**）
        //
        // **日期必须用「今天」**（`modelscope::today()`，与插件算差值时同一个函数）：
        // 夹具若写死某一天，**跨天当天这个用例必挂**——当日流水匹配不上「今天」→
        // 差值 = 0 → 判「今天已领过」→ 拿不到积分（2026-11-28 真踩到过一次）。
        MockMode::Full | MockMode::AlreadyClaimed => {
            let today = llm_router_lib::checkin::sites::modelscope::today();
            (
                StatusCode::OK,
                format!(
                    r#"{{"data":{{"page":1,"page_size":20,"total":3,"records":[
                {{"type":"EARN","rule_key":"daily_active","total_amount":200,"count":1,"expire_days":1,"gmt_created":"{today}"}},
                {{"type":"EARN","rule_key":"infocomplement_profile_experience","total_amount":50,"count":1,"gmt_created":"{today}"}},
                {{"type":"EARN","rule_key":"infocomplement_email_verification","total_amount":50,"count":1,"gmt_created":"{today}"}}
            ]}},"request_id":"r1","success":true}}"#
                ),
            )
                .into_response()
        }
    }
}

async fn start_mock() -> (String, Arc<MockState>) {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/api/v1/users/login/info", get(signin_handler))
        .route(
            "/openapi/v1/magicubes/transactions",
            get(transactions_handler),
        )
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
    // **测试环境关掉通知**（T19）：通知默认开 → 每条用例都会弹一次系统通知，
    // 变成噪音。通知本身的用例在 `tests/checkin_notify_e2e.rs` 里显式打开。
    let _ = llm_router_lib::store::settings::set_setting(&store, "notify_enabled", "0");
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

/// 建一个 ModelScope 内置站点，填登录态，指向 mock。
fn modelscope_site(state: &AppState, mock_url: &str) -> String {
    let summary = checkin_add_builtin_site_of(state, "modelscope").expect("建站失败");
    assert_eq!(summary.plugin, "modelscope");
    assert!(!summary.enabled, "内置站点默认不启用（02 §7.3）");

    // 填登录态（明文进保险库）
    checkin_set_credential_of(state, &summary.id, "cookie", "csrf_session=SECRET-COOKIE")
        .expect("填登录态失败");

    // 把站点指向 mock（真实站点是 www.modelscope.cn）
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let mut config = PluginSiteConfig::default();
    if let Some(json) = row.config_json.as_deref() {
        config = serde_json::from_str(json).expect("配置应能解析");
    }
    config.base_url = Some(mock_url.to_string());
    let mut updated = row.clone();
    updated.config_json = Some(config.to_json());
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &updated))
        .expect("写站点失败");
    summary.id
}

/// 跑一次站点。
async fn run(state: &AppState, site_id: &str) -> llm_router_lib::checkin::replay::ReplayOutcome {
    let engine = ReplayEngine::new().expect("构造引擎");
    scheduler::run_now(&state.store, &state.vault, &engine, site_id)
        .await
        .expect("跑站点失败")
}

// ── 用例 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn modelscope_plugin_signs_in_and_writes_the_reward_everywhere() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);

    let outcome = run(&state, &site_id).await;
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(outcome.class, ReplayClass::Success);

    // 登录态真的发出去了
    assert_eq!(
        mock.seen_cookie.read().expect("读 Cookie 失败").as_deref(),
        Some("csrf_session=SECRET-COOKIE"),
        "登录态应作为 Cookie 头发给上游"
    );

    // 积分：返回值 + reward_json + account.quota_json
    // （**实测口径**：当日每日签到那一笔 `daily_active` = 200 魔粒；资料类一次性奖励不算）
    let reward = outcome.reward.as_ref().expect("应拿到积分");
    assert_eq!(reward.amount, 200);
    assert_eq!(reward.unit, "magicube");
    assert!(
        outcome.message.contains("本次到账 +200"),
        "回执应说明本次到账多少：{}",
        outcome.message
    );

    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(&site_id), 10))
        .expect("列结果失败");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ok);
    let reward_json = rows[0].reward_json.as_deref().expect("reward_json 不该为空");
    assert!(reward_json.contains("\"amount\":200"), "实际 {reward_json}");
    assert!(reward_json.contains("\"unit\":\"magicube\""), "实际 {reward_json}");

    // 账号额度快照
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &site_id))
        .expect("读站点")
        .expect("站点应存在");
    let config: PluginSiteConfig =
        serde_json::from_str(row.config_json.as_deref().expect("配置")).expect("配置应能解析");
    let account_id = config.account_id.expect("站点应有归属账号");
    let snapshot = llm_router_lib::store::account::read_reward(&state.store, &account_id)
        .expect("应有积分快照");
    assert_eq!(snapshot.points, 200);
    assert_eq!(snapshot.unit, "magicube");
    assert_eq!(snapshot.source, "checkin");
}

#[tokio::test]
async fn a_second_run_on_the_same_day_does_not_count_the_reward_twice() {
    // **幂等口径**（02 §7.4 §2）：当日那条 `daily_active` 流水**整天都在** →
    // 第二次运行签到前后的差值 = 0 → 判**「今天已领过」**：`points = null`、**不回写额度池**。
    let (mock_url, mock) = start_mock().await;
    mock.set_mode(MockMode::AlreadyClaimed);
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);

    let outcome = run(&state, &site_id).await;
    assert!(outcome.ok, "{}", outcome.message);
    assert!(
        outcome.message.contains("今天已领过"),
        "回执应说明今天已领过：{}",
        outcome.message
    );
    assert!(outcome.reward.is_none(), "已领过 → 本次没有新增积分");

    // 额度池**不被写**（避免同一天重复计数）
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &site_id))
        .expect("读站点")
        .expect("站点应存在");
    let config: PluginSiteConfig =
        serde_json::from_str(row.config_json.as_deref().expect("配置")).expect("配置应能解析");
    let account_id = config.account_id.expect("站点应有归属账号");
    assert!(
        llm_router_lib::store::account::read_reward(&state.store, &account_id).is_none(),
        "已领过时不该写额度池"
    );
}

#[tokio::test]
async fn modelscope_plugin_reports_an_invalid_credential() {
    // 匿名态实测：HTTP 401 +「禁止访问」→ 必须判凭据失效，**不是成功**
    let (mock_url, mock) = start_mock().await;
    mock.set_mode(MockMode::Unauthorized);
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);

    let outcome = run(&state, &site_id).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::CredentialInvalid);
    assert!(outcome.reward.is_none(), "失败不该有积分");
    // 签到**前**查过一次基线，签到失败后**不再查第二次**
    assert_eq!(
        mock.transactions_hits.load(Ordering::SeqCst),
        1,
        "签到失败后不该再查第二次魔粒"
    );
}

#[tokio::test]
async fn signin_still_succeeds_when_the_transactions_cannot_be_read() {
    // **渐进增强**：没有开放平台 token → 魔粒查不到 → **签到仍然成功**，只是没积分
    let (mock_url, mock) = start_mock().await;
    mock.set_mode(MockMode::NoBalance);
    let (_dir, state) = app_state();
    let site_id = modelscope_site(&state, &mock_url);

    let outcome = run(&state, &site_id).await;
    assert!(outcome.ok, "{}", outcome.message);
    assert!(outcome.reward.is_none(), "查不到魔粒时不应有积分");
    // 签到前基线 + 签到后各查一次
    assert_eq!(
        mock.transactions_hits.load(Ordering::SeqCst),
        2,
        "还是应该尝试查魔粒（签到前基线 + 签到后）"
    );
}

#[tokio::test]
async fn a_missing_credential_is_reported_before_any_request() {
    // 没填登录态 → **不发请求**，直接判凭据缺失
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let summary = checkin_add_builtin_site_of(&state, "modelscope").expect("建站失败");
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let mut config = PluginSiteConfig::default();
    if let Some(json) = row.config_json.as_deref() {
        config = serde_json::from_str(json).expect("配置应能解析");
    }
    config.base_url = Some(mock_url);
    let mut updated = row.clone();
    updated.config_json = Some(config.to_json());
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &updated))
        .expect("写站点失败");

    let outcome = run(&state, &summary.id).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::CredentialMissing);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0, "没凭据就不该打上游");
}

#[tokio::test]
async fn an_unknown_plugin_is_reported_instead_of_being_skipped() {
    let (_dir, state) = app_state();
    let row = checkin::SiteRow {
        id: checkin::new_site_id(),
        name: "野插件".to_string(),
        plugin: "not-a-plugin".to_string(),
        config_json: Some("{}".to_string()),
        schedule: Some("0 9 * * *".to_string()),
        enabled: true,
    };
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &row))
        .expect("写站点失败");

    let outcome = run(&state, &row.id).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::TemplateInvalid);
    assert!(outcome.message.contains("未知的站点插件"), "{}", outcome.message);
}

#[tokio::test]
async fn the_minimax_code_plugin_fails_cleanly_without_the_sidecar() {
    // 辅轨不可用 → 明确失败（**不假装跑过了**）
    let (_dir, state) = app_state();
    let summary = checkin_add_builtin_site_of(&state, "minimax_code").expect("建站失败");
    assert!(summary.needs_sidecar, "MiniMax Code 插件需要辅轨");

    let outcome = run(&state, &summary.id).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::TemplateInvalid);
    assert!(outcome.message.contains("辅轨"), "{}", outcome.message);
}

#[test]
fn builtin_site_creation_wires_channel_account_and_default_schedule() {
    let (_dir, state) = app_state();
    let summary = checkin_add_builtin_site_of(&state, "modelscope").expect("建站失败");

    assert_eq!(summary.plugin, "modelscope");
    assert_eq!(summary.schedule.as_deref(), Some("0 9 * * *"));
    assert!(!summary.enabled);
    assert!(!summary.credential_ready, "还没填登录态");
    assert!(summary.credential_hint.is_some());
    assert!(summary.reward.is_none());

    // **回归**：MiniMax Code 的默认 cron 必须是插件自己的常量（真机踩过：
    // 清单写 `20 8 * * *`，插件却回落到 trait 默认的 `0 9 * * *`）
    let minimax = checkin_add_builtin_site_of(&state, "minimax_code").expect("建站失败");
    assert_eq!(minimax.schedule.as_deref(), Some("20 8 * * *"));
    assert!(minimax.needs_sidecar);

    // 归属渠道 + 归属账号都要在
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let config: PluginSiteConfig =
        serde_json::from_str(row.config_json.as_deref().expect("配置")).expect("配置应能解析");
    let channel_id = config.channel_id.expect("应有归属渠道");
    let account_id = config.account_id.expect("应有归属账号");

    let channel_name: String = state
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT name FROM channel WHERE id = ?1")?;
            let mut rows = stmt.query([&channel_id])?;
            let row = rows.next()?.expect("渠道应存在");
            Ok(row.get(0)?)
        })
        .expect("读渠道失败");
    assert!(channel_name.contains("ModelScope"), "实际 {channel_name}");

    let account_exists: i64 = state
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT COUNT(*) FROM account WHERE id = ?1")?;
            let mut rows = stmt.query([&account_id])?;
            let row = rows.next()?.expect("应有一行");
            Ok(row.get(0)?)
        })
        .expect("读账号失败");
    assert_eq!(account_exists, 1, "归属账号应存在");
}

#[test]
fn the_login_state_never_lands_in_the_site_row() {
    // 密文边界：站点行里只有凭据 id，**没有明文**
    let (_dir, state) = app_state();
    let summary = checkin_add_builtin_site_of(&state, "modelscope").expect("建站失败");
    let secret = "csrf_session=SECRET-COOKIE-VALUE";
    let updated =
        checkin_set_credential_of(&state, &summary.id, "cookie", secret).expect("填登录态失败");
    assert!(updated.credential_ready);

    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let json = row.config_json.as_deref().expect("配置");
    assert!(
        !json.contains("SECRET-COOKIE-VALUE"),
        "站点行里不该出现登录态明文：{json}"
    );

    // 真值在保险库里
    let config: PluginSiteConfig = serde_json::from_str(json).expect("配置应能解析");
    let credential_id = config.credential_id.expect("应有凭据 id");
    let vault = state.vault.read().expect("读保险库");
    let stored = vault
        .get_credential(&credential_id)
        .expect("保险库里应有这条凭据");
    assert_eq!(String::from_utf8_lossy(stored.as_bytes()), secret);
}

#[test]
fn adding_the_same_builtin_site_twice_reuses_the_same_row() {
    let (_dir, state) = app_state();
    let first = checkin_add_builtin_site_of(&state, "modelscope").expect("建站失败");
    let second = checkin_add_builtin_site_of(&state, "modelscope").expect("建站失败");
    assert_eq!(first.id, second.id, "同名站点应复用同一行");

    let rows = state
        .store
        .with_conn(|conn| checkin::list_sites(conn))
        .expect("列站点失败");
    assert_eq!(rows.len(), 1);
}
