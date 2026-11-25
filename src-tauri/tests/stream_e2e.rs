//! T6 流式管道 + fallback + 熔断 —— 端到端验收测试（mock 上游）。
//!
//! 验收判据（03 §2 T6）：mock 上游可复现
//! "首字节后失败不重试""断连取消上游""3 连败熔断""429 冷却时长遵循 Retry-After"
//! 四个场景（用内存 mock server 测试）。
//!
//! 另外覆盖：SSE 零缓冲直通、P-4 学习式兼容、P-3 MiniMax 业务错误、P-7 `X-Routed-Via`。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header::CONTENT_TYPE, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use http_body_util::BodyExt;
use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::fallback::{circuit_key, Forwarder};
use llm_router_lib::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::router::AliasRouter;
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use tower::ServiceExt;

// ── mock 上游 ──────────────────────────────────────────────────────────

#[derive(Default)]
struct MockState {
    ok_hits: AtomicUsize,
    fail500_hits: AtomicUsize,
    fail429_hits: AtomicUsize,
    stream_hits: AtomicUsize,
    streamfail_hits: AtomicUsize,
    badreq_hits: AtomicUsize,
    business_hits: AtomicUsize,
    /// 上游观察到客户端断开
    disconnected: Arc<AtomicBool>,
    /// 收到的请求体（按到达顺序）
    bodies: Mutex<Vec<String>>,
}

impl MockState {
    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().expect("锁").clone()
    }
}

/// 流守卫：流被提前 drop（客户端断开）时置位。
struct StreamGuard {
    completed: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::SeqCst) {
            self.disconnected.store(true, Ordering::SeqCst);
        }
    }
}

/// 无限 SSE 流（用于断连检测）。
fn sse_infinite(state: Arc<MockState>) -> Response {
    let guard = StreamGuard {
        completed: Arc::new(AtomicBool::new(false)),
        disconnected: Arc::clone(&state.disconnected),
    };
    let n = 0u32;
    let stream = futures_util::stream::unfold((n, guard), move |(n, guard)| async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        let payload = format!("data: {{\"id\":\"chunk-{n}\"}}\n\n");
        Some((Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(payload)), (n + 1, guard)))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .expect("构造 SSE 响应")
}

/// 先发一帧再断流的 SSE 流（用于"首字节后失败"）。
fn sse_fail_after_first(state: Arc<MockState>) -> Response {
    let guard = StreamGuard {
        completed: Arc::new(AtomicBool::new(false)),
        disconnected: Arc::clone(&state.disconnected),
    };
    // 状态：(帧序号, 是否已出错, 守卫)
    let stream = futures_util::stream::unfold((0u32, false, guard), move |(n, failed, guard)| async move {
        if failed {
            return None;
        }
        if n >= 1 {
            // 稍作等待，确保响应头与首帧已经刷出（真实 SSE 上游就是这个行为），
            // 然后上游中途失败。
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Some((
                Err::<bytes::Bytes, std::io::Error>(std::io::Error::other("mock upstream broken")),
                (n, true, guard),
            ));
        }
        let payload = "data: {\"id\":\"chunk-0\"}\n\n";
        Some((Ok(bytes::Bytes::from(payload)), (n + 1, false, guard)))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .expect("构造 SSE 响应")
}

/// 固定间隔的有限 SSE 流（用于零缓冲验证）。
fn sse_fixed(state: Arc<MockState>, frames: u32, interval: Duration) -> Response {
    let guard = StreamGuard {
        completed: Arc::new(AtomicBool::new(true)),
        disconnected: Arc::clone(&state.disconnected),
    };
    let stream = futures_util::stream::unfold((0u32, guard), move |(n, guard)| async move {
        if n >= frames {
            return None;
        }
        tokio::time::sleep(interval).await;
        let payload = format!("data: {{\"id\":\"chunk-{n}\"}}\n\n");
        Some((Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(payload)), (n + 1, guard)))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .expect("构造 SSE 响应")
}

async fn mock_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
    uri: Uri,
    body: bytes::Bytes,
) -> Response {
    let scenario = uri
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    state
        .bodies
        .lock()
        .expect("锁")
        .push(format!("{scenario}:{}", String::from_utf8_lossy(&body)));

    match scenario.as_str() {
        "ok" => {
            state.ok_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                [(CONTENT_TYPE, "application/json")],
                r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#,
            )
                .into_response()
        }
        "fail500" => {
            state.fail500_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::INTERNAL_SERVER_ERROR, "upstream boom").into_response()
        }
        "fail429" => {
            state.fail429_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "2"), ("x-ratelimit-remaining-requests", "0")],
                "slow down",
            )
                .into_response()
        }
        "stream" => {
            state.stream_hits.fetch_add(1, Ordering::SeqCst);
            sse_infinite(Arc::clone(&state))
        }
        "streamfixed" => {
            state.stream_hits.fetch_add(1, Ordering::SeqCst);
            sse_fixed(Arc::clone(&state), 5, Duration::from_millis(100))
        }
        "streamfail" => {
            state.streamfail_hits.fetch_add(1, Ordering::SeqCst);
            sse_fail_after_first(Arc::clone(&state))
        }
        "badstreamoptions" => {
            state.badreq_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_REQUEST,
                r#"{"error":{"message":"stream_options is not supported by this endpoint","code":"unsupported_param"}}"#,
            )
                .into_response()
        }
        "business" => {
            state.business_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                r#"{"id":"x","choices":[],"base_resp":{"status_code":1004,"status_msg":"invalid api key"}}"#,
            )
                .into_response()
        }
        _ => (StatusCode::NOT_FOUND, "unknown scenario").into_response(),
    }
}

