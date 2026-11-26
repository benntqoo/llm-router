//! T7 用量采集 —— 端到端验收测试（mock 上游）。
//!
//! 验收判据（03 §2 T7）：
//! - **流式 / 非流式各一条**：`usage_log` 的字段与上游返回一致；
//! - **429 触发后** `quota_json` 记录更新（限额自学习，02 §8.2）。
//!
//! 附带覆盖：上游没回 usage 时走估算入口（`estimated=1`）、失败尝试记 `error` 行且不估 token。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::{header::CONTENT_TYPE, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::estimator::TiktokenEstimator;
use llm_router_lib::gateway::fallback::Forwarder;
use llm_router_lib::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::router::AliasRouter;
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::gateway::usage::{CharRatioEstimator, TokenEstimator};
use llm_router_lib::store::analytics::{aggregate, recent_requests, AggregateQuery, Dim};
use llm_router_lib::store::quota::read_quota;
use llm_router_lib::store::usage::StoreUsageSink;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use tower::ServiceExt;

// ── mock 上游 ──────────────────────────────────────────────────────────

#[derive(Default)]
struct MockState {
    hits: AtomicUsize,
}

/// 非流式：带完整 usage 的成功响应。
const NON_STREAM_BODY: &str = r#"{"id":"chatcmpl-usage","choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":31,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":5},"completion_tokens_details":{"reasoning_tokens":3}}}"#;

/// 非流式：成功但**没有** usage（走估算入口）。
const NON_STREAM_NO_USAGE_BODY: &str =
    r#"{"id":"chatcmpl-nousage","choices":[{"message":{"role":"assistant","content":"你好，世界"}}]}"#;

/// 非流式：成功，`usage` **只给**输入 token 数（缺输出 ⇒ 估算器补缺的那一项）。
const NON_STREAM_PARTIAL_USAGE_BODY: &str =
    r#"{"id":"chatcmpl-partial","choices":[{"message":{"role":"assistant","content":"你好，世界"}}],"usage":{"prompt_tokens":31}}"#;

/// 流式：增量帧 + 末帧 usage（`stream_options.include_usage` 的形态）。
const STREAM_BODY: &str = concat!(
    "data: {\"id\":\"chunk-0\",\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
    "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n",
    "data: {\"id\":\"chunk-2\",\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":9}}\n\n",
    "data: [DONE]\n\n",
);

async fn mock_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
    uri: Uri,
) -> Response {
    state.hits.fetch_add(1, Ordering::SeqCst);
    let scenario = uri
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();

    match scenario.as_str() {
        "usage" => (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            NON_STREAM_BODY,
        )
            .into_response(),
        "nousage" => (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            NON_STREAM_NO_USAGE_BODY,
        )
            .into_response(),
        "partialusage" => (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            NON_STREAM_PARTIAL_USAGE_BODY,
        )
            .into_response(),
        "sseusage" => {
            // 刻意延迟 60ms：让「首个 token 到达时刻（TTFT）」与「输出耗时」可观测
            tokio::time::sleep(Duration::from_millis(60)).await;
            (
                StatusCode::OK,
                [(CONTENT_TYPE, "text/event-stream")],
                STREAM_BODY,
            )
                .into_response()
        }
        "slowusage" => {
            // 刻意延迟 120ms：让「输出速度（tokens·s⁻¹）」能按公式复算
            tokio::time::sleep(Duration::from_millis(120)).await;
            (
                StatusCode::OK,
                [(CONTENT_TYPE, "application/json")],
                NON_STREAM_BODY,
            )
                .into_response()
        }
        "fail429" => (
            StatusCode::TOO_MANY_REQUESTS,
            [
                ("retry-after", "5"),
                ("x-ratelimit-limit-requests", "60"),
                ("x-ratelimit-limit-tokens", "120000"),
                ("x-ratelimit-remaining-requests", "0"),
            ],
            "slow down",
        )
            .into_response(),
        "fail500" => (StatusCode::INTERNAL_SERVER_ERROR, "upstream boom").into_response(),
        _ => (StatusCode::NOT_FOUND, "unknown scenario").into_response(),
    }
}

/// 启动 mock 上游，返回 base_url。
async fn start_mock() -> String {
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
    format!("http://{addr}")
}

