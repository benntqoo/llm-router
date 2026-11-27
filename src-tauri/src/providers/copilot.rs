//! GitHub Copilot 出站适配器（T21；02 §6.2.2）。
//!
//! **三件事**：① R-6 伪装头（02 §4.1）；② 出站 URL 与鉴权（**Bearer = Copilot token**，
//! **不是** GitHub token）；③ 模型目录拉取与解析（`GET {api_base}/models`）。
//!
//! **凭证供给口径（本卡最关键的一条）**：**OAuth 型渠道的凭证由 adapter 自己供给**——
//! 网关**不解密** OAuth 凭证（`CallContext.credential = None`），出站前由本模块调
//! [`oauth::copilot::ensure_token`] 取一个**新鲜的** Copilot token（过期前 300 s 自动重取）。
//!
//! **为什么不能沿用网关的「按 `credential.kind` 选一条」**：GitHub token（`ghu_`，长期）
//! 与 Copilot token（~30 min）**不能互换**——前者只能用来换后者。沿用旧口径，Copilot
//! token 一过期就会**误取 GitHub token** 当 Bearer → 必然 401。详见 02 §6.2.2。
//!
//! **合规**：走 GitHub **官方** device flow；**不做**指纹伪造 / 验证码绕过 / 签名伪造
//! （AGENT.md §5.7）→ **不实现 `machine-id` 头**（设备指纹）。
//!
//! **v1 是改写器不是转换器**（02 §5.6）：本模块**复用** `openai_compat::build_outgoing_call`
//! 完成 R-1~R-8，**只**额外补一个每次请求动态的 `x-request-id`——**不做**任何字段级协议转换。

use std::sync::{Arc, RwLock};

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use rand::RngCore;

use crate::providers::adapter::{
    AdapterError, AuthStyle, CallContext, ChannelConfig, ChannelKind, OpenAIRequest, OutgoingCall,
    ProbeResult, ProviderAdapter,
};
use crate::providers::oauth::copilot::{self, Endpoints, TokenCache};
use crate::providers::openai_compat;
use crate::vault::crypto::SecretBytes;
use crate::vault::Vault;

/// Copilot 的 `channel.provider` 标识（**唯一真值**）。
pub const PROVIDER: &str = "copilot";

/// 出站 chat 路径（**不带 `/v1`**）——liter-llm 与 npm 参考实现一致。
pub const CHAT_PATH: &str = "/chat/completions";
/// 模型目录路径。
pub const MODELS_PATH: &str = "/models";

// ── R-6 伪装头（02 §6.2.2；取值来源逐条列在 02 的来源表里）──

/// `copilot-integration-id` 的值。
pub const COPILOT_INTEGRATION_ID: &str = "vscode-chat";
/// `editor-version` 的值（**常量**——版本串变了改这里即可，不用动协议）。
pub const EDITOR_VERSION: &str = "vscode/1.104.1";
/// `editor-plugin-version` 的值。
pub const EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.26.7";
/// `user-agent` 的值（与 T20 授权请求用的 UA 同串）。
pub const USER_AGENT: &str = "GitHubCopilotChat/0.26.7";

/// `x-request-id` 的头名（**每次请求动态**，值 = UUID v4）。
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// R-6 的**四项固定头**（进 `ChannelConfig.extra_headers`）。
///
/// **不包含** `x-request-id`——它是**每次请求都变**的动态头，由 adapter 在构造
/// 出站调用时补（静态头放不进 `extra_headers`）。
pub fn r6_headers() -> Vec<(String, String)> {
    vec![
        (
            "copilot-integration-id".to_string(),
            COPILOT_INTEGRATION_ID.to_string(),
        ),
        ("editor-version".to_string(), EDITOR_VERSION.to_string()),
        (
            "editor-plugin-version".to_string(),
            EDITOR_PLUGIN_VERSION.to_string(),
        ),
        ("user-agent".to_string(), USER_AGENT.to_string()),
    ]
}