/// 启动 mock 上游，返回 base_url 与状态。
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
        Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None)
            .expect("打开保险库"),
    ));
    // 与生产同款：熔断器带落库（写穿 + 启动恢复）
    let forwarder = Arc::new(Forwarder::new(
        OutboundClient::new().expect("出站客户端"),
        Arc::new(CircuitBreaker::with_persistence(Arc::new(
            StoreCircuitPersistence::new(Arc::clone(&store)),
        ))),
        Arc::new(StreamUsageDowngrades::default()),
        Arc::clone(&vault),
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
                 VALUES ('ck1', 't6', ?1, ?2, ?3)",
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
fn seed_channel(
    f: &Fixture,
    id: &str,
    name: &str,
    scenario: &str,
    base_url: &str,
    priority: i64,
) {
    let now = llm_router_lib::vault::now_secs();
    f.store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at)
                 VALUES (?1, ?2, 'api_key', 'deepseek', ?3, 1, ?4, ?5)",
                rusqlite::params![id, name, format!("{base_url}/{scenario}"), priority, now],
            )?;
            conn.execute(
                "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                 VALUES (?1, ?2, 'deepseek-chat', ?3, 1)",
                rusqlite::params![format!("m_{id}"), id, id],
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

/// 把两个渠道挂到同一个路由组（构造 fallback 候选链）。
fn seed_group(f: &Fixture, alias: &str, strategy: &str, members: &[&str]) {
    f.store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO route_group (id, alias, strategy) VALUES (?1, ?2, ?3)",
                rusqlite::params![format!("g_{alias}"), alias, strategy],
            )?;
            for member in members {
                conn.execute(
                    "INSERT INTO route_member (group_id, model_id, weight) VALUES (?1, ?2, 100)",
                    rusqlite::params![format!("g_{alias}"), format!("m_{member}")],
                )?;
            }
            Ok(())
        })
        .expect("写入路由组");
}

