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
    /// 可用别名列表（仅"别名未命中"时填充；02 §2.1 ③ 要求附可用别名）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<String>>,
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
    /// 建议客户端等待的秒数（429 时写进 `Retry-After` 响应头；不进入 JSON）。
    #[serde(skip)]
    retry_after_secs: Option<u64>,
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
                available_models: None,
                retried: Vec::new(),
            },
            status: None,
            retry_after_secs: None,
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

    /// 附上可用别名列表（别名未命中时，02 §2.1 ③ 要求响应体附可用别名）。
    pub fn with_available_models(mut self, models: Vec<String>) -> Self {
        self.error.available_models = Some(models);
        self
    }

    /// 附上 fallback 走过的候选（02 §4.4 的 `retried`）。
    pub fn with_retried(mut self, retried: Vec<String>) -> Self {
        self.error.retried = retried;
        self
    }

    /// 附上建议的重试等待秒数（写进 `Retry-After` 响应头）。
    ///
    /// 只用于 429：让客户端知道限流冷却还剩多久，而不是自己乱猜。
    pub fn with_retry_after(mut self, secs: u64) -> Self {
        self.retry_after_secs = Some(secs);
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
        let retry_after = self.retry_after_secs;
        let mut response = (status, Json(self)).into_response();
        if let Some(secs) = retry_after {
            if let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string()) {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
        }
        response
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

    #[tokio::test]
    async fn retry_after_is_written_as_header_only() {
        let response = GatewayError::rate_limited("上游限流")
            .with_retry_after(7)
            .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("7")
        );

        // 错误体里不出现 retry_after 字段（02 §4.4 的字段表没有它）
        let bytes = to_bytes(response.into_body(), usize::MAX).await.expect("读取响应体");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
        assert_eq!(value["error"]["type"], "rate_limit_error");
        assert!(
            value["error"].get("retry_after").is_none(),
            "Retry-After 只走响应头，不进错误体：{value}"
        );
    }

    #[tokio::test]
    async fn no_retry_after_header_by_default() {
        let response = GatewayError::upstream("上游 500").into_response();
        assert!(response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .is_none());
    }
}
