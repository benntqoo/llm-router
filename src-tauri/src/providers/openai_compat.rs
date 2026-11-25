//! 通用 OpenAI 兼容出站适配器：实现 02 §4.1 的改写清单 R-1~R-8。
//!
//! | 规则 | 含义 |
//! |---|---|
//! | R-1 | `body.model` 替换为上游真实模型名 |
//! | R-2 | `Authorization: Bearer <凭证>` 替换 |
//! | R-3 | 出站 URL = `base_url` + 渠道路径模板 |
//! | R-4 | 流式时注入 `stream_options.include_usage`（渠道声明支持才注入） |
//! | R-5 | `group_id` 非空且渠道支持时追加 `?GroupId=` |
//! | R-6 | 追加渠道自定义头（如 Copilot 伪装头） |
//! | R-7 | 剥离 hop-by-hop 头（`content-length` 重算） |
//! | R-8 | 不认识的 body 字段一律透传 |
//!
//! v1 是**改写器不是转换器**（02 §4）：不做任何字段级协议转换。

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};

use crate::providers::adapter::{
    AdapterError, AuthStyle, CallContext, ChannelKind, OpenAIRequest, OutgoingCall, ProbeResult,
};
use crate::providers::adapter::ProviderAdapter;

/// 需要剥离的 hop-by-hop 头（R-7）。
pub const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "content-length",
];

/// URL 百分号编码（保留 `A-Za-z0-9-._~`）。
pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 构造出站调用（完成 R-1~R-8）。
pub fn build_outgoing_call(
    ctx: &CallContext,
    req: &OpenAIRequest,
) -> Result<OutgoingCall, AdapterError> {
    let channel = &ctx.channel;

    // ── R-1：替换 model
    let mut body = req.body.clone();
    let Some(map) = body.as_object_mut() else {
        return Err(AdapterError::Build("请求体必须是 JSON 对象".to_string()));
    };
    map.insert(
        "model".to_string(),
        serde_json::Value::String(ctx.upstream_model.clone()),
    );

    // ── R-4：流式时注入 stream_options.include_usage
    // （MiniMax 不注入：其流式自带 usage，多注入会 400；P-4 学习式兼容在 T6 接入）
    if req.stream && channel.inject_stream_usage {
        let entry = map
            .entry("stream_options".to_string())
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let Some(options) = entry.as_object_mut() else {
            return Err(AdapterError::Build(
                "stream_options 必须是 JSON 对象".to_string(),
            ));
        };
        options.insert("include_usage".to_string(), serde_json::Value::Bool(true));
    }

    // ── R-3：出站 URL
    let mut url = format!(
        "{}{}",
        channel.base_url.trim_end_matches('/'),
        channel.chat_path
    );

    // ── R-5：GroupId（仅当非空且渠道声明支持）
    if channel.supports_groupid_query {
        if let Some(group_id) = channel
            .group_id
            .as_ref()
            .map(|g| g.trim())
            .filter(|g| !g.is_empty())
        {
            let separator = if url.contains('?') { '&' } else { '?' };
            url.push(separator);
            url.push_str("GroupId=");
            url.push_str(&url_encode(group_id));
        }
    }

    // ── R-7：剥离 hop-by-hop 头（并剔除将被覆盖的头）
    let mut headers = HeaderMap::new();
    if let Some(inbound) = &req.inbound_headers {
        for (name, value) in inbound.iter() {
            let lower = name.as_str().to_ascii_lowercase();
            if HOP_BY_HOP_HEADERS.contains(&lower.as_str())
                || lower == "host"
                || lower == "authorization"
                || lower == "x-api-key"
            {
                continue;
            }
            headers.append(name.clone(), value.clone());
        }
    }

    // ── R-6：渠道自定义头（如 Copilot 伪装头）
    for (name, value) in &channel.extra_headers {
        if let (Ok(n), Ok(v)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value))
        {
            headers.insert(n, v);
        }
    }

    // ── R-2：鉴权头
    let credential = ctx
        .credential
        .as_ref()
        .map(|c| c.to_string_lossy())
        .ok_or(AdapterError::MissingCredential)?;
    match channel.auth_style {
        AuthStyle::Bearer => {
            headers.insert(
                HeaderName::from_static("authorization"),
                HeaderValue::from_str(&format!("Bearer {credential}"))
                    .map_err(|e| AdapterError::Build(format!("Authorization 头非法: {e}")))?,
            );
        }
        AuthStyle::CustomHeader => {
            let name = channel
                .auth_header_name
                .as_ref()
                .ok_or_else(|| AdapterError::Config("自定义鉴权缺少头名".to_string()))?;
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| AdapterError::Config(format!("鉴权头名非法: {e}")))?;
            headers.insert(
                header_name,
                HeaderValue::from_str(&credential)
                    .map_err(|e| AdapterError::Build(format!("鉴权头值非法: {e}")))?,
            );
        }
    }

    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    // content-length 由 reqwest 按 body 重算（R-7）

    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| AdapterError::Build(format!("请求体序列化失败: {e}")))?;

    Ok(OutgoingCall {
        url,
        method: Method::POST,
        headers,
        body: body_bytes,
    })
}

