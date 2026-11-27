//! T21 Copilot 出站适配 —— 端到端验收测试。
//!
//! 验收判据（03 §2 T21）：
//! ① R-6 四头**逐头断言值** + `x-request-id` 每次都不同；
//! ② 出站 URL 与 Bearer（**是 Copilot token，不是 GitHub token**）；
//! ③ **token 过期后自动续**；
//! ④ 模型目录解析与落库（别名冲突跳过 / 旧模型禁用）；
//! ⑤ **经网关流式对话成功**。
//!
//! **全程打本地 mock 上游，零出网**（OAuth 三端点 + `/models` + `/chat/completions`
//! 都在同一个 mock 上，路径与真实 GitHub 一致）。

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header::CONTENT_TYPE, HeaderMap, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::fallback::Forwarder;
use llm_router_lib::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::router::AliasRouter;
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::gateway::usage::CharRatioEstimator;
use llm_router_lib::providers::copilot::CopilotSession;
use llm_router_lib::providers::oauth::copilot::{self, Endpoints, TokenCache};
use llm_router_lib::providers::AdapterDeps;
use llm_router_lib::store::channels::{ensure_oauth_channel, OAUTH_PROVIDER};
use llm_router_lib::store::usage::StoreUsageSink;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use tower::ServiceExt;

// ── mock 上游 ──────────────────────────────────────────────────────────

/// mock 上游的状态。
struct MockState {
    /// mock 自己的 base（④ 的 `endpoints.api` 要指向它）。
    base: String,
    /// ④ 被调用的次数。
    token_calls: AtomicUsize,
    /// `/models` 被调用的次数。
    models_calls: AtomicUsize,
    /// `/chat/completions` 被调用的次数。
    chat_calls: AtomicUsize,
    /// ④ 发出的 token 有效期（秒）——**调小即可模拟"快过期"**。
    token_ttl: AtomicI64,
    /// `/models` 请求收到的头。
    models_headers: Mutex<Vec<(String, String)>>,
    /// `/chat/completions` 请求收到的头。
    chat_headers: Mutex<Vec<(String, String)>>,
    /// `/chat/completions` 请求的路径。
    chat_path: Mutex<String>,
    /// `/chat/completions` 请求的 body。
    chat_body: Mutex<String>,
}

impl MockState {
    fn new(base: String) -> Self {
        MockState {
            base,
            token_calls: AtomicUsize::new(0),
            models_calls: AtomicUsize::new(0),
            chat_calls: AtomicUsize::new(0),
            token_ttl: AtomicI64::new(1800),
            models_headers: Mutex::new(Vec::new()),
            chat_headers: Mutex::new(Vec::new()),
            chat_path: Mutex::new(String::new()),
            chat_body: Mutex::new(String::new()),
        }
    }

    fn header(&self, name: &str) -> Option<String> {
        self.chat_headers
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }
}

/// ④ 换 Copilot token（**每次给不同的 token**——便于断言"续期后换了新 token"）。
async fn copilot_token(State(state): State<Arc<MockState>>) -> Response {
    let seq = state.token_calls.fetch_add(1, Ordering::SeqCst) + 1;
    let ttl = state.token_ttl.load(Ordering::SeqCst);
    let expires_at = llm_router_lib::vault::now_secs() + ttl;
    let body = serde_json::json!({
        "token": format!("MOCK-COPILOT-TOKEN-{seq}"),
        "expires_at": expires_at as f64,
        "endpoints": { "api": state.base }
    });
    (StatusCode::OK, axum::Json(body)).into_response()
}