/// 生成一个 UUID v4（`x-request-id` 用）。
///
/// **为什么自己生成**：仓库里没有 `uuid` crate；`rand` 已够（0.9 的 `rand::rng()`）。
pub fn new_request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    // version 4
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    // variant 1（RFC 4122）
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].join(""),
        hex[4..6].join(""),
        hex[6..8].join(""),
        hex[8..10].join(""),
        hex[10..16].join("")
    )
}

/// Copilot 出站会话（**凭证供给 + 端点 + 缓存**三合一）。
///
/// - **生产**：真实 GitHub 端点 + **全局** token 缓存（跨请求复用——否则每次出站
///   都要重换 token，既慢又容易撞限流）；
/// - **测试**：**本地 mock 端点 + 独立缓存**（不出网，且并行用例互不污染）。
#[derive(Clone)]
pub struct CopilotSession {
    /// 加密保险库（取 / 刷凭证）。
    pub vault: Arc<RwLock<Vault>>,
    /// OAuth 端点（① ③ ④）。
    pub endpoints: Endpoints,
    /// Copilot token 缓存。
    pub cache: Arc<TokenCache>,
}

impl CopilotSession {
    /// 生产默认：真实 GitHub 端点 + **全局** token 缓存。
    pub fn production(vault: Arc<RwLock<Vault>>) -> Self {
        Self {
            vault,
            endpoints: Endpoints::default(),
            cache: copilot::cache().clone(),
        }
    }

    /// 测试用：端点可注入 + 独立缓存。
    pub fn testing(
        vault: Arc<RwLock<Vault>>,
        endpoints: Endpoints,
        cache: Arc<TokenCache>,
    ) -> Self {
        Self {
            vault,
            endpoints,
            cache,
        }
    }
}

/// Copilot 出站适配器（OAuth 型）。
pub struct CopilotAdapter {
    /// 会话（凭证供给 + 端点 + 缓存）。
    session: CopilotSession,
}

impl CopilotAdapter {
    /// 构造。
    pub fn new(session: CopilotSession) -> Self {
        Self { session }
    }