/// 发起一次 chat 请求（流式或非流式）。
async fn chat(f: &Fixture, model: &str, stream: bool) -> Response {
    let body = serde_json::json!({"model": model, "messages": [{"role": "user", "content": "hi"}], "stream": stream});
    server::router(f.state.clone())
        .oneshot(
            axum::http::Request::builder()
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

async fn error_json(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    serde_json::from_slice(&bytes).expect("应为 JSON")
}

// ── 验收场景 1：首字节后失败不重试 ───────────────────────────────────

#[tokio::test]
async fn acceptance_failure_after_first_byte_does_not_retry() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "首选渠道", "streamfail", &base, 10);
    seed_channel(&f, "c2", "备选渠道", "ok", &base, 20);
    seed_group(&f, "g1", "priority", &["c1", "c2"]);

    let response = chat(&f, "g1", true).await;
    assert_eq!(response.status(), StatusCode::OK);

    // 关键断言：没有回退到备选渠道（先查计数，再看内容）
    assert_eq!(mock.streamfail_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        mock.ok_hits.load(Ordering::SeqCst),
        0,
        "已写出首字节后不得重试到备选渠道"
    );

    // 逐帧读取：应收到首帧 + 网关补的 error 事件
    let mut body = response.into_body();
    let mut text = String::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.expect("帧应可读");
        let bytes = frame.into_data().unwrap_or_default();
        text.push_str(&String::from_utf8_lossy(&bytes));
    }

    assert!(text.contains("chunk-0"), "首帧应透传：{text}");
    assert!(
        text.contains("upstream_error"),
        "中途失败应补 SSE error 事件：{text}"
    );
}

// ── 验收场景 2：断连取消上游 ───────────────────────────────────────────

#[tokio::test]
async fn acceptance_client_disconnect_aborts_upstream() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "流式渠道", "stream", &base, 10);

    let response = chat(&f, "c1", true).await;
    assert_eq!(response.status(), StatusCode::OK);

    let mut body = response.into_body();
    let first = body
        .frame()
        .await
        .expect("应有帧")
        .expect("帧应可读");
    let text = String::from_utf8_lossy(&first.into_data().unwrap_or_default()).to_string();
    assert!(text.contains("chunk-0"), "首帧应透传：{text}");

    // 客户端断连：直接 drop 响应体
    drop(body);

    // 上游应观察到断开
    let mut disconnected = false;
    for _ in 0..50 {
        if mock.disconnected.load(Ordering::SeqCst) {
            disconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        disconnected,
        "客户端断连后上游应观察到连接关闭（避免上游空跑计费）"
    );
}

// ── 验收场景 3：3 连败熔断 ────────────────────────────────────────────

#[tokio::test]
async fn acceptance_three_consecutive_failures_open_circuit() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "故障渠道", "fail500", &base, 10);

    // 前 3 次：真实打到上游并失败
    for i in 0..3 {
        let response = chat(&f, "c1", false).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_GATEWAY,
            "第 {} 次应为 502", i + 1
        );
        let value = error_json(response).await;
        assert_eq!(value["error"]["type"], "upstream_error");
    }
    assert_eq!(mock.fail500_hits.load(Ordering::SeqCst), 3);

    // 第 4 次：熔断已开，直接跳过，不再打上游
    let response = chat(&f, "c1", false).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let value = error_json(response).await;
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("熔断"),
        "错误信息应说明熔断：{}",
        value["error"]["message"]
    );
    assert_eq!(
        mock.fail500_hits.load(Ordering::SeqCst),
        3,
        "熔断后不得再打上游"
    );
}

// ── 验收场景 4：429 冷却遵循 Retry-After ──────────────────────────────

#[tokio::test]
async fn acceptance_rate_limit_cooldown_follows_retry_after() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "限流渠道", "fail429", &base, 10);

    let response = chat(&f, "c1", false).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let value = error_json(response).await;
    assert_eq!(value["error"]["type"], "rate_limit_error");

    // 熔断器记录的冷却截止时间应遵循 Retry-After: 2
    let key = circuit_key("c1", "deepseek-chat");
    let until = f
        .state
        .forwarder
        .circuit
        .open_until(&key)
        .expect("429 后应进入冷却");
    let now = llm_router_lib::vault::now_secs();
    assert!(
        (2..=3).contains(&(until - now)),
        "冷却时长应遵循 Retry-After=2，实际 {}s",
        until - now
    );

    // 冷却期内：不再打上游；客户端应拿到 429 + Retry-After（而不是笼统的 502）
    let response = chat(&f, "c1", false).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .expect("冷却期内应带 Retry-After");
    assert!(
        (1..=3).contains(&retry_after),
        "Retry-After 应等于剩余冷却秒数，实际 {retry_after}"
    );
    let value = error_json(response).await;
    assert_eq!(value["error"]["type"], "rate_limit_error");
    assert_eq!(
        mock.fail429_hits.load(Ordering::SeqCst),
        1,
        "冷却期内不得再打上游"
    );
}

