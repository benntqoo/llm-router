//! axum 网关服务：路由表、监听地址与入站处理。
//!
//! 入站面（v1，02 §0.1）：`POST /v1/chat/completions` + `GET /v1/models`，
//! 另有存活探针 `/healthz`。
//!
//! 红线（AGENT.md §5-4）：默认绑定 `127.0.0.1`；改绑非回环地址时，
//! **必须已存在有效 client key**，否则拒绝启动。

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::gateway::auth::{self, AuthenticatedClient};
use crate::gateway::errors::GatewayError;
use crate::gateway::fallback::Forwarder;
use crate::gateway::inbound::openai::{parse_chat_request, ModelEntry, ModelList};
use crate::gateway::ratelimit::RateLimiter;
use crate::gateway::router::{AliasRouter, RouteError};
use crate::store::Store;

/// 网关默认监听地址。
pub const DEFAULT_GATEWAY_ADDR: &str = "127.0.0.1:8787";

/// 允许用环境变量覆盖监听地址（仅本地调试用）。
pub const ENV_GATEWAY_ADDR: &str = "LLMR_GATEWAY_ADDR";

/// 网关共享状态。
#[derive(Clone)]
pub struct GatewayState {
    /// 数据层（client key 与别名查询）。
    pub store: Arc<Store>,
    /// 限流器。
    pub limiter: Arc<RateLimiter>,
    /// 路由器（别名解析 + 路由策略；持有 sticky / round_robin 内存状态）。
    pub router: Arc<AliasRouter>,
    /// 出站转发器（reqwest 发送 + 熔断 + fallback + SSE 直通）。
    pub forwarder: Arc<Forwarder>,
}

impl fmt::Debug for GatewayState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayState").finish_non_exhaustive()
    }
}

/// 绑定地址被拒绝的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    /// 地址无法解析为 `host:port`
    InvalidAddr { raw: String, reason: String },
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::InvalidAddr { raw, reason } => {
                write!(f, "监听地址 {raw:?} 无法解析: {reason}")
            }
        }
    }
}

impl std::error::Error for BindError {}