// ── 网关夹具 ───────────────────────────────────────────────────────────

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    state: GatewayState,
    vault: Arc<RwLock<Vault>>,
    key: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault = Arc::new(RwLock::new(
        Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
    ));
    // 与生产同款：熔断器带落库、用量采集走 StoreUsageSink
    let forwarder = Arc::new(Forwarder::new(
        OutboundClient::new().expect("出站客户端"),
        Arc::new(CircuitBreaker::with_persistence(Arc::new(
            StoreCircuitPersistence::new(Arc::clone(&store)),
        ))),
        Arc::new(StreamUsageDowngrades::default()),
        Arc::clone(&vault),
        Arc::clone(&store),
        Arc::new(StoreUsageSink::new(Arc::clone(&store))),
        // 与产线同款：T12 起默认用 tiktoken-rs 估算器
        Arc::new(TiktokenEstimator),
    ));
    let state = GatewayState {
        router: Arc::new(AliasRouter::new(Arc::clone(&store))),
        forwarder: Arc::clone(&forwarder),
        store: Arc::clone(&store),
        limiter: Arc::new(RateLimiter::new()),
    };

    // client key（只存哈希）
    let key = auth::generate_client_key();
    let hash = auth::hash_client_key(&key);
    let prefix = auth::key_prefix(&key);
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                 VALUES ('ck1', 't7', ?1, ?2, ?3)",
                rusqlite::params![hash, prefix, now],
            )?;
            Ok(())
        })
        .expect("写入 client key");

    Fixture {
        _dir: dir,
        store,
        state,
        vault,
        key,
    }
}

/// 种入一个渠道（指向 mock 的某个场景）+ 模型 + 凭证。
fn seed_channel(f: &Fixture, id: &str, name: &str, scenario: &str, base_url: &str, alias: &str) {
    let now = llm_router_lib::vault::now_secs();
    f.store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at)
                 VALUES (?1, ?2, 'api_key', 'deepseek', ?3, 1, 100, ?4)",
                rusqlite::params![id, name, format!("{base_url}/{scenario}"), now],
            )?;
            conn.execute(
                "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                 VALUES (?1, ?2, 'deepseek-chat', ?3, 1)",
                rusqlite::params![format!("m_{id}"), id, alias],
            )?;
            Ok(())
        })
        .expect("写入渠道与模型");
    f.vault
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .put_credential(
            id,
            "api_key",
            &llm_router_lib::vault::crypto::SecretBytes::from_slice(b"sk-mock"),
        )
        .expect("写入凭证");
}

/// 发起一次 chat 请求（流式或非流式）。
async fn chat(f: &Fixture, model: &str, stream: bool) -> Response {
    chat_with_prompt(f, model, stream, "hi").await
}

/// 发起一次 chat 请求，带自定义提示词（估算口径要按真实提示词对账）。
async fn chat_with_prompt(f: &Fixture, model: &str, stream: bool, prompt: &str) -> Response {
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": stream
    });
    server::router(f.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {}", f.key))
                .body(Body::from(body.to_string()))
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功")
}

async fn read_body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    String::from_utf8_lossy(&bytes).to_string()
}

// ── usage_log 读取 ────────────────────────────────────────────────────

/// `usage_log` 的一行。
#[derive(Debug, Clone)]
struct UsageRow {
    ts: i64,
    client_key_id: Option<String>,
    channel_id: Option<String>,
    account_id: Option<String>,
    model_alias: Option<String>,
    upstream_model: Option<String>,
    protocol: Option<String>,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    estimated: i64,
    ttft_ms: Option<i64>,
    duration_ms: Option<i64>,
    status: Option<String>,
    error_kind: Option<String>,
    cost_micros: Option<i64>,
}