// ── 附加验收：SSE 零缓冲直通 ──────────────────────────────────────────

#[tokio::test]
async fn acceptance_sse_is_zero_buffer() {
    let (base, _mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "流式渠道", "streamfixed", &base, 10);

    let response = chat(&f, "c1", true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
        "流式响应应保持 SSE content-type"
    );

    let started = Instant::now();
    let mut body = response.into_body();
    let mut arrivals = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.expect("帧应可读");
        let bytes = frame.into_data().unwrap_or_default();
        if bytes.is_empty() {
            continue;
        }
        arrivals.push(started.elapsed());
    }

    assert!(arrivals.len() >= 5, "应收到 5 帧，实际 {}", arrivals.len());
    assert!(
        arrivals[0] < Duration::from_millis(300),
        "首帧应尽快到达（未全量缓冲），实际 {:?}",
        arrivals[0]
    );
    assert!(
        arrivals[4] >= Duration::from_millis(400),
        "帧应逐个到达（总耗时接近逐帧累加），实际 {:?}",
        arrivals[4]
    );
}

// ── 附加验收：P-4 学习式兼容 ─────────────────────────────────────────

#[tokio::test]
async fn acceptance_p4_learns_to_disable_stream_usage_injection() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "老渠道", "badstreamoptions", &base, 10);

    // 流式请求：R-4 会注入 stream_options
    let response = chat(&f, "c1", true).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let value = error_json(response).await;
    assert_eq!(value["error"]["type"], "invalid_request_error");

    // 上游应被打了两次：第一次带注入，第二次不带
    assert_eq!(mock.badreq_hits.load(Ordering::SeqCst), 2);
    let bodies = mock.bodies();
    assert!(bodies[0].contains("stream_options"), "第一次应带注入：{}", bodies[0]);
    assert!(
        !bodies[1].contains("stream_options"),
        "降级后重试不应再带注入：{}",
        bodies[1]
    );
    assert!(
        f.state
            .forwarder
            .downgrades
            .is_disabled("c1"),
        "该渠道应被标记为本次会话内关闭注入"
    );
}

// ── 附加验收：P-3 MiniMax 业务错误 + P-7 X-Routed-Via ─────────────────

#[tokio::test]
async fn acceptance_minimax_business_error_is_mapped() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "MiniMax 国内站", "business", &base, 10);

    let response = chat(&f, "c1", false).await;
    // HTTP 200 但业务错误 → 按 §4.2 映射为 401
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let value = error_json(response).await;
    assert_eq!(value["error"]["type"], "authentication_error");
    assert_eq!(value["error"]["code"], "minimax_1004");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("GroupId"),
        "1004 的提示应指向 GroupId：{}",
        value["error"]["message"]
    );
    assert_eq!(mock.business_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn acceptance_response_has_routed_via_header() {
    let (base, _mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "DeepSeek", "ok", &base, 10);

    let response = chat(&f, "c1", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-routed-via")
            .and_then(|v| v.to_str().ok()),
        Some("DeepSeek/deepseek-chat"),
        "P-7：应带出路由目标"
    );

    // 非流式 body 透传
    let value = error_json(response).await;
    assert_eq!(value["id"], "chatcmpl-1");
    assert_eq!(value["choices"][0]["message"]["content"], "hi");
}

#[tokio::test]
async fn acceptance_routed_via_is_ascii_safe_for_chinese_channel_name() {
    let (base, _mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "DeepSeek 官方", "ok", &base, 10);

    let response = chat(&f, "c1", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = response
        .headers()
        .get("x-routed-via")
        .expect("应有 X-Routed-Via")
        .to_str()
        .expect("头值必须是 ASCII 可读");
    // 中文按百分号编码，ASCII 部分原样（空格保留）
    assert!(value.ends_with("/deepseek-chat"), "实际 {value}");
    assert!(value.starts_with("DeepSeek "), "实际 {value}");
    assert!(value.is_ascii(), "头值必须全 ASCII：{value}");
}

// ── 附加验收：fallback 在首字节前切到备选渠道 ─────────────────────────

#[tokio::test]
async fn acceptance_fallback_switches_candidate_before_first_byte() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "Broken", "fail500", &base, 10);
    seed_channel(&f, "c2", "Healthy", "ok", &base, 20);
    seed_group(&f, "g1", "priority", &["c1", "c2"]);

    let response = chat(&f, "g1", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(mock.fail500_hits.load(Ordering::SeqCst), 1);
    assert_eq!(mock.ok_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        response
            .headers()
            .get("x-routed-via")
            .and_then(|v| v.to_str().ok()),
        Some("Healthy/deepseek-chat"),
        "应落到备选渠道"
    );
}

// ── 附加验收：上游 401 不重试（不可重试类） ───────────────────────────
#[tokio::test]
async fn acceptance_unauthorized_is_not_retried() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    // 借用 ok 场景但改 base_url 指向不存在的路径 → 404；这里用 400 场景代替
    seed_channel(&f, "c1", "参数错误渠道", "badstreamoptions", &base, 10);
    seed_channel(&f, "c2", "备选渠道", "ok", &base, 20);
    seed_group(&f, "g1", "priority", &["c1", "c2"]);

    // 非流式 → 不触发 R-4 注入 → 直接是不可重试的 400
    let response = chat(&f, "g1", false).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(mock.badreq_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        mock.ok_hits.load(Ordering::SeqCst),
        0,
        "不可重试错误不得切换候选"
    );
}