/// 模型目录。
async fn models(State(state): State<Arc<MockState>>, headers: HeaderMap) -> Response {
    state.models_calls.fetch_add(1, Ordering::SeqCst);
    *state.models_headers.lock().unwrap() = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "gpt-5-mini",
                "name": "GPT-5 mini",
                "object": "model",
                "policy": { "state": "enabled" },
                "supported_endpoints": ["/chat/completions"],
                "vendor": "Azure OpenAI",
                "capabilities": {
                    "family": "gpt-5-mini",
                    "type": "chat",
                    "limits": { "max_context_window_tokens": 264000, "max_output_tokens": 64000 },
                    "supports": { "tool_calls": true, "streaming": true, "vision": false }
                }
            },
            {
                "id": "gpt-5.1-codex",
                "name": "GPT-5.1 Codex",
                "policy": { "state": "enabled" },
                "supported_endpoints": ["/responses"],
                "capabilities": { "type": "chat" }
            },
            {
                "id": "text-embedding-3-small",
                "name": "Embedding V3 small",
                "capabilities": { "type": "embeddings" }
            },
            {
                "id": "blocked-model",
                "name": "Blocked",
                "policy": { "state": "disabled" }
            }
        ]
    });
    (StatusCode::OK, axum::Json(body)).into_response()
}

/// 出站 chat（回一个 SSE 流）。
async fn chat_completions(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    uri: Uri,
    body: String,
) -> Response {
    state.chat_calls.fetch_add(1, Ordering::SeqCst);
    *state.chat_headers.lock().unwrap() = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    *state.chat_path.lock().unwrap() = uri.path().to_string();
    *state.chat_body.lock().unwrap() = body;
    let sse = concat!(
        "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"你好\"}}]}\n\n",
        "data: [DONE]\n\n"
    );
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/event-stream")],
        sse,
    )
        .into_response()
}

/// 起本地 mock，返回 (base_url, 状态)。
async fn start_mock() -> (String, Arc<MockState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定 mock 端口");
    let addr = listener.local_addr().expect("读取 mock 地址");
    let base = format!("http://{addr}");
    let state = Arc::new(MockState::new(base.clone()));
    let app = Router::new()
        .route("/copilot_internal/v2/token", get(copilot_token))
        .route("/models", get(models))
        .route("/chat/completions", post(chat_completions))
        .with_state(Arc::clone(&state));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (base, state)
}

// ── 网关夹具 ───────────────────────────────────────────────────────────

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    vault: Arc<RwLock<Vault>>,
    state: GatewayState,
    app_state: llm_router_lib::AppState,
    session: CopilotSession,
    key: String,
}

fn fixture(mock_base: &str) -> Fixture {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault = Arc::new(RwLock::new(
        Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
    ));
    // **测试专用会话**：OAuth 端点指向本地 mock + 独立缓存（不出网、互不污染）
    let session = CopilotSession::testing(
        Arc::clone(&vault),
        Endpoints::local(mock_base),
        Arc::new(TokenCache::new()),
    );
    let forwarder = Arc::new(
        Forwarder::new(
            OutboundClient::new().expect("出站客户端"),
            Arc::new(CircuitBreaker::new()),
            Arc::new(StreamUsageDowngrades::default()),
            Arc::clone(&vault),
            Arc::clone(&store),
            Arc::new(StoreUsageSink::new(Arc::clone(&store))),
            Arc::new(CharRatioEstimator),
        )
        .with_adapter_deps(AdapterDeps {
            copilot: session.clone(),
        }),
    );
    let state = GatewayState {
        router: Arc::new(AliasRouter::new(Arc::clone(&store))),
        forwarder: Arc::clone(&forwarder),
        store: Arc::clone(&store),
        limiter: Arc::new(RateLimiter::new()),
    };
    let app_state = llm_router_lib::AppState {
        gateway_addr: "127.0.0.1:8787".to_string(),
        gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
        store: Arc::clone(&store),
        vault: Arc::clone(&vault),
        limiter: Arc::new(RateLimiter::new()),
        circuit: Arc::new(CircuitBreaker::new()),
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
                 VALUES ('ck1', 't21', ?1, ?2, ?3)",
                rusqlite::params![hash, prefix, now],
            )?;
            Ok(())
        })
        .expect("写入 client key");

    Fixture {
        _dir: dir,
        store,
        vault,
        state,
        app_state,
        session,
        key,
    }
}

