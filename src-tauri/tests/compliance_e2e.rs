//! T24 合规确认与渠道启用开关 —— 端到端验收测试（Copilot 半边）。
//!
//! 验收判据（03 §2 T24、02 §6.8）：
//! - **后端强制**：绕过前端直接调 IPC 把受限渠道置为启用 → **被拒**（前端不是唯一防线）；
//! - 记录一次 ToS 确认后 → 可以启用 → **模型出现在 `GET /v1/models`**；
//! - **保存渠道表单不会抹掉确认记录**；
//! - **关闭永远不需要确认**；
//! - 删渠道 → 确认记录跟着消失。
//!
//! **全程不出网**：上游与授权都不碰网络（不测 OAuth，只测「启用闸门」）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use llm_router_lib::commands::channel;
use llm_router_lib::commands::copilot;
use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::router::AliasRouter;
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::gateway::usage::CharRatioEstimator;
use llm_router_lib::providers::compliance;
use llm_router_lib::store::channels::{
    self, CatalogModel, ChannelInput, ModelInput, OAUTH_PROVIDER,
};
use llm_router_lib::store::usage::StoreUsageSink;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;
use tower::ServiceExt;

// ── 脚手架 ──────────────────────────────────────────────────────────────

struct Fixture {
    _dir: tempfile::TempDir,
    app: AppState,
    gateway: GatewayState,
    key: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault = Arc::new(std::sync::RwLock::new(
        Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
    ));
    let gateway = GatewayState {
        router: Arc::new(AliasRouter::new(Arc::clone(&store))),
        forwarder: Arc::new(llm_router_lib::gateway::fallback::Forwarder::new(
            OutboundClient::new().expect("出站客户端"),
            Arc::new(CircuitBreaker::new()),
            Arc::new(StreamUsageDowngrades::default()),
            Arc::clone(&vault),
            Arc::clone(&store),
            Arc::new(StoreUsageSink::new(Arc::clone(&store))),
            Arc::new(CharRatioEstimator),
        )),
        store: Arc::clone(&store),
        limiter: Arc::new(RateLimiter::new()),
    };
    let app = AppState {
        gateway_addr: "127.0.0.1:8787".to_string(),
        gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
        store: Arc::clone(&store),
        vault,
        limiter: Arc::new(RateLimiter::new()),
        circuit: Arc::new(CircuitBreaker::new()),
    };

    let key = auth::generate_client_key();
    let hash = auth::hash_client_key(&key);
    let prefix = auth::key_prefix(&key);
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                 VALUES ('ck1', 't24', ?1, ?2, ?3)",
                rusqlite::params![hash, prefix, now],
            )?;
            Ok(())
        })
        .expect("写入 client key");

    Fixture {
        _dir: dir,
        app,
        gateway,
        key,
    }
}

/// 建一条**已授权形态**的 Copilot 渠道（OAuth 行 + 一条模型），返回渠道 id。
///
/// 走的都是**真实后端路径**：`ensure_oauth_channel`（T20 建行，默认关闭）
/// + `sync_models_from_catalog`（T21 落模型）。
fn copilot_channel(app: &AppState) -> String {
    let channel_id = channels::ensure_oauth_channel(
        &app.store,
        copilot::CHANNEL_NAME,
        OAUTH_PROVIDER,
        "https://api.individual.githubcopilot.com",
    )
    .expect("建 Copilot 渠道");
    channels::sync_models_from_catalog(
        &app.store,
        &channel_id,
        &[CatalogModel {
            alias: "gpt-4o-mini-2024-07-18".to_string(),
            upstream_id: "gpt-4o-mini-2024-07-18".to_string(),
            context_len: Some(128_000),
        }],
    )
    .expect("同步模型目录");
    channel_id
}

/// 经**真网关**列出 `/v1/models` 的别名。
async fn model_aliases(f: &Fixture) -> Vec<String> {
    let response = server::router(f.gateway.clone())
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/models")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {}", f.key))
                .body(Body::empty())
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
    value["data"]
        .as_array()
        .expect("data 应为数组")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// 一条「保存渠道表单」的入参（**带模型**，走 `channel_save` 的完整路径）。
