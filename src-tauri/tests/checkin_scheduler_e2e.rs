//! T17 调度器 + 系统兜底 —— 端到端验收测试（mock 上游 + 真机 Windows 计划任务）。
//!
//! 验收判据（03 §2 T17）：「cron 调度、Windows 计划任务/launchd 注册、错过补签。
//! 验收：**关闭应用后计划任务仍触发并补签**」。
//!
//! 覆盖：
//! - **cron 调度 + 去重**：该跑的站点跑一次；**同一应触发周期再跑会被跳过**（上游只收到一次请求）；
//! - **错过补签**：站点该跑但应用关着 → 重新判定时仍然"该跑" → 补上；
//! - **Windows 系统兜底**：真机注册一个计划任务 → 查得到 → 动作里带 `--checkin-once`
//!   → 注销后查不到（这是"应用关着也能被唤起"的落地方式）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;
use llm_router_lib::checkin::har::{MarkedVariable, SiteTemplate, VarTarget};
use llm_router_lib::checkin::replay::ReplayEngine;
use llm_router_lib::checkin::scheduler;
use llm_router_lib::checkin::system;
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

// ── mock 上游 ──────────────────────────────────────────────────────────

#[derive(Default)]
struct MockState {
    hits: AtomicUsize,
}

async fn mock_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
) -> impl IntoResponse {
    state.hits.fetch_add(1, Ordering::SeqCst);
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

/// 用 T15 的保存路径落一个真实模板（真实值进保险库），返回站点 id。
fn save_template(state: &AppState) -> String {
    let input = SaveTemplateInput {
        path: FIXTURE.to_string(),
        entry_index: 0,
        site_name: "ModelScope".to_string(),
        channel_id: None,
        channel_name: Some("ModelScope·签到".to_string()),
        variables: vec![MarkedVariable {
            name: "acw_tc".to_string(),
            target: VarTarget::HeaderPart {
                field: "Cookie".to_string(),
                part: "acw_tc".to_string(),
            },
        }],
    };
    checkin_save_template_of(state, &input)
        .expect("保存模板失败")
        .id
}

/// 把模板的 URL 指向 mock，并写进站点行（带 cron 与启用开关）。
fn point_site_at_mock(state: &AppState, site_id: &str, url: &str, schedule: &str, enabled: bool) {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, site_id))
        .expect("读站点")
        .expect("站点应存在");
    let json = row.config_json.clone().expect("模板 JSON");
    let mut template: SiteTemplate = serde_json::from_str(&json).expect("模板应能解析");
    template.request.url = url.to_string();
    template.assert.status = Some(200);
    let mut updated = row.clone();
    updated.config_json = Some(serde_json::to_string(&template).expect("序列化模板"));
    updated.schedule = Some(schedule.to_string());
    updated.enabled = enabled;
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &updated))
        .expect("写站点失败");
}

// ── 用例 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn due_site_runs_once_per_cycle() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);
    // 每 5 分钟一次 → 最近一个应触发时刻在 5 分钟内 → 该跑
    point_site_at_mock(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/users/login/info"),
        "*/5 * * * *",
        true,
    );

    let engine = ReplayEngine::new().expect("构造引擎");
    let runs = scheduler::run_due_sites(&state.store, &state.vault, &engine, None).await;
    assert_eq!(runs.len(), 1);
    let outcome = runs[0].outcome.as_ref().expect("该跑");
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 1, "上游应收到一次请求");

    // **同一应触发周期再跑 → 跳过**（去重状态落库）
    let runs = scheduler::run_due_sites(&state.store, &state.vault, &engine, None).await;
    assert_eq!(runs.len(), 1);
    assert!(runs[0].outcome.is_none(), "本周期已跑过，不该再跑");
    assert!(runs[0]
        .skip_reason
        .as_deref()
        .unwrap_or_default()
        .contains("已跑过"));
    assert_eq!(mock.hits.load(Ordering::SeqCst), 1, "上游不该再收到请求");

    // 结果落库一行
    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(&site_id), 10))
        .expect("列结果失败");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ok);
}

