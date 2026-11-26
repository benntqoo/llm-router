//! 用量采集（02 §8.1）。
//!
//! - **解析上游 `usage`**：OpenAI 兼容格式；MiniMax 的
//!   `usage.completion_tokens_details.reasoning_tokens` 额外落到 `reasoning_tokens`（§4.2）；
//! - **流式旁路 tee**：转发字节的同时在**副本**上扫 SSE 帧（找 `usage` 帧、记 TTFT），
//!   **不缓冲全量**（§2.2 的零缓冲直通不变）；
//! - **估算入口**：上游没回 usage 时走 `TokenEstimator`，并标 `estimated=1`。
//!   本模块只提供入口与启发式实现（字符数估算），**T12 换成 tiktoken-rs**。

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use futures_util::Stream;
use serde_json::Value;

use crate::gateway::stream::UpstreamStream;
use crate::store::usage::{UsageRecord, UsageSink, UsageStatus};

/// 入站协议标识（v1 恒为 OpenAI）。
pub const PROTOCOL_OPENAI: &str = "openai";

/// 估算用的输出文本保留上限（字节）。
///
/// 流式响应要边收边估，但**不能无限缓冲**——超过这个上限就不再累积，
/// 估算值随之变成"前缀估算"（对字符数启发式影响很小）。
pub const MAX_ESTIMATE_TEXT_BYTES: usize = 64 * 1024;

/// 上游回报的 token 用量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTokens {
    /// 输入 token。
    pub prompt_tokens: i64,
    /// 输出 token。
    pub completion_tokens: i64,
    /// 缓存命中 token。
    pub cache_read_tokens: i64,
    /// 缓存写入 token。
    pub cache_write_tokens: i64,
    /// 推理 token（MiniMax 特例）。
    pub reasoning_tokens: i64,
}

impl UsageTokens {
    /// 输入 + 输出 token。
    pub fn total(&self) -> i64 {
        self.prompt_tokens + self.completion_tokens
    }

    /// 从上游的 `usage` 对象解析。
    ///
    /// 兼容两种命名：OpenAI 的 `prompt_tokens` / `completion_tokens`
    /// 与 Anthropic 风格的 `input_tokens` / `output_tokens`。
    ///
    /// 对象里**一个已知字段都没有**时返回 `None`——空的 `usage: {}`
    /// 与"这帧压根没有 usage"必须区分开，否则会被误判成"上游报了 0 token"。
    pub fn from_value(usage: &Value) -> Option<Self> {
        let map = usage.as_object()?;
        const KNOWN: &[&str] = &[
            "prompt_tokens",
            "completion_tokens",
            "input_tokens",
            "output_tokens",
        ];
        if !KNOWN.iter().any(|key| map.contains_key(*key)) {
            return None;
        }
        let int = |key: &str| -> i64 {
            map.get(key)
                .and_then(Value::as_i64)
                .or_else(|| map.get(key).and_then(Value::as_u64).map(|v| v as i64))
                .unwrap_or(0)
        };
        let nested = |parent: &str, child: &str| -> i64 {
            map.get(parent)
                .and_then(|p| p.get(child))
                .and_then(Value::as_i64)
                .or_else(|| {
                    map.get(parent)
                        .and_then(|p| p.get(child))
                        .and_then(Value::as_u64)
                        .map(|v| v as i64)
                })
                .unwrap_or(0)
        };
        let pick = |a: i64, b: i64| if a != 0 { a } else { b };

        Some(Self {
            prompt_tokens: pick(int("prompt_tokens"), int("input_tokens")),
            completion_tokens: pick(int("completion_tokens"), int("output_tokens")),
            cache_read_tokens: pick(
                nested("prompt_tokens_details", "cached_tokens"),
                nested("input_tokens_details", "cached_tokens"),
            ),
            cache_write_tokens: int("cache_creation_input_tokens"),
            reasoning_tokens: pick(
                nested("completion_tokens_details", "reasoning_tokens"),
                int("reasoning_tokens"),
            ),
        })
    }
}

/// 从一个响应 JSON 里取 `usage`（没有则 `None`）。
pub fn parse_usage(response: &Value) -> Option<UsageTokens> {
    response.get("usage").and_then(UsageTokens::from_value)
}