    /// 取一个**可用的** Copilot token（缓存命中就用，否则用 GitHub token 重取）。
    ///
    /// **只拿 Copilot token**——GitHub token（`ghu_`）当 Bearer 给上游会直接 401。
    async fn ensure_copilot_token(
        &self,
        channel_id: &str,
    ) -> Result<copilot::CopilotToken, AdapterError> {
        copilot::ensure_token(
            &self.session.endpoints,
            &self.session.vault,
            channel_id,
            &self.session.cache,
        )
        .await
        .map_err(|err| AdapterError::Upstream(format!("Copilot 凭证不可用：{err}")))
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for CopilotAdapter {
    fn kind(&self) -> ChannelKind {
        ChannelKind::OAuth
    }

    fn provider(&self) -> &str {
        PROVIDER
    }

    async fn complete(
        &self,
        ctx: &CallContext,
        req: OpenAIRequest,
    ) -> Result<OutgoingCall, AdapterError> {
        // ── ① 凭证：OAuth 型**由 adapter 自己供给**（网关不解密）
        let token = self.ensure_copilot_token(&ctx.channel.channel_id).await?;

        // ── ② 用 Copilot token 替换 `CallContext.credential`，其余交给通用改写器
        let mut scoped = ctx.clone();
        scoped.credential = Some(SecretBytes::from_slice(token.token.as_bytes()));
        // 出站域名以 **④ 响应里的 `endpoints.api`** 为准（渠道行存的也是它，
        // 但 token 刚换过时它一定是最新的）→ 渠道行缺失时也能拿到正确域名。
        if !token.api_base.trim().is_empty() {
            scoped.channel.base_url = token.api_base.trim_end_matches('/').to_string();
        }

        // R-1~R-8（复用通用改写器：**改写器不是转换器**，不重复实现）
        let mut call = openai_compat::build_outgoing_call(&scoped, &req)?;

        // ── ③ R-6 动态头：`x-request-id`（每次请求都不同）
        attach_request_id(&mut call.headers)?;
        Ok(call)
    }

    async fn probe(&self) -> Result<ProbeResult, AdapterError> {
        // 连通性探测需要**渠道上下文**（取 token + 打到该账号的 `endpoints.api`），
        // 而 trait 的 `probe(&self)` **拿不到渠道** → 探测走模型目录那条路
        // （`GET {api_base}/models`），由 `fetch_models` 承担。
        Err(AdapterError::Unsupported(
            "Copilot 探测请用「刷新模型目录」（它同时验证凭证与出站域名）".to_string(),
        ))
    }

    async fn refresh(&self) -> Result<(), AdapterError> {
        // 凭证刷新**发生在 `complete()` 内按需进行**（`ensure_token` 自带续期：
        // 缓存命中且离过期 ≥ 300 s 就用，否则重取）。
        // trait 的 `refresh(&self)` **拿不到渠道 id** → 无法单独换某条渠道的 token，
        // 故这里保持 no-op（不是「忘了实现」）。
        Ok(())
    }
}

/// Copilot 渠道的出站配置（**不走 `presets`**——预设表是 7 个 API Key profile）。
///
/// - `chat_path` = `/chat/completions`（**不带 `/v1`**）；
/// - `kind` = OAuth、`auth_style` = Bearer；
/// - `inject_stream_usage` = **true**（Copilot 声明支持 `stream_options.include_usage`）；
/// - `extra_headers` = R-6 四项固定头。
pub fn config_from_row(row: &crate::providers::ChannelRow) -> ChannelConfig {
    ChannelConfig {
        channel_id: row.id.clone(),
        provider: PROVIDER.to_string(),
        name: row.name.clone(),
        kind: ChannelKind::OAuth,
        base_url: row.base_url.clone(),
        chat_path: CHAT_PATH.to_string(),
        auth_style: AuthStyle::Bearer,
        auth_header_name: None,
        group_id: None,
        supports_groupid_query: false,
        inject_stream_usage: true,
        extra_headers: r6_headers(),
    }
}

/// 上游模型目录里的一条模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotModel {
    /// 上游真实模型名（R-1 的替换目标）**兼**对外别名。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 上下文长度（`capabilities.limits.max_context_window_tokens`）。
    pub context_len: Option<i64>,
    /// 单次输出上限（`capabilities.limits.max_output_tokens`）。
    pub max_output_tokens: Option<i64>,
    /// 是否支持工具调用。
    pub supports_tool_calls: Option<bool>,
    /// 是否支持图片输入。
    pub supports_vision: Option<bool>,
    /// 是否支持流式。
    pub supports_streaming: Option<bool>,
    /// 模型家族（`capabilities.family`）。
    pub family: Option<String>,
    /// 厂商（`vendor`）。
    pub vendor: Option<String>,
}

/// 解析上游模型目录（`GET {api_base}/models` 的响应体）。
///
/// **收录规则**（02 §6.2.2；**按真实上游响应核实过**，2026-11-27 真机实测 39 条）：
/// - `policy.state == "disabled"` → **不收录**（策略禁用）；
/// - `capabilities.type` **存在且不是 `chat`** → **不收录**（`embeddings` / `decisions`
///   这些模型走的是别的端点，v1 **不做协议转换**，收录了也打不通）；
/// - `supported_endpoints` **存在且不含 chat 端点** → **不收录**。
///
/// **端点字段的两种形状都要认**（这是真机实测踩到的坑）：
/// - **真实上游给的是路径**：`/chat/completions`（另有 `/responses`、`/responses/ws:`、
///   `/v1/decisions` 等）；
/// - 社区 schema（opencode）里是短名：`chat`。
/// → 两种都算「支持 chat」。
///
/// **字段缺失一律容忍**：`supported_endpoints` 缺失（老模型如 `gpt-4o` 家族没有这个字段）
/// → **按 chat 收**（它们的 `capabilities.type` 是 `chat`，且真机实测能对话）。
pub fn parse_models(body: &str) -> Result<Vec<CopilotModel>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|err| format!("解析模型目录失败：{err}"))?;
    let Some(data) = value.get("data").and_then(|item| item.as_array()) else {
        return Err("模型目录响应里没有 data 数组".to_string());
    };
    let mut out = Vec::new();
    for item in data {
        let Some(id) = item
            .get("id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };

        // ① 策略禁用 → 不收录
        if item
            .get("policy")
            .and_then(|policy| policy.get("state"))
            .and_then(|state| state.as_str())
            == Some("disabled")
        {
            continue;
        }

        let capabilities = item.get("capabilities");
        let supports = capabilities.and_then(|caps| caps.get("supports"));
        let limits = capabilities.and_then(|caps| caps.get("limits"));

        // ② 类型不是 chat（embeddings / decisions …）→ 不收录
        if let Some(model_type) = capabilities
            .and_then(|caps| caps.get("type"))
            .and_then(|value| value.as_str())
        {
            if model_type != "chat" {
                continue;
            }
        }

        // ③ 端点过滤：**含** `supported_endpoints` 但没有 chat 端点 → 不收录
        if let Some(endpoints) = item.get("supported_endpoints").and_then(|value| value.as_array())
        {
            let supports_chat = endpoints
                .iter()
                .filter_map(|value| value.as_str())
                .any(supports_chat_endpoint);
            if !supports_chat {
                continue;
            }
        }

        out.push(CopilotModel {
            id: id.to_string(),
            name: item
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or(id)
                .to_string(),
            context_len: limits.and_then(|limits| number(limits, "max_context_window_tokens")),
            max_output_tokens: limits.and_then(|limits| number(limits, "max_output_tokens")),
            supports_tool_calls: supports.and_then(|s| flag(s, "tool_calls")),
            supports_vision: supports.and_then(|s| flag(s, "vision")),
            supports_streaming: supports.and_then(|s| flag(s, "streaming")),
            family: capabilities
                .and_then(|caps| caps.get("family"))
                .and_then(|value| value.as_str())
                .map(str::to_string),
            vendor: item
                .get("vendor")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        });
    }
    Ok(out)
}

