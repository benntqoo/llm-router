//! 入站 OpenAI 协议解析（v1 唯一入站协议，02 §0.1）。
//!
//! v1 是**改写器不是转换器**（02 §4）：body 只做 JSON 级透传，
//! 这里仅提取 `model` 与 `stream`，其余字段原样保留（R-8）。

use serde::Serialize;
use serde_json::{json, Value};

use crate::gateway::errors::GatewayError;

/// 解析后的 chat 请求。
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// 对外别名（路由用）。
    pub model: String,
    /// 是否流式。
    pub stream: bool,
    /// 原始 body（透传给上游，仅替换 `model`）。
    pub raw: Value,
}

/// 解析 chat 请求体。
pub fn parse_chat_request(body: Value) -> Result<ChatRequest, GatewayError> {
    let map = match &body {
        Value::Object(map) => map,
        _ => {
            return Err(GatewayError::invalid_request(
                "请求体必须是 JSON 对象",
            ));
        }
    };
    let model = match map.get("model") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::String(_)) | None => {
            return Err(GatewayError::invalid_request("缺少 model 字段"));
        }
        Some(_) => {
            return Err(GatewayError::invalid_request("model 字段必须是字符串"));
        }
    };
    let stream = map
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(ChatRequest { model, stream, raw: body })
}

/// `/v1/models` 响应条目（OpenAI 兼容）。
#[derive(Debug, Clone, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub owned_by: String,
}

/// `/v1/models` 响应体。
#[derive(Debug, Clone, Serialize)]
pub struct ModelList {
    pub object: &'static str,
    pub data: Vec<ModelEntry>,
}

impl ModelList {
    /// 构造空列表。
    pub fn empty() -> Self {
        ModelList {
            object: "list",
            data: Vec::new(),
        }
    }

    /// 从别名列表构造。
    pub fn from_entries(entries: Vec<ModelEntry>) -> Self {
        ModelList {
            object: "list",
            data: entries,
        }
    }
}

/// 供诊断使用的错误示例（文档/测试用）。
pub fn example_error_body(message: &str) -> Value {
    json!({
        "error": {
            "message": message,
            "type": "invalid_request_error",
            "code": null,
            "channel": null,
            "retried": []
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_and_stream() {
        let body = json!({"model": "deepseek-chat", "stream": true, "temperature": 0.7});
        let req = parse_chat_request(body).expect("应解析成功");
        assert_eq!(req.model, "deepseek-chat");
        assert!(req.stream);
        // 其余字段原样保留（R-8）
        assert_eq!(req.raw["temperature"], json!(0.7));
    }

    #[test]
    fn missing_model_is_invalid_request() {
        let err = parse_chat_request(json!({"messages": []})).expect_err("缺 model 应报错");
        assert_eq!(
            err.error.kind,
            crate::gateway::errors::ErrorKind::InvalidRequestError
        );
    }

    #[test]
    fn non_object_body_is_invalid_request() {
        assert!(parse_chat_request(json!([])).is_err());
        assert!(parse_chat_request(json!("hello")).is_err());
    }

    #[test]
    fn empty_model_is_rejected() {
        assert!(parse_chat_request(json!({"model": "  "})).is_err());
    }

    #[test]
    fn empty_model_list_shape() {
        let list = ModelList::empty();
        let value = serde_json::to_value(&list).expect("序列化");
        assert_eq!(value["object"], "list");
        assert!(value["data"].as_array().expect("data 应为数组").is_empty());
    }
}