/// 汇总"上游回报的 usage"或"本地估算"的结果。
///
/// 返回 `(token 数, 是否估算)`：上游给了 usage 就用上游的（`estimated=false`），
/// 否则走估算入口（`estimated=true`）。
pub fn resolve_tokens(
    upstream: Option<UsageTokens>,
    estimator: &dyn TokenEstimator,
    model: &str,
    prompt: &str,
    completion: &str,
) -> (UsageTokens, bool) {
    match upstream {
        Some(usage) => (usage, false),
        None => {
            let counts = estimator.estimate(model, prompt, completion);
            (
                UsageTokens {
                    prompt_tokens: counts.map(|c| c.prompt_tokens).unwrap_or(0),
                    completion_tokens: counts.map(|c| c.completion_tokens).unwrap_or(0),
                    ..UsageTokens::default()
                },
                true,
            )
        }
    }
}

/// 估算出的 token 数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenCounts {
    /// 估算的输入 token。
    pub prompt_tokens: i64,
    /// 估算的输出 token。
    pub completion_tokens: i64,
}

/// token 估算器（**入口**；真正的 tokenizer 在 T12）。
pub trait TokenEstimator: Send + Sync {
    /// 估算输入/输出 token；返回 `None` 表示无法估算。
    fn estimate(&self, model: &str, prompt: &str, completion: &str) -> Option<TokenCounts>;
}

/// 字符数启发式估算器（T12 前的占位实现）。
///
/// CJK 按 1 字 1 token，其余按 4 字符 1 token。
pub struct CharRatioEstimator;

impl std::fmt::Debug for CharRatioEstimator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CharRatioEstimator").finish_non_exhaustive()
    }
}

impl TokenEstimator for CharRatioEstimator {
    fn estimate(&self, _model: &str, prompt: &str, completion: &str) -> Option<TokenCounts> {
        Some(TokenCounts {
            prompt_tokens: estimate_tokens(prompt),
            completion_tokens: estimate_tokens(completion),
        })
    }
}

/// 字符数 → token 数启发式。
pub fn estimate_tokens(text: &str) -> i64 {
    let mut tokens = 0i64;
    let mut latin_run = 0i64;
    for ch in text.chars() {
        if is_cjk(ch) {
            latin_run = 0;
            tokens += 1;
        } else {
            latin_run += 1;
            if latin_run >= 4 {
                latin_run = 0;
                tokens += 1;
            }
        }
    }
    if latin_run > 0 {
        tokens += 1;
    }
    tokens
}

/// 是否是 CJK 字符（含假名、谚文）。
fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x30FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xAC00..=0xD7AF
            | 0x20000..=0x2A6DF
    )
}

/// SSE 帧扫描器：从字节流里切出 `data:` 负载。
///
/// 只用于**解析**（在副本上），不影响转发字节。
#[derive(Debug, Default)]
pub struct SseScanner {
    buf: Vec<u8>,
}

impl SseScanner {
    /// 构造空扫描器。
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 喂入字节，返回本次切出的 `data:` 负载（不含 `[DONE]`）。
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some((end, consumed)) = find_event_boundary(&self.buf) {
            let event = self.buf[..end].to_vec();
            self.buf.drain(..consumed);
            if let Some(payload) = extract_data_payload(&event) {
                out.push(payload);
            }
        }
        out
    }

    /// 流结束时冲刷残余（上游没用空行收尾的最后一帧）。
    pub fn flush(&mut self) -> Vec<String> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        let event = std::mem::take(&mut self.buf);
        extract_data_payload(&event).into_iter().collect()
    }
}

/// 找第一个 SSE 事件边界，返回 `(事件内容长度, 消费字节数)`。
fn find_event_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for (pat, len) in [
        (&b"\n\n"[..], 2usize),
        (&b"\r\n\r\n"[..], 4),
        (&b"\n\r\n"[..], 3),
    ] {
        if let Some(pos) = find_sub(buf, pat) {
            best = Some(match best {
                Some((p, l)) if p <= pos => (p, l),
                _ => (pos, len),
            });
        }
    }
    best
}