/// 种一个 Copilot 渠道（**默认关闭** → 这里手工打开，启用开关属 T24）+ GitHub token。
fn seed_copilot_channel(f: &Fixture, mock_base: &str) -> String {
    let channel_id = ensure_oauth_channel(
        &f.store,
        "GitHub Copilot",
        OAUTH_PROVIDER,
        mock_base,
    )
    .expect("建 Copilot 渠道行");
    // **启用开关属 T24**（ToS 弹窗）→ 本卡不提供产品入口，这里直接改库把渠道打开
    f.store
        .with_conn(|conn| {
            conn.execute("UPDATE channel SET enabled = 1 WHERE id = ?1", [&channel_id])?;
            Ok(())
        })
        .expect("启用渠道");
    {
        let guard = f.vault.read().unwrap_or_else(|e| e.into_inner());
        copilot::store_github_token(&guard, &channel_id, "MOCK-GH-TOKEN").expect("存 GitHub token");
    }
    channel_id
}

/// 给渠道加一个模型（别名 → 上游模型名）。
fn seed_model(f: &Fixture, channel_id: &str, alias: &str, upstream: &str) {
    f.store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                rusqlite::params![format!("m_{alias}"), channel_id, upstream, alias],
            )?;
            Ok(())
        })
        .expect("写入模型");
}

/// 发起一次 chat 请求（流式或非流式）。
async fn chat(f: &Fixture, model: &str, stream: bool) -> Response {
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "hi"}],
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

async fn body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    String::from_utf8_lossy(&bytes).to_string()
}

// ── 判据 ①②⑤：出站头 / URL / Bearer + 经网关流式对话 ──────────────────

#[tokio::test]
async fn acceptance_the_gateway_forwards_to_copilot_with_the_r6_headers() {
    let (base, mock) = start_mock().await;
    let f = fixture(&base);
    let channel_id = seed_copilot_channel(&f, &base);
    seed_model(&f, &channel_id, "gpt-5-mini", "gpt-5-mini");

    let response = chat(&f, "gpt-5-mini", true).await;
    assert_eq!(response.status(), StatusCode::OK);

    // 判据⑤：上游被打到一次
    assert_eq!(mock.chat_calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.token_calls.load(Ordering::SeqCst), 1);

    // 判据②：出站 URL = `{endpoints.api}/chat/completions`（**不带 `/v1`**）
    assert_eq!(mock.chat_path.lock().unwrap().as_str(), "/chat/completions");

    // 判据②：Bearer = **Copilot token**（**不是** GitHub token）
    let auth = mock.header("authorization").expect("应有 Authorization");
    assert_eq!(auth, "Bearer MOCK-COPILOT-TOKEN-1");
    assert!(
        !auth.contains("MOCK-GH-TOKEN"),
        "绝不能把 GitHub token 当 Bearer 发出去：{auth}"
    );

    // 判据①：R-6 四头**逐头**断言值
    assert_eq!(
        mock.header("copilot-integration-id").as_deref(),
        Some("vscode-chat")
    );
    assert_eq!(
        mock.header("editor-version").as_deref(),
        Some("vscode/1.104.1")
    );
    assert_eq!(
        mock.header("editor-plugin-version").as_deref(),
        Some("copilot-chat/0.26.7")
    );
    assert_eq!(
        mock.header("user-agent").as_deref(),
        Some("GitHubCopilotChat/0.26.7")
    );

    // 判据①：`x-request-id` 在（UUID v4 形状）
    let request_id = mock.header("x-request-id").expect("应有 x-request-id");
    assert_eq!(request_id.len(), 36, "x-request-id 应是 UUID：{request_id}");
    assert_eq!(request_id.split('-').count(), 5);

    // R-1：模型名换成上游真名；R-4：流式注入 include_usage
    let sent = mock.chat_body.lock().unwrap().clone();
    let sent: serde_json::Value = serde_json::from_str(&sent).expect("出站 body 是 JSON");
    assert_eq!(sent["model"], "gpt-5-mini");
    assert_eq!(sent["stream_options"]["include_usage"], serde_json::json!(true));

    // 判据⑤：流式内容**透传**回客户端
    let text = body_text(response).await;
    assert!(text.contains("你好"), "流式内容应透传：{text}");
    assert!(text.contains("[DONE]"), "结束帧应透传：{text}");
}