/// 一个端点是不是 **chat completions**（两种形状都认）。
///
/// - **真实上游**：路径形式（`/chat/completions`）——实测 2026-11-27；
/// - 社区 schema：短名形式（`chat`）。
fn supports_chat_endpoint(endpoint: &str) -> bool {
    let value = endpoint.trim().trim_end_matches('/').to_ascii_lowercase();
    value == "chat" || value.contains("chat/completions")
}

/// 取一个整数（**兼容 float**——上游偶尔给 `264000.0`）。
fn number(value: &serde_json::Value, key: &str) -> Option<i64> {
    let raw = value.get(key)?;
    raw.as_i64()
        .or_else(|| raw.as_f64().map(|float| float as i64))
}

/// 取一个布尔开关。
fn flag(value: &serde_json::Value, key: &str) -> Option<bool> {
    value.get(key).and_then(|raw| raw.as_bool())
}

/// 拉取上游模型目录（`GET {api_base}/models`，Bearer + R-6 头）。
///
/// **域名取 ④ 响应里的 `endpoints.api`**（`ensure_token` 返回的 `api_base`），
/// **不是** GitHub OAuth 域名。
pub async fn fetch_models(
    session: &CopilotSession,
    channel_id: &str,
) -> Result<Vec<CopilotModel>, String> {
    let raw = fetch_models_raw(session, channel_id).await?;
    parse_models(&raw)
}