/// 子串查找。
fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 从一个 SSE 事件里取出 `data:` 负载（多行按 `\n` 连接）。
fn extract_data_payload(event: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(event);
    let mut parts: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("data:") {
            parts.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if parts.is_empty() {
        return None;
    }
    let payload = parts.join("\n");
    if payload.trim() == "[DONE]" {
        return None;
    }
    Some(payload)
}

/// 采集上下文（落库时需要的元信息）。
#[derive(Debug, Clone)]
pub struct UsageContext {
    /// 入站 client key id。
    pub client_key_id: Option<String>,
    /// 出站渠道 id。
    pub channel_id: String,
    /// 账号 id（02 §8.2）。
    pub account_id: String,
    /// 入站别名。
    pub model_alias: String,
    /// 上游真实模型名。
    pub upstream_model: String,
    /// 入站协议。
    pub protocol: String,
    /// 请求提示词文本（估算输入 token 用）。
    pub prompt_text: String,
}

/// 流式用量采集：**旁路 tee**。
///
/// 转发字节的同时在副本上扫 SSE 帧；流结束（或被 drop）时汇总成一条 `UsageRecord`。
pub struct UsageTap<E> {
    inner: UpstreamStream<E>,
    scanner: SseScanner,
    ctx: UsageContext,
    estimator: Arc<dyn TokenEstimator>,
    sink: Arc<dyn UsageSink>,
    started: Instant,
    usage: Option<UsageTokens>,
    ttft_ms: Option<i64>,
    completion_text: String,
    completion_truncated: bool,
    upstream_errored: bool,
    error_kind: Option<String>,
    finished: bool,
}

impl<E> std::fmt::Debug for UsageTap<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageTap")
            .field("channel_id", &self.ctx.channel_id)
            .field("has_usage", &self.usage.is_some())
            .finish_non_exhaustive()
    }
}

impl<E> UsageTap<E> {
    /// 构造 tee。
    pub fn new(
        inner: UpstreamStream<E>,
        ctx: UsageContext,
        estimator: Arc<dyn TokenEstimator>,
        sink: Arc<dyn UsageSink>,
        started: Instant,
    ) -> Self {
        Self {
            inner,
            scanner: SseScanner::new(),
            ctx,
            estimator,
            sink,
            started,
            usage: None,
            ttft_ms: None,
            completion_text: String::new(),
            completion_truncated: false,
            upstream_errored: false,
            error_kind: None,
            finished: false,
        }
    }

    /// 扫一段字节（转发之外的动作都在这里）。
    fn observe(&mut self, bytes: &[u8]) {
        let payloads = self.scanner.push(bytes);
        self.observe_payloads(payloads);
    }

    /// 处理切出来的 SSE 帧负载：找 usage、记 TTFT、累积输出文本。
    fn observe_payloads(&mut self, payloads: Vec<String>) {
        for payload in payloads {
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            if let Some(usage) = value.get("usage").and_then(UsageTokens::from_value) {
                self.usage = Some(usage);
            }
            let (content, reasoning) = delta_text(&value);
            if content.is_empty() && reasoning.is_empty() {
                continue;
            }
            if self.ttft_ms.is_none() {
                // 首个 token（内容或推理）到达即记 TTFT
                self.ttft_ms = Some(self.started.elapsed().as_millis() as i64);
            }
            self.append_completion(&content);
            self.append_completion(&reasoning);
        }
    }

    /// 累积输出文本（有上限，避免无限缓冲）。
    fn append_completion(&mut self, text: &str) {
        if self.completion_truncated || text.is_empty() {
            return;
        }
        if self.completion_text.len() + text.len() > MAX_ESTIMATE_TEXT_BYTES {
            self.completion_truncated = true;
            return;
        }
        self.completion_text.push_str(text);
    }