/// 通用 OpenAI 兼容适配器。
pub struct OpenAiCompatAdapter {
    provider: &'static str,
    kind: ChannelKind,
}

impl OpenAiCompatAdapter {
    /// 按预设标识构造。
    pub fn new(provider: &'static str, kind: ChannelKind) -> Self {
        OpenAiCompatAdapter { provider, kind }
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for OpenAiCompatAdapter {
    fn kind(&self) -> ChannelKind {
        self.kind
    }

    fn provider(&self) -> &str {
        self.provider
    }

    async fn complete(
        &self,
        ctx: &CallContext,
        req: OpenAIRequest,
    ) -> Result<OutgoingCall, AdapterError> {
        build_outgoing_call(ctx, &req)
    }

    async fn probe(&self) -> Result<ProbeResult, AdapterError> {
        // 连通性探测在 T4 之后（需要真实凭证与出站请求）由调用方实现
        Err(AdapterError::Unsupported("probe 尚未实现".to_string()))
    }

    async fn refresh(&self) -> Result<(), AdapterError> {
        // API Key 型无需刷新
        Ok(())
    }
}

/// 判断某个头是否属于 hop-by-hop（供网关侧复用）。
pub fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::ChannelConfig;
    use crate::providers::presets;
    use crate::vault::crypto::SecretBytes;

    fn config() -> ChannelConfig {
        presets::deepseek::preset().to_config("ch1")
    }

    fn ctx(config: ChannelConfig) -> CallContext {
        CallContext {
            channel: config,
            upstream_model: "deepseek-chat".to_string(),
            credential: Some(SecretBytes::from_slice(b"sk-test-key")),
        }
    }

    fn request(body: serde_json::Value, stream: bool) -> OpenAIRequest {
        OpenAIRequest {
            body,
            stream,
            inbound_headers: None,
        }
    }

    #[test]
    fn r1_replaces_model_with_upstream_name() {
        let call = build_outgoing_call(
            &ctx(config()),
            &request(serde_json::json!({"model": "alias-1", "messages": []}), false),
        )
        .expect("构造出站调用");
        let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
        assert_eq!(body["model"], "deepseek-chat");
    }

    #[test]
    fn r2_sets_bearer_authorization() {
        let call = build_outgoing_call(
            &ctx(config()),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(
            call.headers.get("authorization").expect("应有 Authorization"),
            "Bearer sk-test-key"
        );
    }

    #[test]
    fn r2_missing_credential_is_error() {
        let mut ctx = ctx(config());
        ctx.credential = None;
        let err = build_outgoing_call(&ctx, &request(serde_json::json!({"model": "x"}), false))
            .expect_err("缺凭证应报错");
        assert!(matches!(err, AdapterError::MissingCredential));
    }

    #[test]
    fn r3_uses_base_url_plus_channel_path() {
        // GLM 路径与 OpenAI 不同
        let mut config = presets::glm::preset().to_config("ch-glm");
        config.base_url = "https://open.bigmodel.cn".to_string();
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(
            call.url,
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
    }

    #[test]
    fn r3_strips_trailing_slash_of_base_url() {
        let mut config = config();
        config.base_url = "https://api.deepseek.com/".to_string();
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(call.url, "https://api.deepseek.com/v1/chat/completions");
    }

    #[test]
    fn r4_injects_include_usage_when_streaming_and_supported() {
        let call = build_outgoing_call(
            &ctx(config()),
            &request(serde_json::json!({"model": "x", "stream": true}), true),
        )
        .expect("构造出站调用");
        let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
        assert_eq!(body["stream_options"]["include_usage"], serde_json::json!(true));
    }

    #[test]
    fn r4_does_not_inject_for_non_stream_request() {
        let call = build_outgoing_call(
            &ctx(config()),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn r4_not_injected_when_channel_does_not_support() {
        // MiniMax：流式自带 usage，注入多余字段有 400 风险
        let config = presets::minimax_cn::preset().to_config("ch-mm");
        assert!(!config.inject_stream_usage);
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x", "stream": true}), true),
        )
        .expect("构造出站调用");
        let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn r4_preserves_existing_stream_options_keys() {
        let call = build_outgoing_call(
            &ctx(config()),
            &request(
                serde_json::json!({"model": "x", "stream_options": {"custom": 1}}),
                true,
            ),
        )
        .expect("构造出站调用");
        let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
        assert_eq!(body["stream_options"]["custom"], serde_json::json!(1));
        assert_eq!(body["stream_options"]["include_usage"], serde_json::json!(true));
    }

    #[test]
    fn r5_appends_group_id_for_minimax_cn() {
        let mut config = presets::minimax_cn::preset().to_config("ch-mm");
        config.group_id = Some("17895xxxxxxx".to_string());
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(
            call.url,
            "https://api.minimaxi.com/v1/text/chatcompletion_v2?GroupId=17895xxxxxxx"
        );
    }

    #[test]
    fn r5_not_appended_without_group_id() {
        let config = presets::minimax_cn::preset().to_config("ch-mm");
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(
            call.url,
            "https://api.minimaxi.com/v1/text/chatcompletion_v2"
        );
    }

    #[test]
    fn r5_group_id_is_url_encoded_and_not_leaked_into_body_or_headers() {
        let mut config = presets::minimax_cn::preset().to_config("ch-mm");
        config.group_id = Some("a b&c".to_string());
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert!(call.url.ends_with("?GroupId=a%20b%26c"), "实际 {}", call.url);
        let body_text = String::from_utf8(call.body.clone()).expect("body 为 UTF-8");
        assert!(!body_text.contains("GroupId"), "GroupId 不得进 body");
        for (_, value) in call.headers.iter() {
            let text = value.to_str().unwrap_or_default();
            assert!(!text.contains("a b&c"), "GroupId 不得进 header");
        }
    }

    #[test]
    fn r5_not_appended_for_international_profile() {
        let config = presets::minimax_intl::preset().to_config("ch-mm-intl");
        assert!(!config.supports_groupid_query);
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(call.url, "https://api.minimax.io/v1/text/chatcompletion_v2");
    }

    #[test]
    fn r6_applies_channel_extra_headers() {
        let mut config = config();
        config.extra_headers = vec![
            ("editor-version".to_string(), "vscode/1.95.0".to_string()),
            ("copilot-integration-id".to_string(), "vscode-chat".to_string()),
        ];
        let call = build_outgoing_call(
            &ctx(config),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(
            call.headers.get("editor-version").expect("应有 editor-version"),
            "vscode/1.95.0"
        );
        assert_eq!(
            call.headers
                .get("copilot-integration-id")
                .expect("应有 copilot-integration-id"),
            "vscode-chat"
        );
    }

    #[test]
    fn r7_strips_hop_by_hop_headers() {
        let mut inbound = HeaderMap::new();
        inbound.insert("connection", "keep-alive".parse().unwrap());
        inbound.insert("keep-alive", "timeout=5".parse().unwrap());
        inbound.insert("transfer-encoding", "chunked".parse().unwrap());
        inbound.insert("content-length", "123".parse().unwrap());
        inbound.insert("host", "127.0.0.1:8787".parse().unwrap());
        inbound.insert("authorization", "Bearer inbound".parse().unwrap());
        inbound.insert("x-api-key", "llmr_inbound".parse().unwrap());
        inbound.insert("accept", "application/json".parse().unwrap());

        let mut req = request(serde_json::json!({"model": "x"}), false);
        req.inbound_headers = Some(inbound);

        let call = build_outgoing_call(&ctx(config()), &req).expect("构造出站调用");
        for name in HOP_BY_HOP_HEADERS {
            assert!(call.headers.get(*name).is_none(), "{name} 应被剥离");
        }
        assert!(call.headers.get("host").is_none(), "Host 应被剥离");
        // 入站的鉴权头必须被我们自己的替换
        assert_eq!(
            call.headers.get("authorization").expect("Authorization"),
            "Bearer sk-test-key"
        );
        assert!(call.headers.get("x-api-key").is_none());
        // 普通头透传
        assert_eq!(
            call.headers.get("accept").expect("Accept 应透传"),
            "application/json"
        );
    }

    #[test]
    fn r8_passes_through_unknown_body_fields() {
        let body = serde_json::json!({
            "model": "alias",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "f"}}],
            "tool_choice": "auto",
            "temperature": 0.3,
            "reasoning_effort": "high",
            "unknown_future_field": {"x": 1}
        });
        let call = build_outgoing_call(&ctx(config()), &request(body.clone(), false))
            .expect("构造出站调用");
        let out: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
        assert_eq!(out["tools"], body["tools"]);
        assert_eq!(out["tool_choice"], body["tool_choice"]);
        assert_eq!(out["temperature"], body["temperature"]);
        assert_eq!(out["reasoning_effort"], body["reasoning_effort"]);
        assert_eq!(out["unknown_future_field"], body["unknown_future_field"]);
        assert_eq!(out["messages"], body["messages"]);
    }

    #[test]
    fn minimax_url_follows_section_4_2() {
        // 国内站：base_url + /v1/text/chatcompletion_v2
        let call = build_outgoing_call(
            &ctx(presets::minimax_cn::preset().to_config("ch")),
            &request(serde_json::json!({"model": "x"}), false),
        )
        .expect("构造出站调用");
        assert_eq!(
            call.url,
            "https://api.minimaxi.com/v1/text/chatcompletion_v2"
        );
    }

    #[test]
    fn url_encode_keeps_unreserved_chars() {
        assert_eq!(url_encode("abcXYZ019-._~"), "abcXYZ019-._~");
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("/"), "%2F");
    }
}