/// 拉取上游模型目录的**原始响应体**（供排障：核对上游到底给了哪些字段）。
pub async fn fetch_models_raw(
    session: &CopilotSession,
    channel_id: &str,
) -> Result<String, String> {
    let token = copilot::ensure_token(
        &session.endpoints,
        &session.vault,
        channel_id,
        &session.cache,
    )
    .await?;
    let url = format!("{}{}", token.api_base.trim_end_matches('/'), MODELS_PATH);
    let client = api_client()?;
    let response = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token.token),
        )
        .header("copilot-integration-id", COPILOT_INTEGRATION_ID)
        .header("editor-version", EDITOR_VERSION)
        .header("editor-plugin-version", EDITOR_PLUGIN_VERSION)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(REQUEST_ID_HEADER, new_request_id())
        .send()
        .await
        .map_err(|err| format!("拉取模型目录失败：{err}"))?;
    let status = response.status();
    if !status.is_success() {
        // **不回显响应体**（可能含敏感信息），只报状态码
        return Err(format!("拉取模型目录失败：HTTP {}", status.as_u16()));
    }
    response
        .text()
        .await
        .map_err(|err| format!("读取模型目录响应失败：{err}"))
}

/// 出站 API 的 HTTP 客户端（**普通 DNS**——不钉 github.com 的候选 IP）。
///
/// connect 5 s / 整请求 15 s：模型目录是低频请求，不必像 OAuth 那样紧。
fn api_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|err| format!("构造 HTTP 客户端失败：{err}"))
}

