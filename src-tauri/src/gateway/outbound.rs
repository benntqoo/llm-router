//! 出站发送（reqwest）与上游响应处理。
//!
//! 职责（02 §3.3 / §4.3）：
//! - 按 T4 的 `OutgoingCall` 发出请求（URL / 头 / body 已是改写后的成品）；
//! - 错误分类：可重试（429 / 5xx / 连接超时 / 网络错误）vs 不可重试（401/403/400）；
//! - 429 的 `Retry-After` 与 `x-ratelimit-*` 解析成冷却参数；
//! - P-3：MiniMax 可能 **HTTP 200 但业务错误**，需解析 `base_resp.status_code`。

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use bytes::Bytes;
use futures_util::Stream;

use crate::providers::adapter::OutgoingCall;

/// 连接建立超时（秒）。
pub const CONNECT_TIMEOUT_SECS: u64 = 30;
/// 首字节（收到响应头）超时（秒）；推理模型长思考需要放宽。
pub const FIRST_BYTE_TIMEOUT_SECS: u64 = 600;
/// 非流式响应体缓冲上限（字节）。
pub const MAX_BUFFERED_BYTES: usize = 32 * 1024 * 1024;

/// 上游响应体。
pub enum UpstreamBody {
    /// 整块缓冲（非流式）。
    Buffered(Vec<u8>),
    /// 逐帧流（流式，零缓冲直通）。
    Stream(Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>),
}

/// 上游响应。
pub struct UpstreamResponse {
    /// 状态码。
    pub status: StatusCode,
    /// 响应头（上游原样，P-6 透传用）。
    pub headers: HeaderMap,
    /// 响应体。
    pub body: UpstreamBody,
}

/// 发送失败（连接层）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// 首字节超时（可重试）。
    Timeout(String),
    /// 网络 / 连接错误（可重试）。
    Transport(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Timeout(msg) => write!(f, "首字节超时: {msg}"),
            SendError::Transport(msg) => write!(f, "连接上游失败: {msg}"),
        }
    }
}

impl SendError {
    /// 连接层错误一律可重试（02 §3.3）。
    pub fn is_retryable(&self) -> bool {
        true
    }
}

/// 限流信息（429 冷却参数）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RateLimitInfo {
    /// `Retry-After`（秒）。
    pub retry_after: Option<Duration>,
    /// `x-ratelimit-reset-requests`（秒）。
    pub reset_requests: Option<Duration>,
    /// `x-ratelimit-remaining-requests`。
    pub remaining_requests: Option<i64>,
    /// `x-ratelimit-reset-tokens`（秒）。
    pub reset_tokens: Option<Duration>,
    /// `x-ratelimit-remaining-tokens`。
    pub remaining_tokens: Option<i64>,
    /// `x-ratelimit-limit-requests`：上游的 RPM 上限（限额自学习用，02 §8.2）。
    pub limit_requests: Option<i64>,
    /// `x-ratelimit-limit-tokens`：上游的 TPM 上限（限额自学习用，02 §8.2）。
    pub limit_tokens: Option<i64>,
}

impl RateLimitInfo {
    /// 冷却时长建议：取 `Retry-After` 与 `x-ratelimit-reset-requests` 的较大值。
    pub fn cooldown_hint(&self) -> Option<Duration> {
        let a = self.retry_after;
        let b = self.reset_requests;
        match (a, b) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// 从响应头解析限流信息。
    pub fn parse(headers: &HeaderMap) -> RateLimitInfo {
        RateLimitInfo {
            retry_after: headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_retry_after),
            reset_requests: headers
                .get("x-ratelimit-reset-requests")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_duration_secs),
            remaining_requests: headers
                .get("x-ratelimit-remaining-requests")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok()),
            reset_tokens: headers
                .get("x-ratelimit-reset-tokens")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_duration_secs),
            remaining_tokens: headers
                .get("x-ratelimit-remaining-tokens")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok()),
            limit_requests: headers
                .get("x-ratelimit-limit-requests")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok()),
            limit_tokens: headers
                .get("x-ratelimit-limit-tokens")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<i64>().ok()),
        }
    }
}

/// 解析 `Retry-After`：支持秒数形式（HTTP-date 形式暂不支持）。
pub fn parse_retry_after(raw: &str) -> Option<Duration> {
    let trimmed = raw.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    None
}

/// 解析时长字符串：纯数字按秒；`"1s"` / `"6m"` / `"1h"` / `"500ms"`。
pub fn parse_duration_secs(raw: &str) -> Option<Duration> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let (digits, unit) = trimmed.split_at(
        trimmed
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    let value: u64 = digits.parse().ok()?;
    match unit.trim() {
        "ms" => Some(Duration::from_millis(value)),
        "s" | "sec" | "secs" | "second" | "seconds" => Some(Duration::from_secs(value)),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(Duration::from_secs(value * 60)),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(Duration::from_secs(value * 3600)),
        _ => None,
    }
}

/// 判断错误体是否提到 `stream_options`（P-4 学习式兼容的触发条件）。
pub fn mentions_stream_options(body: &[u8]) -> bool {
    String::from_utf8_lossy(body).contains("stream_options")
}

/// 出站客户端。
#[derive(Debug, Clone)]
pub struct OutboundClient {
    client: reqwest::Client,
}