    /// 汇总成一条用量记录。
    fn build_record(&self, status: UsageStatus) -> UsageRecord {
        let (tokens, estimated) = resolve_tokens(
            self.usage,
            self.estimator.as_ref(),
            &self.ctx.upstream_model,
            &self.ctx.prompt_text,
            &self.completion_text,
        );
        UsageRecord {
            ts: crate::vault::now_secs(),
            client_key_id: self.ctx.client_key_id.clone(),
            channel_id: self.ctx.channel_id.clone(),
            account_id: self.ctx.account_id.clone(),
            model_alias: self.ctx.model_alias.clone(),
            upstream_model: self.ctx.upstream_model.clone(),
            protocol: self.ctx.protocol.clone(),
            prompt_tokens: tokens.prompt_tokens,
            completion_tokens: tokens.completion_tokens,
            cache_read_tokens: tokens.cache_read_tokens,
            cache_write_tokens: tokens.cache_write_tokens,
            reasoning_tokens: tokens.reasoning_tokens,
            estimated,
            ttft_ms: self.ttft_ms,
            duration_ms: self.started.elapsed().as_millis() as i64,
            status,
            error_kind: self.error_kind.clone(),
        }
    }

    /// 收尾（只生效一次）。
    fn finalize(&mut self, status: UsageStatus) {
        if self.finished {
            return;
        }
        self.finished = true;
        let record = self.build_record(status);
        self.sink.record(record);
    }
}

impl<E> Drop for UsageTap<E> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // 没走到流结束就被 drop = 客户端中途断连
        let status = if self.upstream_errored {
            UsageStatus::Error
        } else {
            UsageStatus::Aborted
        };
        self.finalize(status);
    }
}

impl<E> Stream for UsageTap<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = Result<Bytes, E>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => {
                this.observe(&bytes);
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.upstream_errored = true;
                this.error_kind = Some("upstream_error".to_string());
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                // 冲刷没有空行收尾的末帧（上游偶尔这么干，usage 帧不能丢）
                let trailing = this.scanner.flush();
                this.observe_payloads(trailing);
                let status = if this.upstream_errored {
                    UsageStatus::Error
                } else {
                    UsageStatus::Success
                };
                this.finalize(status);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// 取一个 SSE 帧里的输出文本与推理文本。
fn delta_text(value: &Value) -> (String, String) {
    let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    else {
        return (String::new(), String::new());
    };
    let Some(delta) = choice.get("delta") else {
        return (String::new(), String::new());
    };
    let content = text_of(delta.get("content")).unwrap_or_default();
    let reasoning = text_of(delta.get("reasoning_content"))
        .or_else(|| text_of(delta.get("reasoning")))
        .unwrap_or_default();
    (content, reasoning)
}

/// 取一个 JSON 值里的文本（字符串或 `[{text}]` 数组）。
fn text_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
        _ => None,
    }
}

/// 从请求体里抽出提示词文本（估算输入 token 用）。
pub fn extract_prompt_text(body: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(text) = text_of(body.get("system")) {
        parts.push(text);
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(text) = text_of(message.get("content")) {
                parts.push(text);
            }
        }
    }
    parts.join("\n")
}

