//! 出站适配器 trait 契约（02 §6.4）。
//!
//! ```text
//! #[async_trait]
//! pub trait ProviderAdapter: Send + Sync {
//!     fn kind(&self) -> ChannelKind;
//!     async fn complete(&self, ctx: &CallContext, req: OpenAIRequest)
//!         -> Result<OutgoingCall, AdapterError>;           // 返回"已改写"的出站调用
//!     async fn probe(&self) -> Result<ProbeResult, AdapterError>;
//!     async fn refresh(&self) -> Result<(), AdapterError>;
//! }
//! ```

use std::collections::BTreeMap;

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};
use serde::Serialize;

use crate::vault::crypto::SecretBytes;

/// 渠道类型（02 §3.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// API Key 型（DeepSeek/Kimi/GLM/MiniMax/百炼/硅基流动）
    ApiKey,
    /// OAuth 型（GitHub Copilot）
    OAuth,
    /// 登录态型（WorkBuddy）
    Session,
}

impl ChannelKind {
    /// 解析 `channel.kind` 文本（`api_key` / `oauth` / `session`）。
    pub fn parse(raw: &str) -> Option<ChannelKind> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "api_key" | "apikey" => Some(ChannelKind::ApiKey),
            "oauth" => Some(ChannelKind::OAuth),
            "session" => Some(ChannelKind::Session),
            _ => None,
        }
    }

    /// 文本形式（写回 `channel.kind`）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ChannelKind::ApiKey => "api_key",
            ChannelKind::OAuth => "oauth",
            ChannelKind::Session => "session",
        }
    }
}

/// 鉴权样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStyle {
    /// `Authorization: Bearer <key>`
    Bearer,
    /// 自定义头（值由 channel meta 提供，键为头名）
    CustomHeader,
}

impl AuthStyle {
    /// 文本形式（写进 `channel.meta_json.auth_style`）。
    pub fn as_str(self) -> &'static str {
        match self {
            AuthStyle::Bearer => "bearer",
            AuthStyle::CustomHeader => "custom_header",
        }
    }

    /// 解析 `channel.meta_json.auth_style`。
    pub fn parse(raw: &str) -> Option<AuthStyle> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bearer" => Some(AuthStyle::Bearer),
            "custom_header" | "custom-header" => Some(AuthStyle::CustomHeader),
            _ => None,
        }
    }
}

/// 渠道配置：预设 + 渠道行覆盖（02 §9 的 `channel.meta_json`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelConfig {
    /// 渠道 id
    pub channel_id: String,
    /// 预设标识（如 `deepseek`）；自定义渠道为 `custom`
    pub provider: String,
    /// 渠道名（错误体与 `X-Routed-Via` 用）
    pub name: String,
    /// 渠道类型（api_key / oauth / session）
    pub kind: ChannelKind,
    /// 基础地址
    pub base_url: String,
    /// chat 路径模板
    pub chat_path: String,
    /// 鉴权样式
    pub auth_style: AuthStyle,
    /// 自定义鉴权头名（`AuthStyle::CustomHeader` 时必填）
    pub auth_header_name: Option<String>,
    /// MiniMax 国内站 GroupId（02 §4.2）
    pub group_id: Option<String>,
    /// 是否支持 `?GroupId=` 追加
    pub supports_groupid_query: bool,
    /// 是否注入 `stream_options.include_usage`（R-4）
    pub inject_stream_usage: bool,
    /// 额外固定头（如 Copilot 伪装头 R-6）
    pub extra_headers: Vec<(String, String)>,
}

/// 调用上下文。
#[derive(Debug, Clone)]
pub struct CallContext {
    /// 渠道配置
    pub channel: ChannelConfig,
    /// 上游真实模型名（R-1 用）
    pub upstream_model: String,
    /// 凭证明文（短生命周期；由保险库解密得到）
    pub credential: Option<SecretBytes>,
}

/// 入站请求（已解析）。
#[derive(Debug, Clone)]
pub struct OpenAIRequest {
    /// 原始 body（透传用，R-8）
    pub body: serde_json::Value,
    /// 是否流式
    pub stream: bool,
    /// 入站请求头（转发时需剥离 hop-by-hop，R-7）
    pub inbound_headers: Option<HeaderMap>,
}

/// 已改写的出站调用。
#[derive(Debug, Clone)]
pub struct OutgoingCall {
    /// 目标 URL（R-3 + R-5）
    pub url: String,
    /// HTTP 方法
    pub method: Method,
    /// 请求头
    pub headers: HeaderMap,
    /// 请求体（JSON 字节）
    pub body: Vec<u8>,
}

/// 连通性 / 额度探测结果。
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    /// 是否连通
    pub ok: bool,
    /// 剩余额度等（渠道支持时）
    pub quota: Option<serde_json::Value>,
    /// 说明
    pub message: String,
}

/// 适配器错误。
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("渠道配置错误: {0}")]
    Config(String),

    #[error("缺少凭证")]
    MissingCredential,

    #[error("出站请求构造失败: {0}")]
    Build(String),

    #[error("上游错误: {0}")]
    Upstream(String),

    #[error("暂不支持: {0}")]
    Unsupported(String),
}

/// 出站适配器。
#[async_trait::async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// 渠道类型。
    fn kind(&self) -> ChannelKind;

    /// 预设标识。
    fn provider(&self) -> &str;

    /// 构造出站调用（完成 R-1~R-8 改写）。
    async fn complete(
        &self,
        ctx: &CallContext,
        req: OpenAIRequest,
    ) -> Result<OutgoingCall, AdapterError>;

    /// 连通性 / 额度探测。
    async fn probe(&self) -> Result<ProbeResult, AdapterError>;

    /// 凭证刷新（OAuth 型必须实现；其余 no-op）。
    async fn refresh(&self) -> Result<(), AdapterError>;
}

/// 便捷：从 `&[(String, String)]` 构造 `HeaderMap`（跳过非法值）。
pub fn headers_from_pairs(pairs: &[(String, String)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        if let (Ok(n), Ok(v)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(n, v);
        }
    }
    headers
}

/// 便捷：把渠道配置里的额外头转成 `(String, String)` 列表。
pub fn extra_headers_map(config: &ChannelConfig) -> BTreeMap<String, String> {
    config
        .extra_headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect()
}