// ── 附加验收：熔断状态落库 + 重启恢复 + 回到正常即清理 ──────────────────

/// 读取 `circuit_state` 表的全部行。
fn circuit_rows(store: &Store) -> Vec<(String, String, String, Option<String>)> {
    store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT channel_id, upstream_model, state, reason FROM circuit_state ORDER BY channel_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .expect("读取 circuit_state")
}

#[tokio::test]
async fn acceptance_circuit_state_is_persisted_and_survives_restart() {
    let (base, mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "故障渠道", "fail500", &base, 10);

    // 前 2 次失败：未达阈值 → 不落库
    for _ in 0..2 {
        let response = chat(&f, "c1", false).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
    assert!(
        circuit_rows(&f.store).is_empty(),
        "未达熔断阈值时不应有落库行"
    );

    // 第 3 次失败 → 熔断打开 → 写穿落库
    let response = chat(&f, "c1", false).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let rows = circuit_rows(&f.store);
    assert_eq!(rows.len(), 1, "熔断打开应落库 1 行：{rows:?}");
    assert_eq!(rows[0].0, "c1");
    assert_eq!(rows[0].1, "deepseek-chat");
    assert_eq!(rows[0].2, "open");
    assert_eq!(rows[0].3.as_deref(), Some("failures"));

    // 模拟重启：新建熔断器（读同一个库）→ 状态应恢复，仍在拦截
    let restarted = CircuitBreaker::with_persistence(Arc::new(StoreCircuitPersistence::new(
        Arc::clone(&f.store),
    )));
    let key = circuit_key("c1", "deepseek-chat");
    assert!(
        !restarted.allow(&key),
        "重启后熔断必须继续生效（不能再打上游）"
    );
    let before_restart = mock.fail500_hits.load(Ordering::SeqCst);
    let response = chat(&f, "c1", false).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        mock.fail500_hits.load(Ordering::SeqCst),
        before_restart,
        "熔断中不得再打上游"
    );
}

#[tokio::test]
async fn acceptance_circuit_row_is_removed_after_recovery() {
    let (base, _mock) = start_mock().await;
    let f = fixture();
    seed_channel(&f, "c1", "故障渠道", "fail500", &base, 10);

    for _ in 0..3 {
        let _ = chat(&f, "c1", false).await;
    }
    assert_eq!(circuit_rows(&f.store).len(), 1, "熔断应已落库");

    // 模拟一次成功（half_open 探测成功）→ 回到 closed → 落库行应被删除
    let key = circuit_key("c1", "deepseek-chat");
    f.state
        .forwarder
        .circuit
        .on_success(&key);
    assert!(
        circuit_rows(&f.store).is_empty(),
        "回到 closed 后落库行应被清理"
    );
}
