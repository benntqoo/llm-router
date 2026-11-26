//! 请求日志中间件（02 §5.2「日志中间件全局脱敏 `Authorization` / `api_key` / `token` 键值」）。
//!
//! 每个入站请求记录一行：方法、路径、状态码，以及**脱敏后**的请求头。
//! 敏感键（`authorization` / `x-api-key` / `api_key` / `token` / `cookie` …）的**值**
//! 一律替换为 `***`（复用 `vault::redact`），因此日志里不会出现凭证明文。
//!
//! 请求体不落日志——入站体里可能带用户数据，也不该出现在本地日志里。

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use serde_json::{Map, Value};

use crate::vault::redact;

/// 把请求头整理成 JSON 对象（同名头合并为数组）。
fn headers_json(req: &Request) -> Value {
    let mut map: Map<String, Value> = Map::new();
    for (name, value) in req.headers().iter() {
        let text = value.to_str().unwrap_or("<非文本>");
        match map.remove(name.as_str()) {
            Some(Value::Array(mut items)) => {
                items.push(Value::String(text.to_string()));
                map.insert(name.as_str().to_string(), Value::Array(items));
            }
            Some(existing) => {
                let first = existing.as_str().unwrap_or("").to_string();
                map.insert(
                    name.as_str().to_string(),
                    Value::Array(vec![Value::String(first), Value::String(text.to_string())]),
                );
            }
            None => {
                map.insert(name.as_str().to_string(), Value::String(text.to_string()));
            }
        }
    }
    Value::Object(map)
}

/// 脱敏后的请求头摘要（供测试断言用）。
fn redacted_headers(req: &Request) -> String {
    let headers = headers_json(req);
    redact::redact(&headers.to_string())
}

/// 请求日志中间件：记录方法、路径、状态码与**脱敏后**的请求头。
pub async fn request_log(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let headers = redacted_headers(&req);
    let response = next.run(req).await;
    println!(
        "[gateway] {method} {path} -> {} headers={headers}",
        response.status()
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with(headers: &[(&str, &str)]) -> Request {
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .body(axum::body::Body::empty())
            .unwrap();
        for (name, value) in headers {
            req.headers_mut().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        req
    }

    #[test]
    fn masks_authorization_value() {
        let req = request_with(&[("authorization", "Bearer llmr_secret_value")]);
        let out = redacted_headers(&req);
        assert!(
            !out.contains("llmr_secret_value"),
            "凭证明文不得出现在日志里：{out}"
        );
        assert!(out.contains("***"), "敏感值应被替换为掩码：{out}");
    }

    #[test]
    fn masks_api_key_and_cookie() {
        let req = request_with(&[("x-api-key", "sk-plain-secret"), ("cookie", "sid=abc")]);
        let out = redacted_headers(&req);
        assert!(!out.contains("sk-plain-secret"), "x-api-key 不得明文：{out}");
        assert!(!out.contains("sid=abc"), "cookie 不得明文：{out}");
    }

    #[test]
    fn keeps_non_sensitive_headers() {
        let req = request_with(&[("content-type", "application/json")]);
        let out = redacted_headers(&req);
        assert!(out.contains("application/json"), "非敏感头应保留：{out}");
    }
}
