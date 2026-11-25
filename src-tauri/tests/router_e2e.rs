//! T5 路由与别名 —— 端到端验收测试。
//!
//! 验收判据（03 §2 T5）：
//! 1. route_group 命中 / 单别名命中 / 未命中 三条路径单测通过；
//! 2. sticky_session 对同一 client_key + 模型连续请求返回同渠道（多账号池场景）。

use std::sync::Arc;

use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::router::{AliasRouter, RouteError, Strategy};
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::store::Store;
use tower::ServiceExt;

/// 测试用状态。
struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    router: Arc<AliasRouter>,
    state: GatewayState,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let router = Arc::new(AliasRouter::new(Arc::clone(&store)));
    let state = GatewayState {
        router: Arc::clone(&router),
        store: Arc::clone(&store),
        limiter: Arc::new(RateLimiter::new()),
    };
    Fixture {
        _dir: dir,
        store,
        router,
        state,
    }
}

/// 插入渠道。
#[allow(clippy::too_many_arguments)]
fn seed_channel(
    store: &Store,
    id: &str,
    name: &str,
    provider: &str,
    priority: i64,
    enabled: bool,
) {
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at)
                 VALUES (?1, ?2, 'api_key', ?3, 'https://example.invalid', ?4, ?5, ?6)",
                rusqlite::params![id, name, provider, enabled, priority, now],
            )?;
            Ok(())
        })
        .expect("插入渠道");
}

/// 插入模型。
fn seed_model(store: &Store, id: &str, channel_id: &str, alias: &str, upstream: &str) {
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                rusqlite::params![id, channel_id, upstream, alias],
            )?;
            Ok(())
        })
        .expect("插入模型");
}

/// 插入路由组。
fn seed_group(store: &Store, id: &str, alias: &str, strategy: &str) {
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO route_group (id, alias, strategy) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, alias, strategy],
            )?;
            Ok(())
        })
        .expect("插入路由组");
}

/// 插入路由组成员。
fn seed_member(store: &Store, group_id: &str, model_id: &str, weight: i64) {
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO route_member (group_id, model_id, weight) VALUES (?1, ?2, ?3)",
                rusqlite::params![group_id, model_id, weight],
            )?;
            Ok(())
        })
        .expect("插入路由组成员");
}

/// 插入凭证（只放 id 与密文占位；明文不参与）。
fn seed_credential(store: &Store, id: &str, channel_id: &str) {
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO credential (id, channel_id, kind, ciphertext, nonce, key_version, created_at, updated_at)
                 VALUES (?1, ?2, 'api_key', x'00', x'00', 1, ?3, ?3)",
                rusqlite::params![id, channel_id, now],
            )?;
            Ok(())
        })
        .expect("插入凭证");
}

/// 插入价目表。
fn seed_price(
    store: &Store,
    provider: &str,
    pattern: &str,
    input_per_1m: i64,
    output_per_1m: i64,
) {
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO price (provider, model_pattern, input_per_1m, output_per_1m)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![provider, pattern, input_per_1m, output_per_1m],
            )?;
            Ok(())
        })
        .expect("插入价目表");
}

/// 建一个 client key，返回明文。
fn seed_client_key(store: &Store, id: &str) -> String {
    let key = auth::generate_client_key();
    let hash = auth::hash_client_key(&key);
    let prefix = auth::key_prefix(&key);
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                 VALUES (?1, '测试', ?2, ?3, ?4)",
                rusqlite::params![id, hash, prefix, now],
            )?;
            Ok(())
        })
        .expect("插入 client key");
    key
}

/// 关闭渠道。
fn disable_channel(store: &Store, id: &str) {
    store
        .with_conn(|conn| {
            conn.execute("UPDATE channel SET enabled = 0 WHERE id = ?1", [id])?;
            Ok(())
        })
        .expect("禁用渠道");
}

/// 典型数据：两个渠道 + 两个模型 + 一个 priority 路由组。
fn seed_two_channels(f: &Fixture) {
    seed_channel(&f.store, "c1", "DeepSeek 官方", "deepseek", 10, true);
    seed_channel(&f.store, "c2", "Kimi 官方", "kimi", 20, true);
    seed_model(&f.store, "m1", "c1", "ds-chat", "deepseek-chat");
    seed_model(&f.store, "m2", "c2", "moonshot-chat", "moonshot-v1-8k");
    seed_credential(&f.store, "cred1", "c1");
    seed_credential(&f.store, "cred2", "c2");
    seed_group(&f.store, "g1", "smart", "priority");
    seed_member(&f.store, "g1", "m1", 100);
    seed_member(&f.store, "g1", "m2", 100);
}

// ── 三条路径（验收判据 1）──────────────────────────────────────────────