#[tokio::test]
async fn acceptance_the_request_id_changes_between_requests() {
    let (base, mock) = start_mock().await;
    let f = fixture(&base);
    let channel_id = seed_copilot_channel(&f, &base);
    seed_model(&f, &channel_id, "gpt-5-mini", "gpt-5-mini");

    let _ = chat(&f, "gpt-5-mini", true).await;
    let first = mock.header("x-request-id").expect("应有 x-request-id");
    let _ = chat(&f, "gpt-5-mini", true).await;
    let second = mock.header("x-request-id").expect("应有 x-request-id");

    assert_ne!(first, second, "每次请求的 x-request-id 都该不同");
}

// ── 判据③：token 过期后自动续 ─────────────────────────────────────────

#[tokio::test]
async fn acceptance_a_fresh_copilot_token_is_reused_without_asking_github() {
    let (base, mock) = start_mock().await;
    let f = fixture(&base);
    let channel_id = seed_copilot_channel(&f, &base);
    seed_model(&f, &channel_id, "gpt-5-mini", "gpt-5-mini");

    let _ = chat(&f, "gpt-5-mini", true).await;
    assert_eq!(mock.token_calls.load(Ordering::SeqCst), 1);

    // 缓存新鲜（默认 30 min 有效期）→ **不再打 GitHub**
    let _ = chat(&f, "gpt-5-mini", true).await;
    assert_eq!(
        mock.token_calls.load(Ordering::SeqCst),
        1,
        "token 还新鲜时不该重取"
    );
    assert_eq!(
        mock.header("authorization").as_deref(),
        Some("Bearer MOCK-COPILOT-TOKEN-1")
    );
}

#[tokio::test]
async fn acceptance_an_expiring_copilot_token_is_refreshed_automatically() {
    let (base, mock) = start_mock().await;
    // 有效期 100 s < 提前重取的 300 s → 每次都判为"快过期" → 每次都重取
    mock.token_ttl.store(100, Ordering::SeqCst);
    let f = fixture(&base);
    let channel_id = seed_copilot_channel(&f, &base);
    seed_model(&f, &channel_id, "gpt-5-mini", "gpt-5-mini");

    let _ = chat(&f, "gpt-5-mini", true).await;
    assert_eq!(mock.token_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        mock.header("authorization").as_deref(),
        Some("Bearer MOCK-COPILOT-TOKEN-1")
    );

    // 第二次：token 快过期 → **自动续**（用保险库里的 GitHub token 重换）
    let _ = chat(&f, "gpt-5-mini", true).await;
    assert_eq!(
        mock.token_calls.load(Ordering::SeqCst),
        2,
        "快过期的 token 应当自动重取"
    );
    assert_eq!(
        mock.header("authorization").as_deref(),
        Some("Bearer MOCK-COPILOT-TOKEN-2"),
        "续期后应当用新的 Copilot token"
    );
}

// ── 判据④：模型目录拉取 + 落库 ────────────────────────────────────────

