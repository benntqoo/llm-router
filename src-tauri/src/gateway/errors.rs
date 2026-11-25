//! 统一错误结构（02 §4.4）。
//!
//! 入站返回给 harness 的错误一律为：
//! ```json
//! { "error": { "message": "...", "type": "...", "code": "...", "channel": "...", "retried": [] } }
//! ```

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// 错误类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidRequestError,
    AuthenticationError,
    RateLimitError,
    ApiError,
    UpstreamError,
}

impl ErrorKind {
    /// 默认 HTTP 状态码。
    pub fn default_status(self) -> StatusCode {
        match self {
            ErrorKind::InvalidRequestError => StatusCode::BAD_REQUEST,
            ErrorKind::AuthenticationError => StatusCode::UNAUTHORIZED,
            ErrorKind::RateLimitError => StatusCode::TOO_MANY_REQUESTS,
            ErrorKind::ApiError => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorKind::UpstreamError => StatusCode::BAD_GATEWAY,
        }
    }
}

/// 错误体。
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    /// 人类可读描述。
    pub message: String,
    /// 错误类别。
    #[serde(rename = "type")]
    pub kind: ErrorKind,
    /// 上游原始码（如 `minimax_1004`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 渠道名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// fallback 走过的候选。
    pub retried: Vec<String>,
}

/// 统一错误。
#[derive(Debug, Clone, Serialize)]
pub struct GatewayError {
    pub error: ErrorBody,
    /// HTTP 状态覆盖（不进入 JSON；02 §4.4 的错误体不含该字段）。
    #[serde(skip)]
    status: Option<StatusCode>,
}

impl GatewayError {
    /// 构造统一错误。
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        GatewayError {
            error: ErrorBody {
                message: message.into(),
                kind,
                code: None,
                channel: None,
                retried: Vec::new(),
            },
            status: None,
        }
    }

    /// 覆盖 HTTP 状态码。
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = Some(status);
        self
    }

    /// 附带上游原始码。
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.error.code = Some(code.into());
        self
    }

    /// 附带渠道名。
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.error.channel = Some(channel.into());
        self
    }

    /// 401：缺少或无效的 client key。
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::AuthenticationError, message)
    }

    /// 400：请求不合法。
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidRequestError, message)
    }

    /// 429：超出限流。
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::RateLimitError, message)
    }

    /// 403：Host 校验失败。
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidRequestError, message)
    }

    /// 503：功能尚未接入（骨架阶段）。
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ApiError, message).with_status(StatusCode::SERVICE_UNAVAILABLE)
    }

    /// 502：上游错误。
    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::UpstreamError, message)
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let status = self
            .status
            .unwrap_or_else(|| self.error.kind.default_status());
        (status, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn unauthorized_uses_unified_structure() {
        let response = GatewayError::unauthorized("缺少 client key").into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
        assert_eq!(value["error"]["type"], "authentication_error");
        assert_eq!(value["error"]["message"], "缺少 client key");
        assert!(value["error"]["retried"].is_array());
    }

    #[tokio::test]
    async fn rate_limit_and_request_errors_map_status() {
        assert_eq!(
            GatewayError::rate_limited("超出 RPM").into_response().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            GatewayError::invalid_request("model 缺失").into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            GatewayError::upstream("上游 500").into_response().status(),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn error_type_strings_match_spec() {
        let kinds = [
            (ErrorKind::InvalidRequestError, "invalid_request_error"),
            (ErrorKind::AuthenticationError, "authentication_error"),
            (ErrorKind::RateLimitError, "rate_limit_error"),
            (ErrorKind::ApiError, "api_error"),
            (ErrorKind::UpstreamError, "upstream_error"),
        ];
        for (kind, expected) in kinds {
            let body = serde_json::to_string(&kind).expect("序列化");
            assert_eq!(body, format!("\"{expected}\""));
        }
    }
}