#[tokio::test]
async fn acceptance_route_group_hit_selects_by_priority() {
    let f = fixture();
    seed_two_channels(&f);

    let plan = f
        .router
        .resolve("ck1", "smart")
        .await
        .expect("路由组应命中");

    assert_eq!(plan.strategy, Strategy::Priority);
    assert_eq!(plan.alias, "smart");
    // c1 的 priority=10 小于 c2 的 20 → 选 c1
    assert_eq!(plan.primary.channel_id, "c1");
    assert_eq!(plan.primary.channel_name, "DeepSeek 官方");
    assert_eq!(plan.primary.upstream_model, "deepseek-chat");
    assert_eq!(plan.primary.credential_id.as_deref(), Some("cred1"));
    // 候选链：首选在首位，其余按 priority 排序
    assert_eq!(plan.candidates.len(), 2);
    assert_eq!(plan.candidates[0].channel_id, "c1");
    assert_eq!(plan.candidates[1].channel_id, "c2");
}

#[tokio::test]
async fn acceptance_single_alias_hit() {
    let f = fixture();
    seed_two_channels(&f);

    let plan = f
        .router
        .resolve("ck1", "moonshot-chat")
        .await
        .expect("单别名应命中");

    assert_eq!(plan.strategy, Strategy::Priority);
    assert_eq!(plan.primary.channel_id, "c2");
    assert_eq!(plan.primary.upstream_model, "moonshot-v1-8k");
    assert_eq!(plan.candidates.len(), 1, "单别名没有 fallback 备选");
}

#[tokio::test]
async fn acceptance_alias_miss_returns_available_aliases() {
    let f = fixture();
    seed_two_channels(&f);

    let err = f
        .router
        .resolve("ck1", "not-exist")
        .await
        .expect_err("未命中应报错");

    match err {
        RouteError::AliasNotFound { alias, available } => {
            assert_eq!(alias, "not-exist");
            assert!(
                available.contains(&"ds-chat".to_string()),
                "可用别名应含单别名：{available:?}"
            );
            assert!(
                available.contains(&"smart".to_string()),
                "可用别名应含路由组别名：{available:?}"
            );
            assert_eq!(available.len(), 3, "可用别名：{available:?}");
        }
        other => panic!("应为 AliasNotFound，实际 {other:?}"),
    }
}

// ── sticky_session（验收判据 2）────────────────────────────────────────

#[tokio::test]
async fn acceptance_sticky_session_pins_same_channel_for_same_client_key() {
    let f = fixture();
    seed_channel(&f.store, "c1", "DeepSeek 官方", "deepseek", 10, true);
    seed_channel(&f.store, "c2", "Kimi 官方", "kimi", 20, true);
    seed_model(&f.store, "m1", "c1", "ds-chat", "deepseek-chat");
    seed_model(&f.store, "m2", "c2", "moonshot-chat", "moonshot-v1-8k");
    seed_group(&f.store, "g1", "smart", "sticky_session");
    seed_member(&f.store, "g1", "m1", 100);
    seed_member(&f.store, "g1", "m2", 100);

    let first = f.router.resolve("ck-same", "smart").await.expect("首次解析");
    assert_eq!(first.strategy, Strategy::StickySession);
    assert_eq!(first.primary.channel_id, "c1");

    // 连续三次请求 → 同一渠道
    for _ in 0..3 {
        let plan = f.router.resolve("ck-same", "smart").await.expect("解析");
        assert_eq!(plan.primary.channel_id, "c1", "同一 client_key 必须粘住同一渠道");
    }

    // 粘性是真的"粘住"：新增一个优先级更高的候选后，原 client 仍留在原渠道
    seed_channel(&f.store, "c3", "GLM 官方", "glm", 1, true);
    seed_model(&f.store, "m3", "c3", "glm-chat", "glm-4-plus");
    seed_member(&f.store, "g1", "m3", 100);

    let pinned = f.router.resolve("ck-same", "smart").await.expect("解析");
    assert_eq!(
        pinned.primary.channel_id, "c1",
        "已粘住的 client 不应被新候选抢走"
    );

    // 新 client key → 按 priority 重新分配
    let fresh = f.router.resolve("ck-fresh", "smart").await.expect("解析");
    assert_eq!(
        fresh.primary.channel_id, "c3",
        "新 client 应按 priority 选 c3（priority=1）"
    );
}

#[tokio::test]
async fn acceptance_sticky_session_reassigns_when_pinned_channel_disabled() {
    let f = fixture();
    seed_two_channels(&f);
    f.store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE route_group SET strategy = 'sticky_session' WHERE id = 'g1'",
                [],
            )?;
            Ok(())
        })
        .expect("改策略");

    let first = f.router.resolve("ck1", "smart").await.expect("首次解析");
    assert_eq!(first.primary.channel_id, "c1");

    // 原粘性渠道被禁用 → 必须重新分配，而不是继续打已禁用的渠道
    disable_channel(&f.store, "c1");
    let second = f.router.resolve("ck1", "smart").await.expect("二次解析");
    assert_eq!(second.primary.channel_id, "c2");
}