/// 读出全部 `usage_log` 行。
fn query_usage_rows(store: &Store) -> Vec<UsageRow> {
    store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ts, client_key_id, channel_id, account_id, model_alias, upstream_model,
                        protocol, prompt_tokens, completion_tokens, cache_read_tokens,
                        cache_write_tokens, reasoning_tokens, estimated, ttft_ms, duration_ms,
                        status, error_kind, cost_micros
                 FROM usage_log ORDER BY id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(UsageRow {
                    ts: row.get(0)?,
                    client_key_id: row.get(1)?,
                    channel_id: row.get(2)?,
                    account_id: row.get(3)?,
                    model_alias: row.get(4)?,
                    upstream_model: row.get(5)?,
                    protocol: row.get(6)?,
                    prompt_tokens: row.get(7)?,
                    completion_tokens: row.get(8)?,
                    cache_read_tokens: row.get(9)?,
                    cache_write_tokens: row.get(10)?,
                    reasoning_tokens: row.get(11)?,
                    estimated: row.get(12)?,
                    ttft_ms: row.get(13)?,
                    duration_ms: row.get(14)?,
                    status: row.get(15)?,
                    error_kind: row.get(16)?,
                    cost_micros: row.get(17)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .expect("读取 usage_log")
}

/// 等待 `usage_log` 出现指定行数（落库走阻塞线程池，是异步的）。
async fn wait_usage_rows(store: &Store, expected: usize) -> Vec<UsageRow> {
    for _ in 0..300 {
        let rows = query_usage_rows(store);
        if rows.len() >= expected {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let rows = query_usage_rows(store);
    panic!("等待 {expected} 行 usage_log 超时，实际 {} 行", rows.len());
}

// ── 验收：非流式 usage_log 字段与上游一致 ──────────────────────────────

#[tokio::test]
async fn acceptance_non_stream_usage_log_matches_upstream() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "用量渠道", "usage", &base, "chat-a");

    let response = chat(&f, "chat-a", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_body_text(response).await;
    assert!(body.contains("\"prompt_tokens\":31"), "上游 usage 应原样透传：{body}");

    let rows = wait_usage_rows(&f.store, 1).await;
    assert_eq!(rows.len(), 1, "一次出站尝试 = 一行");
    let row = &rows[0];

    // 字段与上游一致（02 §8.1 的字段来源表）
    assert_eq!(row.channel_id.as_deref(), Some("c1"));
    assert_eq!(row.account_id.as_deref(), Some("acct_default_c1"));
    assert_eq!(row.model_alias.as_deref(), Some("chat-a"));
    assert_eq!(row.upstream_model.as_deref(), Some("deepseek-chat"));
    assert_eq!(row.protocol.as_deref(), Some("openai"));
    assert_eq!(row.client_key_id.as_deref(), Some("ck1"));
    assert_eq!(row.prompt_tokens, 31);
    assert_eq!(row.completion_tokens, 7);
    assert_eq!(row.cache_read_tokens, 5, "cached_tokens 应落到 cache_read_tokens");
    assert_eq!(row.reasoning_tokens, 3, "reasoning_tokens 应落到 reasoning_tokens");
    assert_eq!(row.cache_write_tokens, 0);
    assert_eq!(row.estimated, 0, "上游给了 usage → 不是估算值");
    assert_eq!(row.status.as_deref(), Some("success"));
    assert_eq!(row.error_kind, None);
    assert_eq!(row.ttft_ms, None, "非流式没有 TTFT");
    assert!(row.duration_ms.is_some(), "应记录出站耗时");
    assert_eq!(
        row.cost_micros, None,
        "该上游模型没配牌价 → 成本为 NULL（未知），不是 0（T13 起按价目表算）"
    );
    assert!(row.ts > 0);
}

// ── 验收：流式 usage_log 字段与上游一致 ────────────────────────────────

#[tokio::test]
async fn acceptance_stream_usage_log_matches_upstream() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "用量渠道", "sseusage", &base, "chat-a");

    let response = chat(&f, "chat-a", true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_body_text(response).await;
    assert!(
        body.contains("\"prompt_tokens\":42"),
        "上游 usage 帧应原样透传：{body}"
    );

    let rows = wait_usage_rows(&f.store, 1).await;
    assert_eq!(rows.len(), 1, "一次出站尝试 = 一行");
    let row = &rows[0];
    assert_eq!(row.channel_id.as_deref(), Some("c1"));
    assert_eq!(row.account_id.as_deref(), Some("acct_default_c1"));
    assert_eq!(row.model_alias.as_deref(), Some("chat-a"));
    assert_eq!(row.upstream_model.as_deref(), Some("deepseek-chat"));
    assert_eq!(row.protocol.as_deref(), Some("openai"));
    assert_eq!(row.prompt_tokens, 42, "tee 应抓到流末的 usage 帧");
    assert_eq!(row.completion_tokens, 9);
    assert_eq!(row.estimated, 0, "上游给了 usage → 不是估算值");
    assert_eq!(row.status.as_deref(), Some("success"));
    assert!(row.ttft_ms.is_some(), "流式应记录 TTFT");
}

// ── 验收：成本换算全链（网关 → usage_log → cost_micros）────────────────

/// T13 验收（03 §2 T13）：**输入 + 输出 + 缓存读** 三价换算。
///
/// 走**真网关**：请求经 mock 上游 → 用量落库 → 按价目表算出 `cost_micros`。
#[tokio::test]
async fn acceptance_cost_micros_uses_three_prices() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "计价渠道", "usage", &base, "chat-a");
    // 给该渠道的模型配一组价：输入 2 元/1M、输出 8 元/1M、缓存命中 0.04 元/1M
    f.store
        .with_conn(|conn| {
            llm_router_lib::store::price::upsert(
                conn,
                &llm_router_lib::store::price::PriceEntry {
                    provider: "deepseek".to_string(),
                    model_pattern: "deepseek-chat".to_string(),
                    input_per_1m: 2_000_000,
                    output_per_1m: 8_000_000,
                    cache_read_per_1m: 40_000,
                },
            )?;
            Ok(())
        })
        .expect("写入价目");

    let response = chat(&f, "chat-a", false).await;
    assert_eq!(response.status(), StatusCode::OK);

    let rows = wait_usage_rows(&f.store, 1).await;
    let row = &rows[0];
    // 上游 usage：prompt 31（**含** 5 个缓存命中）、completion 7
    assert_eq!(row.prompt_tokens, 31);
    assert_eq!(row.completion_tokens, 7);
    assert_eq!(row.cache_read_tokens, 5);
    // 可计费输入 = 31 - 5 = 26
    //   26 × 2 元/1M  = 0.052 元
    //    7 × 8 元/1M  = 0.056 元
    //    5 × 0.04 元/1M = 0.0002 元
    //   合计 0.1082 元 = 108.2 micros → 总额四舍五入 = 108
    assert_eq!(
        row.cost_micros,
        Some(108),
        "成本应按输入/输出/缓存读三价换算"
    );
}