fn channel_form(channel_id: &str, enabled: bool) -> ChannelInput {
    ChannelInput {
        id: Some(channel_id.to_string()),
        name: copilot::CHANNEL_NAME.to_string(),
        provider: OAUTH_PROVIDER.to_string(),
        base_url: "https://api.individual.githubcopilot.com".to_string(),
        kind: Some("oauth".to_string()),
        enabled,
        priority: None,
        meta: channels::ChannelMeta::default(),
        models: vec![ModelInput {
            alias: "gpt-4o-mini-2024-07-18".to_string(),
            upstream_id: "gpt-4o-mini-2024-07-18".to_string(),
            enabled: true,
        }],
    }
}

// ── 判据 ①：后端强制（绕过前端也没用）──────────────────────────────

#[tokio::test]
async fn bypassing_the_ui_cannot_enable_a_restricted_channel() {
    let f = fixture();
    let channel_id = copilot_channel(&f.app);

    // 未确认 → 直接把渠道存成启用 → **被拒**
    // （走**存储层** `save_channel` = 最强的绕过姿势：连 IPC 都不经过）
    let err = channels::save_channel(&f.app.store, &channel_form(&channel_id, true))
        .expect_err("未确认不该放行");
    let msg = err.to_string();
    assert!(msg.contains("尚未确认 ToS"), "{msg}");
    assert!(msg.contains("GitHub Copilot 授权"), "错误要指路：{msg}");

    // 渠道**仍然是关闭的**（拒绝要真的没生效）
    let summary = channels::channel_summary(&f.app.store, &channel_id).expect("读渠道");
    assert!(!summary.enabled, "被拒的启用不该有副作用");

    // 另一条路径（专用开关 IPC）同样被拒
    let err = channel::channel_set_enabled_of(&f.app, &channel_id, true)
        .expect_err("专用开关也该拦");
    assert!(err.contains("尚未确认 ToS"), "{err}");
}

#[tokio::test]
async fn a_forged_acceptance_from_the_caller_is_ignored() {
    let f = fixture();
    let channel_id = copilot_channel(&f.app);

    // 前端伪造「已确认」→ 不认
    let mut form = channel_form(&channel_id, true);
    form.meta.tos_accepted_at = Some(1_700_000_000);
    form.meta.tos_statement_version = Some(compliance::COPILOT_TOS_VERSION.to_string());
    let err = channels::save_channel(&f.app.store, &form).expect_err("伪造的确认不该算数");
    assert!(err.to_string().contains("尚未确认 ToS"), "{err}");
}

// ── 判据 ②③：确认 → 启用 → 模型对外可见 ─────────────────────────────

#[tokio::test]
async fn after_the_acceptance_the_channel_can_be_enabled_and_publishes_models() {
    let f = fixture();
    let channel_id = copilot_channel(&f.app);

    // 未启用 → 模型**不在**对外列表里（默认关闭的语义）
    assert!(
        model_aliases(&f).await.is_empty(),
        "受限渠道默认关闭时不该对外发布模型"
    );

    // 记录确认（**只记录，不启用**）
    let tos = copilot::copilot_tos_accept_of(&f.app).expect("记录确认失败");
    assert!(tos.accepted_at.is_some());
    assert_eq!(
        tos.statement_version.as_deref(),
        Some(compliance::COPILOT_TOS_VERSION)
    );
    let after_accept = channels::channel_summary(&f.app.store, &channel_id).expect("读渠道");
    assert!(
        !after_accept.enabled,
        "记录确认本身**不启用**渠道（确认与启用分两步）"
    );

    // 再启用 → 成功
    let enabled = channel::channel_set_enabled_of(&f.app, &channel_id, true).expect("启用失败");
    assert!(enabled.enabled);

    // 状态视图带上确认时刻
    let status = copilot::copilot_status_of(&f.app).expect("查状态");
    assert!(status.channel_enabled, "状态应显示已启用");
    assert!(status.tos_accepted_at.is_some(), "状态应带回确认时刻");

    // 模型出现在**真网关**的 `/v1/models`
    let aliases = model_aliases(&f).await;
    assert!(
        aliases.contains(&"gpt-4o-mini-2024-07-18".to_string()),
        "启用后模型应出现在 /v1/models，实际 {aliases:?}"
    );
}