#[tokio::test]
async fn acceptance_the_model_catalog_is_fetched_and_stored() {
    let (base, mock) = start_mock().await;
    let f = fixture(&base);
    let channel_id = seed_copilot_channel(&f, &base);

    let view = llm_router_lib::commands::copilot::copilot_models_refresh_of(&f.app_state, &f.session)
        .await
        .expect("刷新模型目录");

    // 上游只回一次目录
    assert_eq!(mock.models_calls.load(Ordering::SeqCst), 1);
    // `/models` 请求也要带 R-6 头 + Bearer
    let headers = mock.models_headers.lock().unwrap().clone();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(find("authorization").as_deref(), Some("Bearer MOCK-COPILOT-TOKEN-1"));
    assert_eq!(find("copilot-integration-id").as_deref(), Some("vscode-chat"));
    assert_eq!(find("editor-version").as_deref(), Some("vscode/1.104.1"));

    // **只收录 chat 端点 + 策略启用的模型**
    assert_eq!(view.models.len(), 1, "codex 与 disabled 模型不该收录");
    assert_eq!(view.models[0].id, "gpt-5-mini");
    assert_eq!(view.models[0].name, "GPT-5 mini");
    assert_eq!(view.models[0].context_len, Some(264000));
    assert_eq!(view.models[0].supports_tool_calls, Some(true));
    assert_eq!(view.added, 1);
    assert!(view.skipped.is_empty());

    // 落库：别名 = id
    let rows: Vec<(String, String, i64)> = f
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT alias, upstream_id, enabled FROM model WHERE channel_id = ?1 ORDER BY alias",
            )?;
            let rows = stmt.query_map([&channel_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .expect("查模型行");
    assert_eq!(
        rows,
        vec![("gpt-5-mini".to_string(), "gpt-5-mini".to_string(), 1)]
    );
}

#[tokio::test]
async fn acceptance_an_alias_owned_by_another_channel_is_skipped() {
    let (base, _mock) = start_mock().await;
    let f = fixture(&base);
    let channel_id = seed_copilot_channel(&f, &base);

    // 另一个渠道**先占**了这个别名
    f.store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at)
                 VALUES ('ch-other', '别的渠道', 'api_key', 'deepseek', 'https://api.deepseek.com', 1, 100, ?1)",
                rusqlite::params![llm_router_lib::vault::now_secs()],
            )?;
            conn.execute(
                "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                 VALUES ('m_other', 'ch-other', 'deepseek-chat', 'gpt-5-mini', 1)",
                [],
            )?;
            Ok(())
        })
        .expect("占住别名");

    let view = llm_router_lib::commands::copilot::copilot_models_refresh_of(&f.app_state, &f.session)
        .await
        .expect("刷新模型目录");

    assert_eq!(view.added, 0, "别名被占用时不该新增");
    assert_eq!(view.skipped, vec!["gpt-5-mini".to_string()]);
    // **不抢**别人的别名：原行的上游模型名不变
    let owner: String = f
        .store
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT upstream_id FROM model WHERE alias = 'gpt-5-mini'",
                [],
                |row| row.get(0),
            )?)
        })
        .expect("查别名归属");
    assert_eq!(owner, "deepseek-chat");
    // **不抢**别人的别名：Copilot 渠道自己**没有**这一行
    let mine: i64 = f
        .store
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM model WHERE channel_id = ?1 AND alias = 'gpt-5-mini'",
                [&channel_id],
                |row| row.get(0),
            )?)
        })
        .expect("查本渠道模型");
    assert_eq!(mine, 0, "别名被占用时不该在 Copilot 渠道下建行");
}

#[tokio::test]
async fn acceptance_a_model_missing_from_the_catalog_gets_disabled_not_deleted() {
    let (base, _mock) = start_mock().await;
    let f = fixture(&base);
    let channel_id = seed_copilot_channel(&f, &base);

    // 上游目录里已经没有的旧模型
    seed_model(&f, &channel_id, "old-model", "old-model");

    let view = llm_router_lib::commands::copilot::copilot_models_refresh_of(&f.app_state, &f.session)
        .await
        .expect("刷新模型目录");

    assert_eq!(view.disabled, 1, "目录里没有的旧模型应被禁用");
    // **置为禁用**，**不物理删除**（避免连带删掉 route_member 编排）
    let row: i64 = f
        .store
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT enabled FROM model WHERE alias = 'old-model'",
                [],
                |row| row.get(0),
            )?)
        })
        .expect("查旧模型");
    assert_eq!(row, 0, "旧模型应被禁用（而不是被删掉）");
}
