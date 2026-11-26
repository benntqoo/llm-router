//! T8 预设接入 + 自定义渠道 —— 端到端验收测试（mock 上游）。
//!
//! 验收判据（03 §2 T8）：
//! - 预设渠道选中后仅剩"填 key"一步（预设默认值自动落库）；
//! - **自定义渠道可保存并出现在 `/v1/models`**。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use llm_router_lib::commands::channel;
use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::fallback::Forwarder;
use llm_router_lib::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::router::AliasRouter;
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::gateway::usage::CharRatioEstimator;
use llm_router_lib::store::channels::{ChannelInput, ModelInput};
use llm_router_lib::store::usage::StoreUsageSink;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;
use tower::ServiceExt;

// ── mock 上游 ──────────────────────────────────────────────────────────

/// 返回固定 JSON 的 mock 上游（记录收到的请求路径与 Authorization 头）。
async fn mock_handler(
    axum::extract::State(state): axum::extract::State<Arc<MockState>>,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> Response {
    state.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    state
        .paths
        .lock()
        .expect("锁")
        .push(uri.path().to_string());
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    state
        .auths
        .lock()
        .expect("锁")
        .push(auth);
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    state
        .api_keys
        .lock()
        .expect("锁")
        .push(api_key);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        r#"{"id":"chatcmpl-custom","choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#,
    )
        .into_response()
}

#[derive(Default)]
struct MockState {
    hits: std::sync::atomic::AtomicUsize,
    paths: std::sync::Mutex<Vec<String>>,
    auths: std::sync::Mutex<Vec<String>>,
    api_keys: std::sync::Mutex<Vec<String>>,
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

// ── 夹具 ───────────────────────────────────────────────────────────────

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
    let forwarder = Arc::new(Forwarder::new(
        OutboundClient::new().expect("出站客户端"),
        Arc::new(CircuitBreaker::new()),
        Arc::new(StreamUsageDowngrades::default()),
        Arc::clone(&vault),
        Arc::clone(&store),
        Arc::new(StoreUsageSink::new(Arc::clone(&store))),
        Arc::new(CharRatioEstimator),
    ));
    let gateway = GatewayState {
        router: Arc::new(AliasRouter::new(Arc::clone(&store))),
        forwarder: Arc::clone(&forwarder),
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

    // client key（只存哈希）
    let key = auth::generate_client_key();
    let hash = auth::hash_client_key(&key);
    let prefix = auth::key_prefix(&key);
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                 VALUES ('ck1', 't8', ?1, ?2, ?3)",
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

/// 列出 `/v1/models` 的别名。
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
        .filter_map(|item| item["id"].as_str().map(str::to_string))
        .collect()
}

// ── 验收：自定义渠道可保存并出现在 /v1/models ──────────────────────────

#[tokio::test]
async fn acceptance_custom_channel_appears_in_models_list() {
    let f = fixture();

    let payload = ChannelInput {
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
    let saved = channel::channel_save_of(&f.app, payload).expect("保存失败");
    assert_eq!(saved.provider, "custom");
    assert_eq!(saved.models.len(), 1);
    assert_eq!(saved.models[0].alias, "my-alias");

    let aliases = model_aliases(&f).await;
    assert!(
        aliases.contains(&"my-alias".to_string()),
        "自定义渠道的别名应出现在 /v1/models，实际 {aliases:?}"
    );
}

#[tokio::test]
async fn acceptance_preset_channel_saves_with_preset_defaults() {
    let f = fixture();

    // 预设路径：选中预设 → 表单只剩密钥（模型名由预设给出）
    let presets = channel::channel_presets().expect("预设目录");
    assert_eq!(presets.len(), 7, "02 §6.1 表体为 7 行");
    let preset = presets
        .iter()
        .find(|p| p.provider == "deepseek")
        .expect("应有 deepseek 预设");
    assert!(!preset.default_models.is_empty(), "预设应带默认模型建议");

    let payload = channel::input_from_preset(preset, "DeepSeek 官方", Vec::new());
    let saved = channel::channel_save_of(&f.app, payload).expect("保存失败");
    assert_eq!(saved.provider, "deepseek");
    assert_eq!(saved.base_url, "https://api.deepseek.com");
    assert_eq!(saved.meta.path_chat.as_deref(), Some("/v1/chat/completions"));
    assert_eq!(saved.meta.auth_style.as_deref(), Some("bearer"));
    assert_eq!(saved.models.len(), 1);
    assert_eq!(saved.models[0].alias, "deepseek-chat");

    let aliases = model_aliases(&f).await;
    assert!(
        aliases.contains(&"deepseek-chat".to_string()),
        "预设渠道的别名应出现在 /v1/models，实际 {aliases:?}"
    );
}

#[tokio::test]
async fn acceptance_custom_channel_forwards_to_upstream() {
    let (base, mock) = start_mock().await;
    let f = fixture();

    // 自定义渠道：非标准路径 + 自定义鉴权头
    let payload = ChannelInput {
        name: "我的代理".to_string(),
        provider: "custom".to_string(),
        base_url: base.clone(),
        meta: llm_router_lib::store::channels::ChannelMeta {
            path_chat: Some("/v1/chat/completions".to_string()),
            auth_style: Some("custom_header".to_string()),
            auth_header_name: Some("x-api-key".to_string()),
            group_id: None,
            supports_groupid_query: None,
            inject_stream_usage: Some(true),
            extra_headers: Default::default(),
        },
        models: vec![ModelInput {
            alias: "my-alias".to_string(),
            upstream_id: "my-model".to_string(),
            enabled: true,
        }],
        ..ChannelInput::default()
    };
    let saved = channel::channel_save_of(&f.app, payload).expect("保存失败");
    channel::channel_put_key_of(&f.app, &saved.id, "sk-custom-key").expect("录入密钥失败");

    let response = server::router(f.gateway.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {}", f.key))
                .body(Body::from(
                    r#"{"model":"my-alias","messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");
    assert_eq!(response.status(), StatusCode::OK);

    let paths = mock.paths.lock().expect("锁").clone();
    assert_eq!(paths.len(), 1, "应打到上游一次");
    assert!(paths[0].ends_with("/v1/chat/completions"), "实际 {}", paths[0]);
    let auths = mock.auths.lock().expect("锁").clone();
    assert_eq!(auths[0], "", "自定义鉴权样式不应再带 Authorization");
    let api_keys = mock.api_keys.lock().expect("锁").clone();
    assert_eq!(
        api_keys[0], "sk-custom-key",
        "自定义鉴权样式应把密钥放在自定义头 x-api-key"
    );

    // 别名也在模型列表里
    let aliases = model_aliases(&f).await;
    assert!(aliases.contains(&"my-alias".to_string()));
}

#[tokio::test]
async fn acceptance_invalid_channel_is_rejected_without_writing() {
    let f = fixture();

    let payload = ChannelInput {
        name: "坏渠道".to_string(),
        provider: "custom".to_string(),
        base_url: "not-a-url".to_string(),
        models: vec![ModelInput {
            alias: "x".to_string(),
            upstream_id: "x".to_string(),
            enabled: true,
        }],
        ..ChannelInput::default()
    };
    assert!(channel::channel_save_of(&f.app, payload).is_err());
    assert!(channel::channel_list_of(&f.app).expect("列表失败").is_empty());
    assert!(model_aliases(&f).await.is_empty());
}