// ── 验收：四维聚合 + 最近请求明细（T14；02 §8.4）────────────────────────

/// 不限时间范围的聚合查询。
fn all_time(dim: Dim) -> AggregateQuery {
    AggregateQuery {
        dim,
        since_ts: None,
        until_ts: None,
        limit: Some(50),
    }
}

/// T14 验收（03 §2 T14 / 03 §1 M2 判据）：**四维聚合 + 最近请求明细**。
///
/// 走**真网关**：流式 + 非流式各一次 → 用量落库 → 聚合结果必须反映真实流量。
#[tokio::test]
async fn acceptance_aggregate_reflects_real_gateway_traffic() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "非流式渠道", "slowusage", &base, "chat-a");
    seed_channel(&f, "c2", "流式渠道", "sseusage", &base, "chat-b");

    // 一次非流式（上游 usage：prompt 31 / completion 7 / cached 5）
    let r1 = chat(&f, "chat-a", false).await;
    assert_eq!(r1.status(), StatusCode::OK);
    // 一次流式（上游 usage 帧：prompt 42 / completion 9，带 TTFT）
    let r2 = chat(&f, "chat-b", true).await;
    assert_eq!(r2.status(), StatusCode::OK);
    // 必须**读完响应体**：tee 在流被消费完时才冲刷末帧的 usage（02 §8.1）
    let _ = read_body_text(r2).await;

    let rows = wait_usage_rows(&f.store, 2).await;
    assert_eq!(rows.len(), 2, "一次出站尝试 = 一行");

    // ── 按渠道聚合 ──
    let by_channel = aggregate(&f.store, &all_time(Dim::Channel)).expect("按渠道聚合");
    assert_eq!(by_channel.buckets.len(), 2, "两个渠道 → 两个桶");

    let c1 = by_channel
        .buckets
        .iter()
        .find(|b| b.key == "c1")
        .expect("应有 c1 桶");
    assert_eq!(c1.label, "非流式渠道", "桶应带上可读的渠道名");
    assert_eq!(c1.requests, 1);
    assert_eq!(c1.prompt_tokens, 31);
    assert_eq!(c1.completion_tokens, 7);
    assert_eq!(c1.cache_read_tokens, 5);
    assert_eq!(c1.total_tokens, 38);
    assert_eq!(c1.success_count, 1);
    assert_eq!(c1.error_count, 0);
    assert_eq!(c1.estimated_rows, 0, "上游给了 usage → 不是估算行");
    assert_eq!(c1.ttft_p50_ms, None, "非流式没有 TTFT");

    let c2 = by_channel
        .buckets
        .iter()
        .find(|b| b.key == "c2")
        .expect("应有 c2 桶");
    assert_eq!(c2.prompt_tokens, 42);
    assert_eq!(c2.completion_tokens, 9);
    assert!(
        c2.ttft_p50_ms.is_some(),
        "流式有 TTFT → 桶里应有分位数"
    );

    // ── 合计（不受 limit 影响）──
    let total = &by_channel.total;
    assert_eq!(total.requests, 2);
    assert_eq!(total.prompt_tokens, 73);
    assert_eq!(total.completion_tokens, 16);
    assert_eq!(total.total_tokens, 89);
    assert_eq!(total.success_count, 2);
    // 该上游模型没配牌价 → 两行成本都是 NULL（未知）→ 已知成本合计 0，
    // 但"成本未知"的行数要单独计数（02 §8.3 的 0 vs NULL 语义）
    assert_eq!(total.cost_micros, 0);
    assert_eq!(total.cost_unknown_rows, 2);

    // ── 按 UTC 日聚合 ──
    let by_day = aggregate(&f.store, &all_time(Dim::Day)).expect("按日聚合");
    assert_eq!(by_day.buckets.len(), 1, "同一天的两次请求 → 一个桶");
    assert_eq!(by_day.buckets[0].requests, 2);
    assert_eq!(by_day.buckets[0].total_tokens, 89);

    // ── 按模型 / 按客户端聚合 ──
    let by_model = aggregate(&f.store, &all_time(Dim::Model)).expect("按模型聚合");
    assert_eq!(by_model.buckets.len(), 1, "两个渠道用的是同一个上游模型");
    assert_eq!(by_model.buckets[0].key, "deepseek-chat");
    assert_eq!(by_model.buckets[0].requests, 2);

    let by_client = aggregate(&f.store, &all_time(Dim::Client)).expect("按客户端聚合");
    assert_eq!(by_client.buckets.len(), 1);
    assert_eq!(by_client.buckets[0].key, "ck1");
    assert_eq!(by_client.buckets[0].label, "t7", "桶应带上 client key 标签");
    assert_eq!(by_client.buckets[0].requests, 2);

    // ── 最近请求明细（M2 判据要显示的四项）──
    let recent = recent_requests(&f.store, 10).expect("读明细");
    assert_eq!(recent.len(), 2);
    assert!(recent[0].ts >= recent[1].ts, "新的在前");
    for item in &recent {
        assert_eq!(item.status.as_deref(), Some("success"));
        assert!(item.total_tokens > 0);
        assert_eq!(item.estimated, false, "上游给了 usage → 不是估算行");
        assert_eq!(item.cost_micros, None, "没配牌价 → 成本未知（NULL）");
        // 输出速度必须**按该行自己的耗时复算**（tokens ÷ 秒）
        let duration = item.duration_ms.expect("成功行应记录耗时");
        assert!(duration > 0, "mock 有意延迟，耗时应为正");
        let tps = item.output_tps.expect("成功且有耗时的行应能算出 tokens·s⁻¹");
        let expected = item.completion_tokens as f64 * 1000.0 / duration as f64;
        assert!(
            (tps - expected).abs() < 1e-6,
            "tokens·s⁻¹ 应等于 输出 token ÷ 耗时秒：期望 {expected}，实际 {tps}"
        );
    }
}

