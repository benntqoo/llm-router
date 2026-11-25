//! SSE 流式管道（02 §2.2、§3.3）。
//!
//! - **逐 chunk 直通**：收到即转发，不重组、不缓冲全量；
//! - `[DONE]` 原样透传（不做解析）；
//! - **中途失败**（上游断流）→ 追加一个 SSE `error` 事件后收尾，**不再重试**
//!   （已向客户端写出内容，重试会产生重复内容）；
//! - **客户端断连** → 响应体被 drop → 上游连接随之关闭（避免上游空跑计费）。

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use bytes::Bytes;
use futures_util::Stream;
use serde_json::Value;

/// 格式化为一个 SSE 事件（`data: <json>\n\n`）。
pub fn format_sse_event(value: &Value) -> String {
    format!("data: {value}\n\n")
}

/// 把上游流转成客户端响应体。
///
/// `error_event` 是中途失败时要追加的 SSE 错误事件内容（02 §4.4 的统一错误结构）。
pub fn into_body<E>(stream: UpstreamStream<E>, error_event: Value) -> Body
where
    E: std::error::Error + Send + Sync + 'static,
{
    Body::from_stream(GuardedStream {
        inner: stream,
        error_event: Some(error_event),
    })
}

/// 上游字节流类型（错误类型可参数化，便于测试注入）。
pub type UpstreamStream<E> =
    Pin<Box<dyn Stream<Item = Result<Bytes, E>> + Send>>;

/// 带错误收尾的上游流。
struct GuardedStream<E> {
    inner: UpstreamStream<E>,
    error_event: Option<Value>,
}

impl<E> Stream for GuardedStream<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(bytes))),
            Poll::Ready(Some(Err(err))) => match this.error_event.take() {
                // 上游断流：向客户端补一个 SSE error 事件后收尾
                Some(event) => Poll::Ready(Some(Ok(Bytes::from(format_sse_event(&event))))),
                // 没有兜底事件（理论上不会发生）：以 IO 错误结束
                None => Poll::Ready(Some(Err(std::io::Error::other(err.to_string())))),
            },
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// 构造中途失败时用的 SSE 错误事件（02 §4.4 统一错误结构）。
pub fn error_event(
    message: impl Into<String>,
    kind: &str,
    code: Option<&str>,
    channel: Option<&str>,
    retried: &[String],
) -> Value {
    serde_json::json!({
        "error": {
            "message": message.into(),
            "type": kind,
            "code": code,
            "channel": channel,
            "retried": retried,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个先产出若干 chunk 再失败、然后结束的流。
    ///
    /// 计数器必须走 `unfold` 的**状态**（`u8` 是 Copy，闭包捕获它会每次从初值重来，
    /// 导致流永不终止、读取方 100% CPU 空转）。
    fn failing_stream() -> UpstreamStream<std::io::Error> {
        Box::pin(futures_util::stream::unfold(0u8, move |count| async move {
            let next = count + 1;
            match next {
                1 | 2 => Some((Ok(Bytes::from_static(b"data: ok\n\n")), next)),
                3 => Some((Err(std::io::Error::other("mock upstream broken")), next)),
                // 失败后必须结束，否则消费方会一直空转
                _ => None,
            }
        }))
    }

    #[tokio::test]
    async fn stream_forwards_chunks_then_appends_error_event() {
        let body = into_body(
            failing_stream(),
            error_event("上游中断", "upstream_error", None, Some("c1"), &[]),
        );
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("读取响应体");
        let text = String::from_utf8(bytes.to_vec()).expect("UTF-8");

        assert!(text.starts_with("data: ok\n\n"), "前两帧应原样透传：{text}");
        assert_eq!(text.matches("data: ok\n\n").count(), 2);
        assert!(text.contains("\"type\":\"upstream_error\""), "应补 error 事件：{text}");
        assert!(text.contains("上游中断"));
    }

    #[tokio::test]
    async fn error_event_is_emitted_only_once() {
        let body = into_body(
            failing_stream(),
            error_event("上游中断", "upstream_error", None, None, &[]),
        );
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("读取响应体");
        let text = String::from_utf8(bytes.to_vec()).expect("UTF-8");
        assert_eq!(text.matches("upstream_error").count(), 1);
    }

    #[test]
    fn sse_event_format_matches_spec() {
        let value = serde_json::json!({"error": {"message": "x"}});
        assert_eq!(
            format_sse_event(&value),
            "data: {\"error\":{\"message\":\"x\"}}\n\n"
        );
    }

    #[test]
    fn error_event_shape_follows_section_4_4() {
        let value = error_event("m", "upstream_error", Some("minimax_1004"), Some("c1"), &["a".into()]);
        assert_eq!(value["error"]["message"], "m");
        assert_eq!(value["error"]["type"], "upstream_error");
        assert_eq!(value["error"]["code"], "minimax_1004");
        assert_eq!(value["error"]["channel"], "c1");
        assert_eq!(value["error"]["retried"][0], "a");
    }
}
