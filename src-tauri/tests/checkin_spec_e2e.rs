//! T25 声明式站点 + 站点目录 —— 端到端验收测试（mock 上游 + 真保险库 + 真库文件）。
//!
//! 验收判据（03 §2 T25）：
//! ① 一份 spec 指向本地 mock → 建站 → 执行成功并**拿到积分**；
//! ② 断言不成立 → 判 `assert_failed`（**不静默当成功**）；
//! ③ 缺凭据 → 判 `credential_missing` 且**上游请求数不增加**；
//! ④ 目录导入：非法条目**带原因不静默丢弃**、默认 `enabled = 0`；
//! ⑤ 目录导出 → 再导入**站点数一致**（幂等）；导出**不含凭据明文**；
//! ⑥ 余额可查，且与「本次拿到」**严格区分**。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use llm_router_lib::checkin::replay::{ReplayClass, ReplayEngine};
use llm_router_lib::checkin::scheduler;
use llm_router_lib::checkin::spec::{
    SpecAssert, SpecBalance, SpecExtract, SpecHeader, SpecJsonPath, SpecSite,
};
use llm_router_lib::commands::checkin_catalog::{
    checkin_add_spec_site_of, checkin_balance_of, checkin_export_catalog_of,
    checkin_import_catalog_of,
};
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::store::checkin;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;

// ── mock 上游 ──────────────────────────────────────────────────────────

#[derive(Default)]
struct MockState {
    /// 上游被打了多少次（**签到 + 余额**都算）。
    hits: AtomicUsize,
    /// 签到接口被打了多少次。
    checkin_hits: AtomicUsize,
    /// 余额接口被打了多少次。
    balance_hits: AtomicUsize,
    /// 收到的 Authorization 头（**明文**——测试里断言真的把登录态发出去了）。
    seen_auth: RwLock<Option<String>>,
    /// 收到的查询串（断言 `{{today}}` 真的替换进去了）。
    seen_query: RwLock<Option<String>>,
}

async fn checkin_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    state.checkin_hits.fetch_add(1, Ordering::SeqCst);
    *state.seen_auth.write().expect("写 Authorization 失败") = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    *state.seen_query.write().expect("写 query 失败") = uri.query().map(|q| q.to_string());
    (
        StatusCode::OK,
        r#"{"data":{"ok":true,"credits":400,"secret":"SHOULD-NOT-PERSIST"}}"#,
    )
        .into_response()
}

/// 断言不过的那个端点（业务字段 `ok = false`）。
async fn checkin_bad_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    state.checkin_hits.fetch_add(1, Ordering::SeqCst);
    (StatusCode::OK, r#"{"data":{"ok":false,"credits":0}}"#).into_response()
}

async fn balance_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
    state.balance_hits.fetch_add(1, Ordering::SeqCst);
    (StatusCode::OK, r#"{"data":{"balance":1200}}"#).into_response()
}

async fn start_mock() -> (String, Arc<MockState>) {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/api/checkin", post(checkin_handler))
        .route("/api/checkin-bad", post(checkin_bad_handler))
        .route("/api/balance", get(balance_handler))
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

/// 一份指向 mock 的声明式站点定义。
fn sample_spec(mock_url: &str) -> SpecSite {
    SpecSite {
        id: "demo-platform".to_string(),
        name: "演示平台".to_string(),
        method: Some("POST".to_string()),
        url: format!("{mock_url}/api/checkin"),
        headers: vec![SpecHeader {
            name: "Authorization".to_string(),
            value: "Bearer {{token}}".to_string(),
        }],
        query: vec![llm_router_lib::checkin::spec::SpecParam {
            name: "day".to_string(),
            value: "{{today}}".to_string(),
        }],
        body: None,
        credential_id: None,
        assert: SpecAssert {
            status: Some(200),
            json_paths: vec![SpecJsonPath {
                path: "data.ok".to_string(),
                equals: Some(serde_json::json!(true)),
            }],
        },
        reward: Some(SpecExtract {
            path: "data.credits".to_string(),
            unit: Some("point".to_string()),
        }),
        balance: Some(SpecBalance {
            method: None,
            url: format!("{mock_url}/api/balance"),
            path: "data.balance".to_string(),
            unit: Some("point".to_string()),
            headers: Vec::new(),
            query: Vec::new(),
        }),
        schedule: None,
    }
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
async fn spec_site_signs_in_and_extracts_the_reward_everywhere() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let spec = sample_spec(&mock_url);

    let summary = checkin_add_spec_site_of(&state, &spec, Some("SECRET-TOKEN")).expect("建站失败");
    assert_eq!(summary.plugin, "spec");
    assert!(!summary.enabled, "声明式站点默认不启用（02 §7.3）");

    let outcome = run(&state, &summary.id).await;
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(outcome.class, ReplayClass::Success);

    // 登录态真的发出去了（**且 `{{today}}` 被替换成当天日期**）
    assert_eq!(
        mock.seen_auth.read().expect("读 Authorization 失败").as_deref(),
        Some("Bearer SECRET-TOKEN"),
        "登录态应作为 Authorization 头发给上游"
    );
    let query = mock.seen_query.read().expect("读 query 失败").clone();
    assert!(query.unwrap_or_default().contains("day="), "查询参数应带上");

    // 积分：返回值 + reward_json + account.quota_json
    let reward = outcome.reward.as_ref().expect("应拿到积分");
    assert_eq!(reward.amount, 400);
    assert_eq!(reward.unit, "point");
    // 响应体里的字符串**不得进 reward_json**（密文边界）
    let reward_json = reward.to_json();
    assert!(!reward_json.contains("SHOULD-NOT-PERSIST"), "{reward_json}");

    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(&summary.id), 10))
        .expect("列结果失败");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ok);
    let stored = rows[0].reward_json.as_deref().expect("reward_json 不该为空");
    assert!(stored.contains("\"amount\":400"), "实际 {stored}");
    assert!(!stored.contains("SHOULD-NOT-PERSIST"), "实际 {stored}");

    // 账号额度快照（**本次拿到**）
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let config = llm_router_lib::checkin::spec::parse_site_config(&row).expect("配置应能解析");
    let account_id = config.account_id.expect("站点应有归属账号");
    let snapshot = llm_router_lib::store::account::read_reward(&state.store, &account_id)
        .expect("应有积分快照");
    assert_eq!(snapshot.points, 400);
    assert_eq!(snapshot.unit, "point");
}