// ── 验收：429 后 quota_json 更新 ───────────────────────────────────────

#[tokio::test]
async fn acceptance_rate_limit_updates_quota_json() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "限流渠道", "fail429", &base, "chat-a");

    let response = chat(&f, "chat-a", false).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    // 限额自学习：上游报的上限写进 account.quota_json（02 §8.2）
    let state = read_quota(&f.store, "acct_default_c1").expect("读取限额状态");
    assert_eq!(state.rpm, Some(60), "x-ratelimit-limit-requests 应学成 RPM 上限");
    assert_eq!(state.tpm, Some(120_000), "x-ratelimit-limit-tokens 应学成 TPM 上限");
    assert_eq!(state.source.as_deref(), Some("upstream_headers"));
    let fact = state.last_rate_limit.expect("应记下限流事实");
    assert_eq!(fact.retry_after_secs, Some(5));
    assert_eq!(fact.remaining_requests, Some(0));

    // 同时记一条失败用量（失败的调用没有产出 → 不估 token）
    let rows = wait_usage_rows(&f.store, 1).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.status.as_deref(), Some("error"));
    assert_eq!(row.error_kind.as_deref(), Some("rate_limit_error"));
    assert_eq!(row.prompt_tokens, 0);
    assert_eq!(row.completion_tokens, 0);
    assert_eq!(row.estimated, 0);
}