/// 给**已构造好的**出站调用补上 R-6 的动态头（`x-request-id`）。
///
/// 单独抽出来是为了**测试**能直接断言「同一个请求两次调用 → 两个不同的 id」。
pub fn attach_request_id(headers: &mut HeaderMap) -> Result<(), AdapterError> {
    let value = HeaderValue::from_str(&new_request_id())
        .map_err(|err| AdapterError::Build(format!("x-request-id 非法：{err}")))?;
    headers.insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r6_headers_carry_the_four_fixed_values() {
        let headers = r6_headers();
        assert_eq!(
            headers,
            vec![
                (
                    "copilot-integration-id".to_string(),
                    "vscode-chat".to_string()
                ),
                ("editor-version".to_string(), "vscode/1.104.1".to_string()),
                (
                    "editor-plugin-version".to_string(),
                    "copilot-chat/0.26.7".to_string()
                ),
                ("user-agent".to_string(), "GitHubCopilotChat/0.26.7".to_string()),
            ]
        );
        // **不包含**动态头（它每次请求都变）
        assert!(
            !headers.iter().any(|(name, _)| name == REQUEST_ID_HEADER),
            "x-request-id 是动态头，不进固定头表"
        );
    }

    #[test]
    fn request_id_is_a_uuid_v4_and_changes_every_time() {
        let first = new_request_id();
        let second = new_request_id();
        assert_ne!(first, second, "每次请求都该是新的 id");
        for id in [&first, &second] {
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.len(), 5, "UUID 形状不对：{id}");
            assert_eq!(parts[0].len(), 8);
            assert_eq!(parts[1].len(), 4);
            assert_eq!(parts[2].len(), 4);
            assert_eq!(parts[3].len(), 4);
            assert_eq!(parts[4].len(), 12);
            assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            assert!(parts[2].starts_with('4'), "version 4：{id}");
        }
    }

    #[test]
    fn attach_request_id_writes_the_header() {
        let mut headers = HeaderMap::new();
        attach_request_id(&mut headers).expect("补动态头");
        let value = headers.get(REQUEST_ID_HEADER).expect("应有 x-request-id");
        assert_eq!(value.to_str().expect("头值应是文本").len(), 36);
    }

    #[test]
    fn the_config_carries_the_copilot_specifics() {
        let row = crate::providers::ChannelRow {
            id: "ch-copilot".to_string(),
            name: "GitHub Copilot".to_string(),
            kind: "oauth".to_string(),
            provider: PROVIDER.to_string(),
            base_url: "https://api.individual.githubcopilot.com".to_string(),
            meta_json: None,
        };
        let config = config_from_row(&row);
        assert_eq!(config.kind, ChannelKind::OAuth);
        assert_eq!(config.auth_style, AuthStyle::Bearer);
        assert_eq!(config.chat_path, "/chat/completions");
        assert!(config.inject_stream_usage);
        assert!(!config.supports_groupid_query);
        assert_eq!(config.extra_headers.len(), 4);
        assert_eq!(config.base_url, "https://api.individual.githubcopilot.com");
    }

    #[test]
    fn models_are_parsed_from_the_openai_style_list() {
        let body = r#"{
          "data": [
            {
              "id": "gpt-5-mini",
              "name": "GPT-5 mini",
              "object": "model",
              "policy": { "state": "enabled" },
              "supported_endpoints": ["chat"],
              "vendor": "Azure OpenAI",
              "capabilities": {
                "family": "gpt-5-mini",
                "type": "chat",
                "limits": { "max_context_window_tokens": 264000, "max_output_tokens": 64000 },
                "supports": { "tool_calls": true, "streaming": true, "vision": false }
              }
            },
            { "id": "no-capabilities-model" }
          ],
          "object": "list"
        }"#;
        let models = parse_models(body).expect("解析模型目录");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5-mini");
        assert_eq!(models[0].name, "GPT-5 mini");
        assert_eq!(models[0].context_len, Some(264000));
        assert_eq!(models[0].max_output_tokens, Some(64000));
        assert_eq!(models[0].supports_tool_calls, Some(true));
        assert_eq!(models[0].supports_vision, Some(false));
        assert_eq!(models[0].family.as_deref(), Some("gpt-5-mini"));
        assert_eq!(models[0].vendor.as_deref(), Some("Azure OpenAI"));
        // 缺字段的模型**照样收录**（字段缺失一律容忍）
        assert_eq!(models[1].id, "no-capabilities-model");
        assert_eq!(models[1].name, "no-capabilities-model");
        assert_eq!(models[1].context_len, None);
    }

    #[test]
    fn disabled_models_are_not_collected() {
        let body = r#"{"data":[
            {"id":"blocked","policy":{"state":"disabled"}},
            {"id":"allowed","policy":{"state":"enabled"}}
        ]}"#;
        let models = parse_models(body).expect("解析模型目录");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "allowed");
    }

    #[test]
    fn models_without_the_chat_endpoint_are_not_collected() {
        // codex 类只支持 `/responses` → v1 不做协议转换 → 不收录
        let body = r#"{"data":[
            {"id":"gpt-5.1-codex","supported_endpoints":["responses"]},
            {"id":"chat-model","supported_endpoints":["chat","responses"]}
        ]}"#;
        let models = parse_models(body).expect("解析模型目录");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "chat-model");
    }

    #[test]
    fn a_body_without_data_is_an_error() {
        assert!(parse_models(r#"{"object":"list"}"#).is_err());
        assert!(parse_models("not json").is_err());
    }

    #[test]
    fn the_real_upstream_shapes_are_filtered_correctly() {
        // **真机实测 2026-11-27**：`supported_endpoints` 给的是**路径**
        // （`/chat/completions`、`/responses`、`/responses/ws:`），
        // `capabilities.type` 区分 `chat` / `embeddings` / `decisions`。
        let body = r#"{"data":[
            {"id":"chat-path","capabilities":{"type":"chat"},"supported_endpoints":["/chat/completions"]},
            {"id":"chat-and-responses","capabilities":{"type":"chat"},"supported_endpoints":["/chat/completions","/responses"]},
            {"id":"responses-only","capabilities":{"type":"chat"},"supported_endpoints":["/responses"]},
            {"id":"ws-only","capabilities":{"type":"chat"},"supported_endpoints":["/responses/ws:","/responses"]},
            {"id":"decisions","capabilities":{"type":"decisions"},"supported_endpoints":["/v1/decisions"]},
            {"id":"embedding","capabilities":{"type":"embeddings"}},
            {"id":"legacy-chat","capabilities":{"type":"chat"}},
            {"id":"no-type-no-endpoints"}
        ]}"#;
        let models = parse_models(body).expect("解析模型目录");
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["chat-path", "chat-and-responses", "legacy-chat", "no-type-no-endpoints"],
            "只收 chat 类型 + 有 chat 端点（或端点字段缺失）的模型"
        );
    }
}