impl OutboundClient {
    /// 构造客户端。
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .user_agent(concat!("llm-router/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(OutboundClient { client })
    }

    /// 发送出站调用。
    ///
    /// `stream = true` 时响应体走逐帧流（零缓冲）；否则整块缓冲。
    pub async fn send(&self, call: &OutgoingCall, stream: bool) -> Result<UpstreamResponse, SendError> {
        let mut builder = self.client.request(call.method.clone(), &call.url);
        for (name, value) in call.headers.iter() {
            builder = builder.header(name, value);
        }
        let request = builder.body(call.body.clone());

        let response = tokio::time::timeout(
            Duration::from_secs(FIRST_BYTE_TIMEOUT_SECS),
            request.send(),
        )
        .await
        .map_err(|_| {
            SendError::Timeout(format!(
                "上游 {} 在 {}s 内未返回响应头",
                call.url, FIRST_BYTE_TIMEOUT_SECS
            ))
        })?
        .map_err(|err| SendError::Transport(format!("{}: {err}", call.url)))?;

        let status = response.status();
        let headers = response.headers().clone();
        // 错误响应一律整块缓冲：错误体要读出来做错误分类（02 §3.3）与 P-4 判定
        let is_error = status.is_client_error() || status.is_server_error();

        if stream && !is_error {
            Ok(UpstreamResponse {
                status,
                headers,
                body: UpstreamBody::Stream(Box::pin(response.bytes_stream())),
            })
        } else {
            let bytes = response
                .bytes()
                .await
                .map_err(|err| SendError::Transport(format!("读取上游响应体失败: {err}")))?;
            if bytes.len() > MAX_BUFFERED_BYTES {
                return Err(SendError::Transport(format!(
                    "上游响应体超过 {} 字节上限",
                    MAX_BUFFERED_BYTES
                )));
            }
            Ok(UpstreamResponse {
                status,
                headers,
                body: UpstreamBody::Buffered(bytes.to_vec()),
            })
        }
    }
}

/// 流式注入降级状态（P-4 学习式兼容）。
///
/// 某渠道因 R-4 注入 `stream_options` 被上游 400 拒绝后，**本次会话内**
/// 关闭该渠道的注入（02 §4.3 P-4）。状态只存内存，不落库。
#[derive(Debug, Clone, Default)]
pub struct StreamUsageDowngrades {
    disabled: Arc<Mutex<HashSet<String>>>,
}

impl StreamUsageDowngrades {
    /// 该渠道是否已关闭注入。
    pub fn is_disabled(&self, channel_id: &str) -> bool {
        lock(&self.disabled).contains(channel_id)
    }

    /// 关闭该渠道的注入（返回是否首次关闭）。
    pub fn disable(&self, channel_id: &str) -> bool {
        lock(&self.disabled).insert(channel_id.to_string())
    }

    /// 恢复该渠道的注入。
    pub fn enable(&self, channel_id: &str) {
        lock(&self.disabled).remove(channel_id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn parses_retry_after_seconds() {
        assert_eq!(
            parse_retry_after("30"),
            Some(Duration::from_secs(30))
        );
        assert_eq!(parse_retry_after("  5 "), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
    }

    #[test]
    fn parses_duration_strings() {
        assert_eq!(parse_duration_secs("12"), Some(Duration::from_secs(12)));
        assert_eq!(parse_duration_secs("1s"), Some(Duration::from_secs(1)));
        assert_eq!(parse_duration_secs("6m"), Some(Duration::from_secs(360)));
        assert_eq!(parse_duration_secs("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration_secs("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("abc"), None);
    }

    #[test]
    fn rate_limit_info_reads_all_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("30"));
        headers.insert("x-ratelimit-reset-requests", HeaderValue::from_static("6m"));
        headers.insert("x-ratelimit-remaining-requests", HeaderValue::from_static("0"));
        headers.insert("x-ratelimit-reset-tokens", HeaderValue::from_static("12s"));
        headers.insert("x-ratelimit-remaining-tokens", HeaderValue::from_static("1234"));
        headers.insert("x-ratelimit-limit-requests", HeaderValue::from_static("60"));
        headers.insert("x-ratelimit-limit-tokens", HeaderValue::from_static("120000"));

        let info = RateLimitInfo::parse(&headers);
        assert_eq!(info.retry_after, Some(Duration::from_secs(30)));
        assert_eq!(info.reset_requests, Some(Duration::from_secs(360)));
        assert_eq!(info.remaining_requests, Some(0));
        assert_eq!(info.reset_tokens, Some(Duration::from_secs(12)));
        assert_eq!(info.remaining_tokens, Some(1234));
        // 上限（限额自学习用）
        assert_eq!(info.limit_requests, Some(60));
        assert_eq!(info.limit_tokens, Some(120_000));
        // 冷却取较大值
        assert_eq!(info.cooldown_hint(), Some(Duration::from_secs(360)));
    }

    #[test]
    fn rate_limit_info_without_headers() {
        let info = RateLimitInfo::parse(&HeaderMap::new());
        assert_eq!(info.retry_after, None);
        assert_eq!(info.cooldown_hint(), None);
    }

    #[test]
    fn detects_stream_options_in_error_body() {
        assert!(mentions_stream_options(
            br#"{"error":{"message":"stream_options is not supported"}}"#
        ));
        assert!(!mentions_stream_options(br#"{"error":{"message":"bad param"}}"#));
    }

    #[test]
    fn downgrades_are_per_channel() {
        let downgrades = StreamUsageDowngrades::default();
        assert!(!downgrades.is_disabled("c1"));
        assert!(downgrades.disable("c1"));
        assert!(downgrades.is_disabled("c1"));
        assert!(!downgrades.is_disabled("c2"), "降级只影响该渠道");
        assert!(!downgrades.disable("c1"), "重复关闭返回 false");
        downgrades.enable("c1");
        assert!(!downgrades.is_disabled("c1"));
    }
}
