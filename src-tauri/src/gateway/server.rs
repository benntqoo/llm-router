//! axum 网关服务：路由表与监听地址。
//!
//! 红线（AGENT.md §5-4）：网关默认绑定 `127.0.0.1`；在 client key 鉴权落地（T3）之前，
//! 任何非回环地址一律拒绝启动，避免本机变成开放代理。

use std::fmt;
use std::net::SocketAddr;

use axum::{routing::get, Json, Router};
use serde::Serialize;

/// 网关默认监听地址。
pub const DEFAULT_GATEWAY_ADDR: &str = "127.0.0.1:8787";

/// 允许用环境变量覆盖监听地址（仅本地调试用）。
pub const ENV_GATEWAY_ADDR: &str = "LLMR_GATEWAY_ADDR";

/// 绑定地址被拒绝的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    /// 地址无法解析为 `host:port`
    InvalidAddr { raw: String, reason: String },
    /// 非回环地址：client key 鉴权尚未实现，拒绝启动。
    NonLoopback(SocketAddr),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::InvalidAddr { raw, reason } => {
                write!(f, "监听地址 {raw:?} 无法解析: {reason}")
            }
            BindError::NonLoopback(addr) => write!(
                f,
                "拒绝绑定非回环地址 {addr}：client key 鉴权落地前只允许 127.0.0.1（AGENT.md §5-4）"
            ),
        }
    }
}

impl std::error::Error for BindError {}

/// `/healthz` 响应体。
#[derive(Serialize, Clone, Debug)]
pub struct HealthPayload {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

/// 构建 axum 路由表。
///
/// v1 入站面（02 §0.1）：`POST /v1/chat/completions` + `GET /v1/models`，由 T3 接入。
/// 当前只保留 `/healthz` 作为存活探针。
pub fn router() -> Router {
    Router::new().route("/healthz", get(healthz))
}

async fn healthz() -> Json<HealthPayload> {
    Json(HealthPayload {
        status: "ok",
        service: "llm-router",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// 从环境变量解析监听地址（默认 `127.0.0.1:8787`）。
pub fn resolve_bind_addr() -> Result<SocketAddr, BindError> {
    let raw = std::env::var(ENV_GATEWAY_ADDR).unwrap_or_else(|_| DEFAULT_GATEWAY_ADDR.to_string());
    resolve_bind_addr_from(&raw)
}

/// 解析并校验监听地址：只允许回环地址。
pub fn resolve_bind_addr_from(raw: &str) -> Result<SocketAddr, BindError> {
    let trimmed = raw.trim();
    let addr: SocketAddr = trimmed
        .parse::<SocketAddr>()
        .map_err(|err: std::net::AddrParseError| BindError::InvalidAddr {
            raw: trimmed.to_string(),
            reason: err.to_string(),
        })?;
    if !addr.ip().is_loopback() {
        return Err(BindError::NonLoopback(addr));
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn non_loopback_is_rejected() {
        let err = resolve_bind_addr_from("0.0.0.0:8787").expect_err("非回环地址必须被拒绝");
        assert!(matches!(err, BindError::NonLoopback(_)));
        assert!(err.to_string().contains("AGENT.md §5-4"));
    }

    #[test]
    fn invalid_addr_is_rejected() {
        let err = resolve_bind_addr_from("not-an-addr").expect_err("非法地址必须被拒绝");
        assert!(matches!(err, BindError::InvalidAddr { .. }));
    }

    #[tokio::test]
    async fn healthz_returns_ok_with_json_body() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let app = router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("请求构造失败"),
            )
            .await
            .expect("请求应成功");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("读取响应体失败");
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).expect("响应体应为 JSON");
        assert_eq!(value["status"], "ok");
        assert_eq!(value["service"], "llm-router");
    }
}