#[tokio::test]
async fn missed_cycle_is_caught_up_on_the_next_run() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);
    // 每天 09:00 —— 无论现在几点，最近一个应触发时刻都已经过去（= 应用关着时漏掉了）
    point_site_at_mock(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/users/login/info"),
        "0 9 * * *",
        true,
    );

    let engine = ReplayEngine::new().expect("构造引擎");
    let runs = scheduler::run_due_sites(&state.store, &state.vault, &engine, None).await;
    assert_eq!(runs.len(), 1);
    let outcome = runs[0].outcome.as_ref().expect("漏掉的周期应被补上");
    assert!(outcome.ok, "{}", outcome.message);
    assert_eq!(mock.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disabled_site_is_never_run_by_the_scheduler() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);
    point_site_at_mock(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/users/login/info"),
        "*/5 * * * *",
        false,
    );

    let engine = ReplayEngine::new().expect("构造引擎");
    let runs = scheduler::run_due_sites(&state.store, &state.vault, &engine, None).await;
    assert_eq!(runs.len(), 1);
    assert!(runs[0].outcome.is_none(), "停用的站点不该被跑");
    assert!(runs[0]
        .skip_reason
        .as_deref()
        .unwrap_or_default()
        .contains("未启用"));
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn single_site_selection_limits_the_run() {
    let (mock_url, mock) = start_mock().await;
    let (_dir, state) = app_state();
    let site_id = save_template(&state);
    point_site_at_mock(
        &state,
        &site_id,
        &format!("{mock_url}/api/v1/users/login/info"),
        "*/5 * * * *",
        true,
    );

    let engine = ReplayEngine::new().expect("构造引擎");
    // 指定一个不存在的站点 → 什么都不跑（系统计划任务按站点唤起时的路径）
    let runs = scheduler::run_due_sites(
        &state.store,
        &state.vault,
        &engine,
        Some("site-not-exist"),
    )
    .await;
    assert!(runs.is_empty());
    assert_eq!(mock.hits.load(Ordering::SeqCst), 0);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_scheduled_task_is_registered_queried_and_removed() {
    // **真机**：Windows 计划任务（系统兜底）——注册 → 查得到 → 动作带 `--checkin-once` → 注销
    let site_id = format!("site_t17_test_{}", std::process::id());
    let _ = system::unregister(&site_id);

    let task = system::register(&site_id, "0 9 * * *").expect("注册计划任务应成功");
    assert!(task.registered);
    assert_eq!(task.name, format!("LLM Router Checkin - {site_id}"));
    assert!(
        task.frequency.as_deref().unwrap_or_default().contains("每天 09:00"),
        "频率描述：{:?}",
        task.frequency
    );

    // 查得到
    let status = system::query(&site_id).expect("查询应成功");
    assert!(status.registered, "注册后应查得到");

    // 动作串里必须带 `--checkin-once --site <id>`（这才是"应用关着也能被唤起"的落地方式）
    let action = system::action_for(&site_id).expect("动作串");
    assert!(action.contains("--checkin-once"), "{action}");
    assert!(action.contains(site_id.as_str()), "{action}");
    assert!(action.contains("llm-router"), "{action}");

    // 列表里能看到
    let names = system::list().expect("列任务失败");
    assert!(
        names.iter().any(|name| name == &task.name),
        "任务应出现在列表里：{names:?}"
    );

    // 注销
    assert!(system::unregister(&site_id).expect("注销应成功"));
    let status = system::query(&site_id).expect("查询应成功");
    assert!(!status.registered, "注销后应查不到");
    // 再注销一次不算错误
    assert!(!system::unregister(&site_id).expect("重复注销应不算错"));
}

#[cfg(target_os = "windows")]
#[test]
fn scheduled_task_frequency_maps_cron_shapes() {
    let site_id = format!("site_t17_shape_{}", std::process::id());
    let _ = system::unregister(&site_id);
    // 每 15 分钟
    let task = system::register(&site_id, "*/15 * * * *").expect("注册应成功");
    assert!(
        task.frequency.as_deref().unwrap_or_default().contains("每 15 分钟"),
        "{:?}",
        task.frequency
    );
    // 未识别的表达式 → 退回每天一次（宁可多跑不可漏跑）
    let task = system::register(&site_id, "0 9 1 11 *").expect("注册应成功");
    assert!(
        task.frequency.as_deref().unwrap_or_default().contains("每天 09:00"),
        "{:?}",
        task.frequency
    );
    let _ = system::unregister(&site_id);
}