/// `/healthz` 响应体。
#[derive(serde::Serialize, Clone, Debug)]
pub struct HealthPayload {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

/// 构建 axum 路由表。
pub fn router(state: GatewayState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .with_state(state)
}

async fn healthz() -> Json<HealthPayload> {
    Json(HealthPayload {
        status: "ok",
        service: "llm-router",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// `POST /v1/chat/completions`
///
/// 当前阶段（T6）完成：Host 校验 → client key 鉴权 → 限流 → body 解析 → 路由解析
/// → **出站转发**（fallback + 熔断 + SSE 直通）。
async fn chat_completions(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, GatewayError> {
    auth::check_host(&headers)?;
    let client = auth::authenticate(&state.store, &headers)?;
    check_rate_limit(&state, &client)?;

    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| GatewayError::invalid_request(format!("请求体不是合法 JSON: {e}")))?;
    let request = parse_chat_request(value)?;

    // ②→③ 路由解析（02 §2.1 ③）
    let plan = state
        .router
        .resolve(&client.id, &request.model)
        .await
        .map_err(route_error_to_gateway)?;

    // ④⑤⑥⑧⑨ 候选链 + 出站转发 + 熔断 + SSE 直通（02 §2.1）
    state.forwarder.forward(&plan, &request, &headers).await
}

/// 路由错误 → 统一错误结构。
fn route_error_to_gateway(err: RouteError) -> GatewayError {
    match err {
        RouteError::AliasNotFound { alias, available } => GatewayError::invalid_request(format!(
            "未知的模型别名 {alias:?}：既没有匹配的 model.alias，也没有匹配的 route_group.alias"
        ))
        .with_code("model_not_found")
        .with_available_models(available),
        RouteError::NoCandidate { alias, reason } => {
            GatewayError::invalid_request(format!("别名 {alias:?} 没有可用候选：{reason}"))
        }
        RouteError::Internal(message) => GatewayError::new(
            crate::gateway::errors::ErrorKind::ApiError,
            format!("路由查询失败: {message}"),
        ),
    }
}

/// `GET /v1/models`：返回模型别名列表。
///
/// 别名 = `model.alias`（单别名）∪ `route_group.alias`（路由组对外名）。
async fn list_models(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Result<Json<ModelList>, GatewayError> {
    auth::check_host(&headers)?;
    let client = auth::authenticate(&state.store, &headers)?;
    check_rate_limit(&state, &client)?;

    let entries: Vec<ModelEntry> = state
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT alias, owned_by FROM (
                     SELECT m.alias AS alias, c.name AS owned_by
                     FROM model m JOIN channel c ON c.id = m.channel_id
                     WHERE m.enabled = 1 AND c.enabled = 1
                     UNION
                     SELECT rg.alias AS alias, 'route_group' AS owned_by
                     FROM route_group rg
                 ) ORDER BY alias",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ModelEntry {
                    id: row.get(0)?,
                    object: "model",
                    created: 0,
                    owned_by: row.get(1)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .map_err(|e| GatewayError::new(
            crate::gateway::errors::ErrorKind::ApiError,
            format!("别名查询失败: {e}"),
        ))?;

    Ok(Json(ModelList::from_entries(entries)))
}

/// 限流检查。
fn check_rate_limit(
    state: &GatewayState,
    client: &AuthenticatedClient,
) -> Result<(), GatewayError> {
    state.limiter.check(
        &client.id,
        client.rpm_limit,
        client.daily_tokens,
        crate::vault::now_secs(),
    )
}

/// 从环境变量解析监听地址（默认 `127.0.0.1:8787`）。
pub fn resolve_bind_addr() -> Result<SocketAddr, BindError> {
    let raw = std::env::var(ENV_GATEWAY_ADDR).unwrap_or_else(|_| DEFAULT_GATEWAY_ADDR.to_string());
    resolve_bind_addr_from(&raw)
}

/// 解析监听地址（仅做格式解析；是否允许绑定由 `check_bind_policy` 判定）。
pub fn resolve_bind_addr_from(raw: &str) -> Result<SocketAddr, BindError> {
    let trimmed = raw.trim();
    trimmed.parse::<SocketAddr>().map_err(|err| BindError::InvalidAddr {
        raw: trimmed.to_string(),
        reason: err.to_string(),
    })
}

/// 绑定策略（AGENT.md §5-4）。
///
/// - 回环地址：直接允许；
/// - 非回环地址：**必须已有有效 client key**，否则拒绝启动（否则等于开放代理）。
pub fn check_bind_policy(addr: SocketAddr, has_client_key: bool) -> Result<(), String> {
    if addr.ip().is_loopback() {
        return Ok(());
    }
    if has_client_key {
        return Ok(());
    }
    Err(format!(
        "拒绝绑定非回环地址 {addr}：改绑必须先存在有效 client key（AGENT.md §5-4），否则本机将变成开放代理"
    ))
}

/// 统计当前是否存在可用的 client key。
pub fn has_client_key(store: &Store) -> bool {
    store
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM client_key WHERE expires_at IS NULL OR expires_at > ?1",
                [crate::vault::now_secs()],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
        .unwrap_or(false)
}

/// 诊断用的健康检查载荷。
pub fn health_payload() -> serde_json::Value {
    json!({
        "status": "ok",
        "service": "llm-router",
        "version": env!("CARGO_PKG_VERSION")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::gateway::circuit::CircuitBreaker;
    use crate::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
    use crate::vault::Vault;

    /// 启动一个恒返回 500 的本机 mock 上游，返回 base_url。
    ///
    /// 不用"黑洞地址"（如 127.0.0.1:9）：本机防火墙会把这类连接丢弃，
    /// 导致每次等待 30s 连接超时，拖慢整个测试套件。
    async fn start_mock_500() -> String {
        use axum::response::IntoResponse;
        let app = axum::Router::new().fallback(axum::routing::any(|| async {
            (StatusCode::INTERNAL_SERVER_ERROR, "upstream boom").into_response()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定 mock 端口");
        let addr = listener.local_addr().expect("读取 mock 地址");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    async fn temp_state() -> (tempfile::TempDir, GatewayState, Arc<RwLock<Vault>>, String) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
        let vault = Arc::new(RwLock::new(
            Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None)
                .expect("打开保险库"),
        ));
        let forwarder = Arc::new(Forwarder::new(
            OutboundClient::new().expect("出站客户端"),
            Arc::new(CircuitBreaker::new()),
            Arc::new(StreamUsageDowngrades::default()),
            Arc::clone(&vault),
        ));
        let state = GatewayState {
            router: Arc::new(AliasRouter::new(Arc::clone(&store))),
            store,
            limiter: Arc::new(RateLimiter::new()),
            forwarder,
        };
        let mock_base = start_mock_500().await;
        (dir, state, vault, mock_base)
    }

    fn create_client_key(state: &GatewayState) -> String {
        let key = auth::generate_client_key();
        let hash = auth::hash_client_key(&key);
        let prefix = auth::key_prefix(&key);
        let now = crate::vault::now_secs();
        state
            .store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                     VALUES ('ck1', '测试', ?1, ?2, ?3)",
                    rusqlite::params![hash, prefix, now],
                )?;
                Ok(())
            })
            .expect("写入 client key");
        key
    }

    fn request(method: &str, uri: &str, key: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "127.0.0.1:8787");
        if let Some(key) = key {
            builder = builder.header("authorization", format!("Bearer {key}"));
        }
        builder
            .body(Body::from(r#"{"model":"deepseek-chat","messages":[]}"#))
            .expect("构造请求")
    }

    fn chat_request(key: Option<&str>) -> Request<Body> {
        request("POST", "/v1/chat/completions", key)
    }

    fn models_request(key: Option<&str>) -> Request<Body> {
        request("GET", "/v1/models", key)
    }

    #[tokio::test]
    async fn chat_completions_without_key_returns_401_unified_structure() {
        let (_dir, state, _vault, _mock) = temp_state().await;
        let response = router(state)
            .oneshot(chat_request(None))
            .await
            .expect("请求应成功");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
        assert_eq!(value["error"]["type"], "authentication_error");
        assert!(value["error"]["message"].is_string());
    }

    #[tokio::test]
    async fn chat_completions_with_invalid_key_returns_401() {
        let (_dir, state, _vault, _mock) = temp_state().await;
        let response = router(state)
            .oneshot(chat_request(Some("llmr_wrong")))
            .await
            .expect("请求应成功");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// 种入一个渠道 + 模型（别名 `deepseek-chat`）+ 凭证。
    ///
    /// base_url 指向本机 mock（恒返回 500）：出站必然失败，用来验证
    /// "鉴权通过 → 进入出站阶段 → 统一错误结构"，且不依赖外网。
    fn seed_model(state: &GatewayState, vault: &RwLock<Vault>, base_url: &str) {
        let now = crate::vault::now_secs();
        state
            .store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at)
                     VALUES ('c1', 'DeepSeek 官方', 'api_key', 'deepseek', ?1, 1, 100, ?2)",
                    rusqlite::params![base_url, now],
                )?;
                conn.execute(
                    "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                     VALUES ('m1', 'c1', 'deepseek-chat', 'deepseek-chat', 1)",
                    [],
                )?;
                Ok(())
            })
            .expect("写入渠道与模型");
        let guard = vault.write().unwrap_or_else(|e| e.into_inner());
        guard
            .put_credential(
                "c1",
                "api_key",
                &crate::vault::crypto::SecretBytes::from_slice(b"sk-test"),
            )
            .expect("写入凭证");
    }

    #[tokio::test]
    async fn chat_completions_with_valid_key_passes_auth() {
        let (_dir, state, vault, mock) = temp_state().await;
        let key = create_client_key(&state);
        seed_model(&state, &vault, &mock);
        let response = router(state)
            .oneshot(chat_request(Some(&key)))
            .await
            .expect("请求应成功");
        // 鉴权通过 → 路由命中 → 出站阶段（本例上游不可达 → 502 统一结构）
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
        assert_eq!(value["error"]["type"], "upstream_error");
        assert_eq!(value["error"]["channel"], "DeepSeek 官方");
    }

    #[tokio::test]
    async fn chat_completions_with_unknown_alias_returns_400_with_available_aliases() {
        // 02 §2.1 ③：别名未命中 → 400，响应体附可用别名列表
        let (_dir, state, vault, mock) = temp_state().await;
        let key = create_client_key(&state);
        seed_model(&state, &vault, &mock);
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("host", "127.0.0.1:8787")
            .header("authorization", format!("Bearer {key}"))
            .body(Body::from(r#"{"model":"not-exist","messages":[]}"#))
            .expect("构造请求");

        let response = router(state).oneshot(request).await.expect("请求应成功");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
        assert_eq!(value["error"]["type"], "invalid_request_error");
        assert_eq!(value["error"]["code"], "model_not_found");
        let available = value["error"]["available_models"]
            .as_array()
            .expect("应附可用别名列表");
        assert_eq!(available.len(), 1);
        assert_eq!(available[0], "deepseek-chat");
    }

    #[tokio::test]
    async fn models_returns_empty_list_without_models() {
        let (_dir, state, _vault, _mock) = temp_state().await;
        let key = create_client_key(&state);
        let response = router(state)
            .oneshot(models_request(Some(&key)))
            .await
            .expect("请求应成功");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
        assert_eq!(value["object"], "list");
        assert!(value["data"].as_array().expect("data 应为数组").is_empty());
    }

    #[tokio::test]
    async fn models_requires_key() {
        let (_dir, state, _vault, _mock) = temp_state().await;
        let response = router(state)
            .oneshot(models_request(None))
            .await
            .expect("请求应成功");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn non_loopback_host_is_rejected() {
        let (_dir, state, _vault, _mock) = temp_state().await;
        let key = create_client_key(&state);
        let req = Request::builder()
            .uri("/v1/models")
            .header("host", "evil.example.com")
            .header("authorization", format!("Bearer {key}"))
            .body(Body::empty())
            .expect("构造请求");
        let response = router(state).oneshot(req).await.expect("请求应成功");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rpm_limit_returns_429() {
        let (_dir, state, _vault, _mock) = temp_state().await;
        let key = create_client_key(&state);
        state
            .store
            .with_conn(|conn| {
                conn.execute("UPDATE client_key SET rpm_limit = 1", [])?;
                Ok(())
            })
            .expect("设置限额");

        let app = router(state);
        let first = app
            .clone()
            .oneshot(models_request(Some(&key)))
            .await
            .expect("请求应成功");
        assert_eq!(first.status(), StatusCode::OK);

        let second = app
            .oneshot(models_request(Some(&key)))
            .await
            .expect("请求应成功");
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let bytes = axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
        assert_eq!(value["error"]["type"], "rate_limit_error");
    }

    #[test]
    fn default_addr_is_loopback() {
        let addr = resolve_bind_addr_from(DEFAULT_GATEWAY_ADDR).expect("默认地址应可解析");
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 8787);
    }

    #[test]
    fn ipv6_loopback_is_allowed() {
        let addr = resolve_bind_addr_from("[::1]:8787").expect("IPv6 回环应允许");
        assert!(addr.ip().is_loopback());
    }

    #[test]
    fn invalid_addr_is_rejected() {
        let err = resolve_bind_addr_from("not-an-addr").expect_err("非法地址必须被拒绝");
        assert!(matches!(err, BindError::InvalidAddr { .. }));
    }

    #[test]
    fn bind_policy_blocks_non_loopback_without_client_key() {
        let addr: SocketAddr = "0.0.0.0:8787".parse().expect("解析");
        let err = check_bind_policy(addr, false).expect_err("无 client key 时必须拒绝");
        assert!(err.contains("AGENT.md §5-4"));

        // 已有 client key 时允许改绑
        assert!(check_bind_policy(addr, true).is_ok());
        // 回环地址始终允许
        let loopback: SocketAddr = "127.0.0.1:8787".parse().expect("解析");
        assert!(check_bind_policy(loopback, false).is_ok());
    }
}