// ── round_robin / cost ───────────────────────────────────────────────

#[tokio::test]
async fn acceptance_round_robin_rotates_candidates() {
    let f = fixture();
    seed_two_channels(&f);
    f.store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE route_group SET strategy = 'round_robin' WHERE id = 'g1'",
                [],
            )?;
            Ok(())
        })
        .expect("改策略");

    let first = f.router.resolve("ck1", "smart").await.expect("解析");
    let second = f.router.resolve("ck1", "smart").await.expect("解析");
    let third = f.router.resolve("ck1", "smart").await.expect("解析");

    assert_eq!(first.strategy, Strategy::RoundRobin);
    assert_eq!(first.primary.channel_id, "c1");
    assert_eq!(second.primary.channel_id, "c2", "第二次应轮转到另一个候选");
    assert_eq!(third.primary.channel_id, "c1", "第三次回到第一个");
}

#[tokio::test]
async fn acceptance_cost_strategy_prefers_cheapest_candidate() {
    let f = fixture();
    seed_two_channels(&f);
    // deepseek 贵、kimi 便宜
    seed_price(&f.store, "deepseek", "deepseek-*", 2_000_000, 8_000_000);
    seed_price(&f.store, "kimi", "moonshot-*", 1_000, 2_000);
    f.store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE route_group SET strategy = 'cost' WHERE id = 'g1'",
                [],
            )?;
            Ok(())
        })
        .expect("改策略");

    let plan = f.router.resolve("ck1", "smart").await.expect("解析");
    assert_eq!(plan.strategy, Strategy::Cost);
    assert_eq!(
        plan.primary.channel_id, "c2",
        "kimi 更便宜，应选 c2（priority 更高但成本更低）"
    );
}

// ── alias 唯一索引 ───────────────────────────────────────────────────

#[tokio::test]
async fn acceptance_model_alias_is_unique() {
    let f = fixture();
    seed_two_channels(&f);

    // 重复别名必须被唯一索引挡住（idx_model_alias，T1 建表时已建）
    let result = f.store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
             VALUES ('m-dup', 'c2', 'dup-upstream', 'ds-chat', 1)",
            [],
        )?;
        Ok(())
    });
    assert!(result.is_err(), "重复 alias 必须插入失败");

    // 路由组别名也唯一
    let result = f.store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO route_group (id, alias, strategy) VALUES ('g-dup', 'smart', 'priority')",
            [],
        )?;
        Ok(())
    });
    assert!(result.is_err(), "重复 route_group alias 必须插入失败");
}

// ── 网关级：未命中 400 + 可用别名 ─────────────────────────────────────

#[tokio::test]
async fn acceptance_gateway_returns_400_with_available_aliases_on_miss() {
    let f = fixture();
    seed_two_channels(&f);
    let key = seed_client_key(&f.store, "ck1");

    let response = server::router(f.state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {key}"))
                .body(axum::body::Body::from(
                    r#"{"model":"not-exist","messages":[]}"#,
                ))
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
    assert_eq!(value["error"]["type"], "invalid_request_error");
    assert_eq!(value["error"]["code"], "model_not_found");
    let available = value["error"]["available_models"]
        .as_array()
        .expect("应附可用别名列表");
    let names: Vec<&str> = available.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"ds-chat"), "可用别名：{names:?}");
    assert!(names.contains(&"smart"), "可用别名：{names:?}");
}

#[tokio::test]
async fn acceptance_gateway_resolves_route_group_before_outbound() {
    let f = fixture();
    seed_two_channels(&f);
    let key = seed_client_key(&f.store, "ck1");

    let response = server::router(f.state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {key}"))
                .body(axum::body::Body::from(
                    r#"{"model":"smart","messages":[]}"#,
                ))
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");

    // 出站转发在 T6 落地 → 503，但错误体必须带出已解析到的渠道
    assert_eq!(response.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
    assert_eq!(value["error"]["type"], "api_error");
    assert_eq!(value["error"]["channel"], "DeepSeek 官方");
    let message = value["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("deepseek-chat"), "消息应带出上游模型：{message}");
}

#[tokio::test]
async fn acceptance_models_lists_route_group_aliases() {
    let f = fixture();
    seed_two_channels(&f);
    let key = seed_client_key(&f.store, "ck1");

    let response = server::router(f.state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/models")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {key}"))
                .body(axum::body::Body::empty())
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
    let ids: Vec<&str> = value["data"]
        .as_array()
        .expect("data 应为数组")
        .iter()
        .filter_map(|v| v["id"].as_str())
        .collect();
    assert!(ids.contains(&"ds-chat"), "别名列表：{ids:?}");
    assert!(ids.contains(&"smart"), "路由组别名也要对外可见：{ids:?}");
}