// ── 判据 ④：保存渠道表单不抹掉确认记录 ───────────────────────────────

#[tokio::test]
async fn saving_the_channel_form_does_not_erase_the_acceptance() {
    let f = fixture();
    let channel_id = copilot_channel(&f.app);
    copilot::copilot_tos_accept_of(&f.app).expect("记录确认失败");

    // 表单保存（meta 里**不带** tos 字段）→ 确认记录要保住
    let saved = channels::save_channel(&f.app.store, &channel_form(&channel_id, true))
        .expect("保存失败");
    assert!(
        saved.meta.tos_accepted_at.is_some(),
        "保存渠道表单不能抹掉 ToS 确认记录"
    );
    assert_eq!(
        saved.meta.tos_statement_version.as_deref(),
        Some(compliance::COPILOT_TOS_VERSION)
    );
    assert!(saved.enabled, "已确认的渠道保存时保持启用");
}

// ── 判据 ⑤：关闭永远不需要确认 ──────────────────────────────────────

#[tokio::test]
async fn turning_the_channel_off_never_requires_an_acceptance() {
    let f = fixture();
    let channel_id = copilot_channel(&f.app);
    copilot::copilot_tos_accept_of(&f.app).expect("记录确认失败");
    channel::channel_set_enabled_of(&f.app, &channel_id, true).expect("启用失败");

    let off = channel::channel_set_enabled_of(&f.app, &channel_id, false).expect("关闭失败");
    assert!(!off.enabled);
    assert!(model_aliases(&f).await.is_empty(), "关闭后模型应下线");
}

// ── 判据 ⑥：删渠道 → 确认记录跟着消失 ────────────────────────────────

#[tokio::test]
async fn deleting_the_channel_takes_the_acceptance_with_it() {
    let f = fixture();
    let channel_id = copilot_channel(&f.app);
    copilot::copilot_tos_accept_of(&f.app).expect("记录确认失败");

    channel::channel_delete_of(&f.app, &channel_id).expect("删除失败");
    let rows = channels::list_channels(&f.app.store).expect("列渠道");
    assert!(rows.is_empty(), "删掉的渠道不该留下残渣");

    // 确认记录跟着渠道走 → 新建同名渠道是**全新的**（没有继承的确认）
    let fresh = channels::ensure_oauth_channel(
        &f.app.store,
        copilot::CHANNEL_NAME,
        OAUTH_PROVIDER,
        "https://api.individual.githubcopilot.com",
    )
    .expect("重建渠道");
    let summary = channels::channel_summary(&f.app.store, &fresh).expect("读渠道");
    assert!(
        summary.meta.tos_accepted_at.is_none(),
        "新渠道不该继承旧渠道的 ToS 确认"
    );
}

// ── 附：非受限渠道完全不受影响 ──────────────────────────────────────

#[tokio::test]
async fn unrestricted_channels_are_not_gated_at_all() {
    let f = fixture();
    let input = ChannelInput {
        name: "我的代理".to_string(),
        provider: "custom".to_string(),
        base_url: "https://proxy.example.com".to_string(),
        models: vec![ModelInput {
            alias: "my-alias".to_string(),
            upstream_id: "my-model".to_string(),
            enabled: true,
        }],
        ..ChannelInput::default()
    };
    let summary = channel::channel_save_of(&f.app, input).expect("保存失败");
    assert!(summary.enabled, "自定义渠道不受合规限制");
    let enabled = channel::channel_set_enabled_of(&f.app, &summary.id, true).expect("启用失败");
    assert!(enabled.enabled);
}