#[tokio::test]
async fn a_failing_json_path_assertion_is_not_silently_a_success() {
    let (mock_url, _mock) = start_mock().await;
    let (_dir, state) = app_state();
    let mut spec = sample_spec(&mock_url);
    spec.url = format!("{mock_url}/api/checkin-bad");
    let summary = checkin_add_spec_site_of(&state, &spec, Some("SECRET-TOKEN")).expect("建站失败");

    let outcome = run(&state, &summary.id).await;
    assert!(!outcome.ok, "断言不过就不能算成功");
    assert_eq!(outcome.class, ReplayClass::AssertFailed);
    assert!(outcome.reward.is_none(), "失败没有积分");

    // 结果也落库（ok = 0）
    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(&summary.id), 10))
        .expect("列结果失败");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].ok);
}

#[tokio::test]
async fn a_site_without_a_credential_never_reaches_the_upstream() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let spec = sample_spec(&mock_url);
    // **不填登录态**建站（就像从别的机器导进来的目录）
    let summary = checkin_add_spec_site_of(&state, &spec, None).expect("建站失败");
    assert!(!summary.credential_ready, "登录态没填");

    let outcome = run(&state, &summary.id).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.class, ReplayClass::CredentialMissing);
    assert_eq!(
        mock.hits.load(Ordering::SeqCst),
        0,
        "缺凭据时**上游一个请求都不该发**"
    );
}

#[tokio::test]
async fn the_balance_is_a_separate_number_from_the_reward() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let spec = sample_spec(&mock_url);
    let summary = checkin_add_spec_site_of(&state, &spec, Some("SECRET-TOKEN")).expect("建站失败");

    // 先签到一次（拿「本次拿到」）
    let outcome = run(&state, &summary.id).await;
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(outcome.reward.as_ref().expect("有积分").amount, 400);

    // 再查「当前余额」——**另一个数**
    let balance = checkin_balance_of(&state, &summary.id).await.expect("查余额失败");
    assert_eq!(balance.amount, 1200);
    assert_eq!(balance.unit, "point");
    assert!(balance.message.contains("当前余额"), "{}", balance.message);
    assert_eq!(mock.balance_hits.load(Ordering::SeqCst), 1);

    // **余额不写进额度池**（额度池里仍然是「本次拿到」的 400）
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let config = llm_router_lib::checkin::spec::parse_site_config(&row).expect("配置应能解析");
    let account_id = config.account_id.expect("站点应有归属账号");
    let snapshot = llm_router_lib::store::account::read_reward(&state.store, &account_id)
        .expect("应有积分快照");
    assert_eq!(snapshot.points, 400, "余额不该覆盖「本次拿到」");
}