/// 从非流式响应里抽出输出文本（估算输出 token 用）。
pub fn extract_completion_text(response: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(choices) = response.get("choices").and_then(Value::as_array) {
        for choice in choices {
            if let Some(text) = choice.get("message").and_then(|m| text_of(m.get("content"))) {
                parts.push(text);
            }
        }
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::sync::Mutex;

    /// 收集式 sink（单测用，不落库）。
    #[derive(Default)]
    struct CollectingSink {
        records: Mutex<Vec<UsageRecord>>,
    }

    impl UsageSink for CollectingSink {
        fn record(&self, record: UsageRecord) {
            self.records.lock().unwrap().push(record);
        }
    }

    fn ctx() -> UsageContext {
        UsageContext {
            client_key_id: Some("ck1".to_string()),
            channel_id: "c1".to_string(),
            account_id: "acct_default_c1".to_string(),
            model_alias: "alias".to_string(),
            upstream_model: "deepseek-chat".to_string(),
            protocol: PROTOCOL_OPENAI.to_string(),
            prompt_text: "hello".to_string(),
        }
    }

    /// 造一个字节流。
    fn stream_of(chunks: Vec<Vec<u8>>) -> UpstreamStream<std::io::Error> {
        Box::pin(futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok(Bytes::from(c)))
                .collect::<Vec<Result<Bytes, std::io::Error>>>(),
        ))
    }

    /// 造一个 SSE 帧（带空行收尾——真实上游就是这样发的）。
    fn sse(payload: &str) -> Vec<u8> {
        format!("data: {payload}\n\n").into_bytes()
    }

    #[test]
    fn parses_openai_usage() {
        let value = serde_json::json!({
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "prompt_tokens_details": {"cached_tokens": 3},
                "completion_tokens_details": {"reasoning_tokens": 5}
            }
        });
        let usage = parse_usage(&value).expect("应解析出 usage");
        assert_eq!(usage.prompt_tokens, 11);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.reasoning_tokens, 5);
        assert_eq!(usage.total(), 18);
    }

    #[test]
    fn parses_minimax_reasoning_tokens() {
        // 02 §4.2：MiniMax 的 reasoning_tokens 在 completion_tokens_details 里
        let value = serde_json::json!({
            "usage": {"prompt_tokens": 10, "completion_tokens": 20,
                      "completion_tokens_details": {"reasoning_tokens": 16}}
        });
        let usage = parse_usage(&value).expect("应解析出 usage");
        assert_eq!(usage.reasoning_tokens, 16);
    }

    #[test]
    fn parses_anthropic_style_usage() {
        let value = serde_json::json!({
            "usage": {"input_tokens": 9, "output_tokens": 4,
                      "input_tokens_details": {"cached_tokens": 2},
                      "cache_creation_input_tokens": 6}
        });
        let usage = parse_usage(&value).expect("应解析出 usage");
        assert_eq!(usage.prompt_tokens, 9);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.cache_read_tokens, 2);
        assert_eq!(usage.cache_write_tokens, 6);
    }

    #[test]
    fn missing_usage_is_none() {
        assert!(parse_usage(&serde_json::json!({"id": "x"})).is_none());
        assert!(parse_usage(&serde_json::json!({"usage": 5})).is_none());
        assert!(
            parse_usage(&serde_json::json!({"usage": {}})).is_none(),
            "空 usage 对象不算上游回报"
        );
        assert!(
            parse_usage(&serde_json::json!({"usage": {"foo": 1}})).is_none(),
            "没有已知字段的 usage 对象不算上游回报"
        );
    }

    #[test]
    fn sse_scanner_splits_events() {
        let mut scanner = SseScanner::new();
        let out = scanner.push(b"data: {\"a\":1}\n\ndata: {\"b\":2}\n\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "{\"a\":1}");
        assert_eq!(out[1], "{\"b\":2}");
    }

    #[test]
    fn sse_scanner_handles_split_chunks() {
        let mut scanner = SseScanner::new();
        assert!(scanner.push(b"data: {\"a\":").is_empty());
        let out = scanner.push(b"1}\n\n");
        assert_eq!(out, vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn sse_scanner_handles_crlf_and_skips_done() {
        let mut scanner = SseScanner::new();
        let out = scanner.push(b"data: {\"a\":1}\r\n\r\ndata: [DONE]\r\n\r\n");
        assert_eq!(out, vec!["{\"a\":1}".to_string()], "[DONE] 不应被当作负载");
    }

    #[test]
    fn sse_scanner_flushes_trailing_frame() {
        let mut scanner = SseScanner::new();
        assert!(scanner.push(b"data: {\"a\":1}").is_empty());
        assert_eq!(scanner.flush(), vec!["{\"a\":1}".to_string()]);
    }

    #[tokio::test]
    async fn tap_records_ttft_and_estimates_without_upstream_usage() {
        let sink = Arc::new(CollectingSink::default());
        let tap = UsageTap::new(
            stream_of(vec![
                sse(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
                sse(r#"{"choices":[{"delta":{"content":"你好"}}]}"#),
            ]),
            ctx(),
            Arc::new(CharRatioEstimator),
            sink.clone(),
            Instant::now(),
        );
        let collected: Vec<_> = tap.collect::<Vec<_>>().await;
        assert_eq!(collected.len(), 2, "字节应原样转发");

        let records = sink.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.status, UsageStatus::Success);
        assert!(record.estimated, "上游没回 usage → 走估算");
        assert_eq!(record.channel_id, "c1");
        assert_eq!(record.account_id, "acct_default_c1");
        assert_eq!(record.model_alias, "alias");
        assert_eq!(record.upstream_model, "deepseek-chat");
        assert_eq!(record.protocol, PROTOCOL_OPENAI);
        assert!(record.ttft_ms.is_some(), "应记录 TTFT");
        assert_eq!(record.completion_tokens, 2, "「你好」= 2 token");
    }

    #[tokio::test]
    async fn tap_captures_usage_frame() {
        let sink = Arc::new(CollectingSink::default());
        let tap = UsageTap::new(
            stream_of(vec![
                sse(r#"{"choices":[{"delta":{"content":"hi"}}]}"#),
                sse(r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":5}}"#),
            ]),
            ctx(),
            Arc::new(CharRatioEstimator),
            sink.clone(),
            Instant::now(),
        );
        let _ = tap.collect::<Vec<_>>().await;

        let records = sink.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt_tokens, 12);
        assert_eq!(records[0].completion_tokens, 5);
        assert!(!records[0].estimated, "上游给了 usage 就不是估算值");
    }

    #[tokio::test]
    async fn tap_flushes_trailing_frame_without_blank_line() {
        let sink = Arc::new(CollectingSink::default());
        // 上游没用空行收尾：末帧只能靠流结束时冲刷拿到
        let tap = UsageTap::new(
            stream_of(vec![
                br#"data: {"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#.to_vec(),
            ]),
            ctx(),
            Arc::new(CharRatioEstimator),
            sink.clone(),
            Instant::now(),
        );
        let _ = tap.collect::<Vec<_>>().await;

        let records = sink.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt_tokens, 7);
        assert_eq!(records[0].completion_tokens, 3);
        assert!(!records[0].estimated);
    }

    #[tokio::test]
    async fn tap_estimates_when_upstream_has_no_usage() {
        let sink = Arc::new(CollectingSink::default());
        let mut context = ctx();
        context.prompt_text = "hello world".to_string();
        let tap = UsageTap::new(
            stream_of(vec![sse(r#"{"choices":[{"delta":{"content":"abcd"}}]}"#)]),
            context,
            Arc::new(CharRatioEstimator),
            sink.clone(),
            Instant::now(),
        );
        let _ = tap.collect::<Vec<_>>().await;

        let records = sink.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert!(records[0].estimated, "没有 usage 应标估算");
        assert!(records[0].prompt_tokens > 0, "输入应被估算");
        assert!(records[0].completion_tokens > 0, "输出应被估算");
    }

    #[tokio::test]
    async fn tap_marks_upstream_error() {
        let sink = Arc::new(CollectingSink::default());
        let tap = UsageTap::new(
            Box::pin(futures_util::stream::iter(vec![
                Ok(Bytes::from_static(b"data: {}\n\n")),
                Err(std::io::Error::other("boom")),
            ])),
            ctx(),
            Arc::new(CharRatioEstimator),
            sink.clone(),
            Instant::now(),
        );
        let _ = tap.collect::<Vec<_>>().await;

        let records = sink.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, UsageStatus::Error);
        assert_eq!(records[0].error_kind.as_deref(), Some("upstream_error"));
    }

    #[tokio::test]
    async fn tap_marks_aborted_when_dropped_mid_stream() {
        let sink = Arc::new(CollectingSink::default());
        {
            let _tap = UsageTap::new(
                stream_of(vec![sse(r#"{"choices":[{"delta":{"content":"hi"}}]}"#)]),
                ctx(),
                Arc::new(CharRatioEstimator),
                sink.clone(),
                Instant::now(),
            );
            // 直接 drop：模拟客户端断连
        }
        let records = sink.records.lock().unwrap().clone();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, UsageStatus::Aborted);
    }

    #[test]
    fn estimator_counts_cjk_as_one_token() {
        assert_eq!(estimate_tokens("你好"), 2);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn extracts_prompt_text_from_messages() {
        let body = serde_json::json!({
            "system": "你是助手",
            "messages": [
                {"role": "user", "content": "第一句"},
                {"role": "assistant", "content": [{"type": "text", "text": "第二句"}]}
            ]
        });
        let text = extract_prompt_text(&body);
        assert!(text.contains("你是助手"));
        assert!(text.contains("第一句"));
        assert!(text.contains("第二句"));
    }

    #[test]
    fn extracts_completion_text_from_choices() {
        let response = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "答案"}}]
        });
        assert_eq!(extract_completion_text(&response), "答案");
    }
}