// ── 补充：上游没回 usage → 走估算入口（T12：tiktoken 路径）─────────────

/// 估算用例用的提示词：含 emoji——字符启发式与 tiktoken 的结论明显不同，
/// 这样"落库的数确实来自 tiktoken"才测得出来。
const ESTIMATE_PROMPT: &str = "🌈估算🌈路径🌈验证🌈token🌈";

#[tokio::test]
async fn acceptance_success_without_upstream_usage_is_estimated() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "用量渠道", "nousage", &base, "chat-a");

    let response = chat_with_prompt(&f, "chat-a", false, ESTIMATE_PROMPT).await;
    assert_eq!(response.status(), StatusCode::OK);

    let rows = wait_usage_rows(&f.store, 1).await;
    let row = &rows[0];
    assert_eq!(row.estimated, 1, "上游没回 usage → 应标估算");
    assert!(row.prompt_tokens > 0, "输入应被估算");
    assert!(row.completion_tokens > 0, "输出应被估算");
    assert_eq!(row.status.as_deref(), Some("success"));

    // 落库的数必须**等于 tiktoken 的计数**（上游模型名走 cl100k_base）
    let completion = "你好，世界";
    let tik = TiktokenEstimator
        .estimate("deepseek-chat", ESTIMATE_PROMPT, completion)
        .expect("tiktoken 估算");
    let chr = CharRatioEstimator
        .estimate("deepseek-chat", ESTIMATE_PROMPT, completion)
        .expect("字符启发式估算");
    assert_ne!(
        tik.prompt_tokens, chr.prompt_tokens,
        "用例必须能区分 tiktoken 与字符启发式，否则测不出换了实现"
    );
    assert_eq!(
        row.prompt_tokens, tik.prompt_tokens,
        "输入 token 应等于 tiktoken 计数"
    );
    assert_eq!(
        row.completion_tokens, tik.completion_tokens,
        "输出 token 应等于 tiktoken 计数"
    );
}

// ── 补充：上游 usage 缺一项 → 只补缺的那一项（02 §8.1）──────────────────

#[tokio::test]
async fn acceptance_partial_upstream_usage_estimates_only_missing_side() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "用量渠道", "partialusage", &base, "chat-a");

    let response = chat(&f, "chat-a", false).await;
    assert_eq!(response.status(), StatusCode::OK);

    let rows = wait_usage_rows(&f.store, 1).await;
    let row = &rows[0];
    assert_eq!(
        row.prompt_tokens, 31,
        "上游给了的输入 token 不该被估算覆盖（R7）"
    );
    assert!(
        row.completion_tokens > 0,
        "缺失的输出 token 应由估算器补"
    );
    assert_eq!(
        row.estimated, 1,
        "含估算值的行必须标估算，不能冒充精确值"
    );
    assert_eq!(row.status.as_deref(), Some("success"));
}

// ── 补充：失败尝试记 error 行且不估 token ─────────────────────────────

#[tokio::test]
async fn acceptance_failed_attempt_is_logged_without_tokens() {
    let base = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "用量渠道", "fail500", &base, "chat-a");

    let response = chat(&f, "chat-a", false).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

    let rows = wait_usage_rows(&f.store, 1).await;
    let row = &rows[0];
    assert_eq!(row.status.as_deref(), Some("error"));
    assert_eq!(row.error_kind.as_deref(), Some("upstream_error"));
    assert_eq!(row.prompt_tokens, 0);
    assert_eq!(row.completion_tokens, 0);
    assert_eq!(row.estimated, 0, "失败行不做估算");
}