#[tokio::test]
async fn importing_a_catalog_reports_bad_entries_and_is_idempotent() {
    let (mock_url, _mock) = start_mock().await;
    let (_dir, state) = app_state();

    let good = sample_spec(&mock_url);
    let mut bad = sample_spec(&mock_url);
    bad.id = "broken-platform".to_string();
    bad.name = "坏站点".to_string();
    bad.url = "ftp://example.com/checkin".to_string();

    let catalog = serde_json::json!({
        "version": 1,
        "sites": [good, bad]
    })
    .to_string();

    let result = checkin_import_catalog_of(&state, &catalog).expect("导入应返回结果");
    assert_eq!(result.imported.len(), 1, "合法的应建站");
    assert_eq!(result.errors.len(), 1, "非法的**带原因**返回，不静默丢弃");
    assert_eq!(result.errors[0].id, "broken-platform");
    assert!(result.errors[0].reason.contains("http"), "{}", result.errors[0].reason);

    // 导入的站点**默认不启用**
    let site = &result.imported[0];
    assert!(!site.enabled, "导入的站点默认关（02 §7.3）");

    // **再导入一次 → 站点数不变**（幂等：同 spec id 视为更新）
    let again = checkin_import_catalog_of(&state, &catalog).expect("再导入");
    assert_eq!(again.imported.len(), 1);
    assert_eq!(again.errors.len(), 1);
    let sites = state
        .store
        .with_conn(|conn| checkin::list_sites(conn))
        .expect("列站点失败");
    assert_eq!(sites.len(), 1, "重复导入不应多建站点");
}

#[tokio::test]
async fn exporting_a_catalog_never_leaks_the_credential() {
    let (mock_url, _mock) = start_mock().await;
    let (_dir, state) = app_state();
    let spec = sample_spec(&mock_url);
    checkin_add_spec_site_of(&state, &spec, Some("SECRET-TOKEN")).expect("建站失败");

    let exported = checkin_export_catalog_of(&state).expect("导出失败");
    assert!(exported.contains("demo-platform"), "导出的目录应含站点");
    assert!(!exported.contains("SECRET-TOKEN"), "导出的目录**不得含凭据明文**");
    // 只有凭据 id
    let parsed: serde_json::Value = serde_json::from_str(&exported).expect("导出应是合法 JSON");
    let credential_id = parsed["sites"][0]["credentialId"]
        .as_str()
        .expect("应带凭据 id");
    assert!(!credential_id.is_empty());
    assert!(!credential_id.contains("SECRET"));
}

#[tokio::test]
async fn the_declaration_never_carries_the_plaintext_credential() {
    // **零明文（AGENT.md §5.3）**：站点行 / 库 / 保险库里都**不得出现登录态明文**——
    // 声明式站点的 `config_json` 里只有 `credentialId`（id，不是密文）。
    let (mock_url, _mock) = start_mock().await;
    let (dir, state) = app_state();
    let spec = sample_spec(&mock_url);
    checkin_add_spec_site_of(&state, &spec, Some("SECRET-TOKEN")).expect("建站失败");

    // 站点行本身
    let sites = state
        .store
        .with_conn(|conn| checkin::list_sites(conn))
        .expect("列站点失败");
    let row = sites.first().expect("应有站点");
    let config_json = row.config_json.as_deref().expect("站点应有定义");
    assert!(
        !config_json.contains("SECRET-TOKEN"),
        "站点定义里不得有登录态明文：{config_json}"
    );
    assert!(
        config_json.contains("credentialId"),
        "站点定义里只该有凭据 id：{config_json}"
    );

    // 库（含 WAL/SHM）与保险库：**逐字节扫**
    for name in ["store.db", "store.db-wal", "store.db-shm", "vault.bin"] {
        let path = dir.path().join(name);
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).expect("读文件失败");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("SECRET-TOKEN"),
            "{name} 里出现了登录态明文"
        );
    }
}

#[tokio::test]
async fn setting_the_login_state_keeps_the_declaration_intact() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let spec = sample_spec(&mock_url);
    let summary = checkin_add_spec_site_of(&state, &spec, None).expect("建站失败");

    // **后填登录态**（目录导入后的常见路径）——不能把声明式定义抹掉
    let updated = llm_router_lib::commands::checkin::checkin_set_credential_of(
        &state,
        &summary.id,
        "token",
        "SECRET-TOKEN",
    )
    .expect("填登录态失败");
    assert!(updated.credential_ready);

    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let config = llm_router_lib::checkin::spec::parse_site_config(&row).expect("定义应还在");
    assert_eq!(config.spec.id, "demo-platform");
    assert_eq!(config.spec.url, format!("{mock_url}/api/checkin"));

    let outcome = run(&state, &summary.id).await;
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(
        mock.seen_auth.read().expect("读 Authorization 失败").as_deref(),
        Some("Bearer SECRET-TOKEN")
    );
}
