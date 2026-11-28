//! 出站适配器（02 §4.1 / §6）。
//!
//! 结构：
//! - [`adapter`]：trait 契约与共用类型（`ChannelConfig` / `CallContext` / `OutgoingCall` …）
//! - [`openai_compat`]：唯一通用实现，完成 R-1~R-8 改写
//! - [`presets`]：纯数据预设（02 §6.1 的 7 个 API Key profile）
//! - [`oauth`]：OAuth 型渠道的**凭证获取**（T20：GitHub Copilot；只取凭证，不出站）
//! - [`copilot`]：Copilot **出站适配**（T21：R-6 伪装头 + Copilot token 供给 + 模型目录）
//! - [`compliance`]：合规确认与受限渠道的**启用闸门**（T24：ToS 声明 + 启用前校验）
//!
//! v1 入站仅 OpenAI 协议，出站亦为 OpenAI 兼容改写；**v1 是改写器不是转换器**。

pub mod adapter;
pub mod compliance;
pub mod copilot;
pub mod oauth;
pub mod openai_compat;
pub mod presets;

use serde_json::Value;

use crate::providers::adapter::{AdapterError, AuthStyle, ChannelConfig, ChannelKind, ProviderAdapter};
use crate::providers::copilot::CopilotSession;
use crate::providers::openai_compat::OpenAiCompatAdapter;

/// 渠道表的一行（`channel` 表，02 §9）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    /// 渠道 id
    pub id: String,
    /// 渠道名（`X-Routed-Via` 与错误体用）
    pub name: String,
    /// 渠道类型：`api_key` / `oauth` / `session`
    pub kind: String,
    /// 预设标识（`deepseek` … ；自定义渠道为 `custom`）
    pub provider: String,
    /// 基础地址
    pub base_url: String,
    /// 渠道私有参数（JSON 文本，可能为 NULL）
    pub meta_json: Option<String>,
}

/// 自定义渠道的默认 chat 路径（无预设时的兜底）。
pub const DEFAULT_CHAT_PATH: &str = "/v1/chat/completions";

/// 适配器构造依赖（**目前只有 OAuth 型渠道需要**：凭证供给 + 端点 + 缓存）。
///
/// **为什么要有它**：OAuth 型渠道的凭证**由 adapter 自己供给**（02 §6.2.2）→
/// adapter 得拿到**保险库**（取 / 刷凭证）、**OAuth 端点**与 **token 缓存**。
///
/// - **生产**：真实 GitHub 端点 + **全局** token 缓存（跨请求复用）；
/// - **测试**：本地 mock 端点 + **独立**缓存（不出网，并行用例互不污染）。
#[derive(Clone)]
pub struct AdapterDeps {
    /// Copilot 会话（OAuth 型渠道的凭证供给）。
    pub copilot: CopilotSession,
}

/// 按 `provider` 取适配器。
///
/// - 7 个 v1 预设 → 通用 OpenAI 兼容适配器
/// - `custom` → 同样是通用适配器（自定义渠道即"填表"出来的 OpenAI 兼容渠道）
/// - `copilot` → **Copilot 专用适配器**（T21：R-6 伪装头 + Copilot token 供给）
/// - 其余（如 `workbuddy`）→ `None`（各自实现 trait，不在 M1 范围）
pub fn adapter_for(
    provider: &str,
    deps: &AdapterDeps,
) -> Option<Box<dyn ProviderAdapter>> {
    match presets::find(provider) {
        Some(preset) => Some(Box::new(OpenAiCompatAdapter::new(
            preset.provider,
            preset.kind,
        ))),
        None if provider == "custom" => Some(Box::new(OpenAiCompatAdapter::new(
            "custom",
            ChannelKind::ApiKey,
        ))),
        None if provider == copilot::PROVIDER => Some(Box::new(copilot::CopilotAdapter::new(
            deps.copilot.clone(),
        ))),
        None => None,
    }
}

/// 按渠道行取适配器（自定义渠道按行上的 `kind` 推断）。
pub fn adapter_for_row(
    row: &ChannelRow,
    deps: &AdapterDeps,
) -> Option<Box<dyn ProviderAdapter>> {
    if let Some(adapter) = adapter_for(&row.provider, deps) {
        return Some(adapter);
    }
    // 未知 provider：只有在能识别为 API Key 型时才退回通用适配器
    if ChannelKind::parse(&row.kind) == Some(ChannelKind::ApiKey) {
        return Some(Box::new(OpenAiCompatAdapter::new("custom", ChannelKind::ApiKey)));
    }
    None
}

/// 由渠道行构造出站配置：**预设默认值 + 渠道行覆盖 + `meta_json` 覆盖**。
///
/// `channel.meta_json` 约定字段（02 §9）：
/// ```json
/// { "group_id": "..." | null, "supports_groupid_query": true|false,
///   "path_chat": "/v1/text/chatcompletion_v2", "inject_stream_usage": false }
/// ```
pub fn config_from_row(row: &ChannelRow) -> Result<ChannelConfig, AdapterError> {
    let mut config = match presets::find(&row.provider) {
        Some(preset) => preset.to_config(&row.id),
        // Copilot（OAuth 型）：**不进预设表**（预设是 7 个 API Key profile），
        // 走专属配置（`/chat/completions` + R-6 四项固定头）。
        None if row.provider == copilot::PROVIDER => copilot::config_from_row(row),
        None => ChannelConfig {
            channel_id: row.id.clone(),
            provider: row.provider.clone(),
            name: row.name.clone(),
            kind: ChannelKind::ApiKey,
            base_url: row.base_url.clone(),
            chat_path: DEFAULT_CHAT_PATH.to_string(),
            auth_style: AuthStyle::Bearer,
            auth_header_name: None,
            group_id: None,
            supports_groupid_query: false,
            inject_stream_usage: true,
            extra_headers: Vec::new(),
        },
    };

    // 渠道行的名称与 base_url 优先于预设默认值
    if !row.name.is_empty() {
        config.name = row.name.clone();
    }
    if !row.base_url.is_empty() {
        config.base_url = row.base_url.clone();
    }
    if let Some(kind) = ChannelKind::parse(&row.kind) {
        config.kind = kind;
    }

    apply_meta_overrides(&mut config, row.meta_json.as_deref())?;
    Ok(config)
}

/// 把 `meta_json` 的约定字段覆盖到配置上。
fn apply_meta_overrides(
    config: &mut ChannelConfig,
    meta_json: Option<&str>,
) -> Result<(), AdapterError> {
    let Some(raw) = meta_json else {
        return Ok(());
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let meta: Value = serde_json::from_str(trimmed).map_err(|e| {
        AdapterError::Config(format!("channel.meta_json 不是合法 JSON: {e}"))
    })?;
    let Some(map) = meta.as_object() else {
        return Err(AdapterError::Config(
            "channel.meta_json 必须是 JSON 对象".to_string(),
        ));
    };

    if let Some(value) = map.get("group_id") {
        config.group_id = match value {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => {
                return Err(AdapterError::Config(
                    "meta_json.group_id 必须是字符串或 null".to_string(),
                ));
            }
        };
    }
    if let Some(value) = map.get("supports_groupid_query") {
        // null = 不覆盖（表单里的"没填"不是错）
        if !value.is_null() {
            config.supports_groupid_query = value.as_bool().ok_or_else(|| {
                AdapterError::Config("meta_json.supports_groupid_query 必须是布尔值".to_string())
            })?;
        }
    }
    if let Some(value) = map.get("path_chat") {
        if !value.is_null() {
            let path = value.as_str().ok_or_else(|| {
                AdapterError::Config("meta_json.path_chat 必须是字符串".to_string())
            })?;
            config.chat_path = path.to_string();
        }
    }
    if let Some(value) = map.get("inject_stream_usage") {
        if !value.is_null() {
            config.inject_stream_usage = value.as_bool().ok_or_else(|| {
                AdapterError::Config("meta_json.inject_stream_usage 必须是布尔值".to_string())
            })?;
        }
    }
    if let Some(value) = map.get("base_url") {
        if !value.is_null() {
            let base = value.as_str().ok_or_else(|| {
                AdapterError::Config("meta_json.base_url 必须是字符串".to_string())
            })?;
            config.base_url = base.to_string();
        }
    }
    if let Some(value) = map.get("auth_header_name") {
        if !value.is_null() {
            let name = value.as_str().ok_or_else(|| {
                AdapterError::Config("meta_json.auth_header_name 必须是字符串".to_string())
            })?;
            config.auth_header_name = Some(name.to_string());
        }
    }
    if let Some(value) = map.get("auth_style") {
        if !value.is_null() {
            let style = value.as_str().ok_or_else(|| {
                AdapterError::Config("meta_json.auth_style 必须是字符串".to_string())
            })?;
            config.auth_style = AuthStyle::parse(style).ok_or_else(|| {
                AdapterError::Config(format!(
                    "meta_json.auth_style 不支持 {style:?}（可选 bearer / custom_header）"
                ))
            })?;
        }
    }
    if let Some(value) = map.get("extra_headers") {
        if !value.is_null() {
            let map = value.as_object().ok_or_else(|| {
                AdapterError::Config("meta_json.extra_headers 必须是对象".to_string())
            })?;
            for (k, v) in map {
                let v = v.as_str().ok_or_else(|| {
                    AdapterError::Config(format!("meta_json.extra_headers.{k} 必须是字符串"))
                })?;
                config.extra_headers.push((k.to_string(), v.to_string()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::providers::copilot::CopilotSession;
    use crate::providers::oauth::copilot::{Endpoints, TokenCache};
    use crate::store::Store;
    use crate::vault::Vault;

    /// 一份可用的适配器依赖（**真保险库 + 本地 mock 端点 + 独立缓存**）。
    fn deps() -> (tempfile::TempDir, AdapterDeps) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
        let vault = Arc::new(std::sync::RwLock::new(
            Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
        ));
        let deps = AdapterDeps {
            copilot: CopilotSession::testing(
                vault,
                Endpoints::local("http://127.0.0.1:1"),
                Arc::new(TokenCache::new()),
            ),
        };
        (dir, deps)
    }

    fn row(provider: &str) -> ChannelRow {
        ChannelRow {
            id: "ch-1".to_string(),
            name: "我的渠道".to_string(),
            kind: "api_key".to_string(),
            provider: provider.to_string(),
            base_url: String::new(),
            meta_json: None,
        }
    }

    #[test]
    fn adapter_registry_covers_all_presets() {
        let (_dir, deps) = deps();
        for preset in presets::all() {
            let adapter = adapter_for(preset.provider, &deps).expect("预设应有适配器");
            assert_eq!(adapter.provider(), preset.provider);
            assert_eq!(adapter.kind(), preset.kind);
        }
    }

    #[test]
    fn adapter_registry_handles_custom_and_rejects_unknown() {
        let (_dir, deps) = deps();
        assert!(adapter_for("custom", &deps).is_some());
        assert!(adapter_for("not-exist", &deps).is_none());
    }

    #[test]
    fn copilot_gets_its_own_adapter() {
        // T21：Copilot **不再**被跳过（此前 `adapter_for("copilot")` 恒为 None）
        let (_dir, deps) = deps();
        let adapter = adapter_for(copilot::PROVIDER, &deps).expect("Copilot 应有适配器");
        assert_eq!(adapter.provider(), copilot::PROVIDER);
        assert_eq!(adapter.kind(), ChannelKind::OAuth);
    }

    #[test]
    fn adapter_for_row_falls_back_to_generic_for_api_key_kind() {
        let (_dir, deps) = deps();
        let mut r = row("my-own-proxy");
        r.kind = "api_key".to_string();
        let adapter = adapter_for_row(&r, &deps).expect("API Key 型未知 provider 应退回通用适配器");
        assert_eq!(adapter.provider(), "custom");
    }

    #[test]
    fn adapter_for_row_rejects_non_api_key_unknown_provider() {
        let (_dir, deps) = deps();
        let mut r = row("workbuddy");
        r.kind = "session".to_string();
        assert!(adapter_for_row(&r, &deps).is_none());
    }

    #[test]
    fn config_from_row_uses_preset_defaults() {
        let config = config_from_row(&row("deepseek")).expect("构造配置");
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.base_url, "https://api.deepseek.com");
        assert_eq!(config.chat_path, "/v1/chat/completions");
        assert_eq!(config.name, "我的渠道");
        assert!(config.inject_stream_usage);
        assert!(!config.supports_groupid_query);
        assert_eq!(config.group_id, None);
    }

    #[test]
    fn config_from_row_uses_preset_defaults_for_minimax_cn() {
        let config = config_from_row(&row("minimax_cn")).expect("构造配置");
        assert_eq!(config.base_url, "https://api.minimaxi.com");
        assert_eq!(config.chat_path, "/v1/text/chatcompletion_v2");
        assert!(config.supports_groupid_query);
        assert!(!config.inject_stream_usage, "MiniMax 不注入 stream_options");
    }

    #[test]
    fn config_from_row_custom_provider_uses_default_path() {
        let mut r = row("custom");
        r.base_url = "https://proxy.example.com".to_string();
        let config = config_from_row(&r).expect("构造配置");
        assert_eq!(config.base_url, "https://proxy.example.com");
        assert_eq!(config.chat_path, DEFAULT_CHAT_PATH);
        assert!(config.inject_stream_usage);
    }

    #[test]
    fn config_from_row_copilot_uses_the_copilot_specifics() {
        // T21：Copilot **不进预设表**，但走专属配置（路径不带 `/v1` + R-6 四项固定头）
        let mut r = row(copilot::PROVIDER);
        r.kind = "oauth".to_string();
        r.base_url = "https://api.individual.githubcopilot.com".to_string();
        let config = config_from_row(&r).expect("构造配置");
        assert_eq!(config.provider, copilot::PROVIDER);
        assert_eq!(config.kind, ChannelKind::OAuth);
        assert_eq!(config.auth_style, AuthStyle::Bearer);
        assert_eq!(config.chat_path, "/chat/completions");
        assert_eq!(config.base_url, "https://api.individual.githubcopilot.com");
        assert!(config.inject_stream_usage);
        assert_eq!(
            config.extra_headers,
            copilot::r6_headers(),
            "R-6 四项固定头必须在配置里"
        );
    }

    #[test]
    fn meta_json_overrides_group_id_and_path() {
        let mut r = row("minimax_cn");
        r.meta_json = Some(
            r#"{"group_id":"17895xxxxxxx","supports_groupid_query":true,"path_chat":"/v1/text/chatcompletion_v2"}"#
                .to_string(),
        );
        let config = config_from_row(&r).expect("构造配置");
        assert_eq!(config.group_id.as_deref(), Some("17895xxxxxxx"));
        assert!(config.supports_groupid_query);
        assert_eq!(config.chat_path, "/v1/text/chatcompletion_v2");
    }

    #[test]
    fn meta_json_can_disable_groupid_query() {
        let mut r = row("minimax_cn");
        r.meta_json =
            Some(r#"{"group_id":"17895xxxxxxx","supports_groupid_query":false}"#.to_string());
        let config = config_from_row(&r).expect("构造配置");
        assert_eq!(config.group_id.as_deref(), Some("17895xxxxxxx"));
        assert!(!config.supports_groupid_query);
    }

    #[test]
    fn meta_json_null_group_id_clears_it() {
        let mut r = row("minimax_cn");
        r.meta_json = Some(r#"{"group_id":null}"#.to_string());
        let config = config_from_row(&r).expect("构造配置");
        assert_eq!(config.group_id, None);
    }

    #[test]
    fn meta_json_can_toggle_stream_usage_injection() {
        let mut r = row("deepseek");
        r.meta_json = Some(r#"{"inject_stream_usage":false}"#.to_string());
        let config = config_from_row(&r).expect("构造配置");
        assert!(!config.inject_stream_usage);
    }

    #[test]
    fn meta_json_empty_and_invalid_are_handled() {
        let mut r = row("deepseek");
        r.meta_json = Some("   ".to_string());
        assert!(config_from_row(&r).is_ok());

        r.meta_json = Some("not json".to_string());
        let err = config_from_row(&r).expect_err("非法 JSON 应报错");
        assert!(matches!(err, AdapterError::Config(_)));
    }

    #[test]
    fn meta_json_wrong_type_is_rejected() {
        let mut r = row("deepseek");
        r.meta_json = Some(r#"{"group_id":123}"#.to_string());
        let err = config_from_row(&r).expect_err("类型错误应报错");
        assert!(matches!(err, AdapterError::Config(_)));
    }

    #[test]
    fn meta_json_extra_headers_are_applied() {
        let mut r = row("deepseek");
        r.meta_json = Some(r#"{"extra_headers":{"editor-version":"vscode/1.95.0"}}"#.to_string());
        let config = config_from_row(&r).expect("构造配置");
        assert_eq!(
            config.extra_headers,
            vec![("editor-version".to_string(), "vscode/1.95.0".to_string())]
        );
    }

    #[test]
    fn meta_json_can_switch_to_custom_auth_header() {
        // T8 自定义渠道表单：鉴权样式 + 头名
        let mut r = row("custom");
        r.meta_json = Some(
            r#"{"auth_style":"custom_header","auth_header_name":"x-api-key"}"#.to_string(),
        );
        let config = config_from_row(&r).expect("构造配置");
        assert_eq!(config.auth_style, AuthStyle::CustomHeader);
        assert_eq!(config.auth_header_name.as_deref(), Some("x-api-key"));
    }

    #[test]
    fn meta_json_rejects_unknown_auth_style() {
        let mut r = row("custom");
        r.meta_json = Some(r#"{"auth_style":"magic"}"#.to_string());
        let err = config_from_row(&r).expect_err("未知鉴权样式应报错");
        assert!(matches!(err, AdapterError::Config(_)));
    }

    #[test]
    fn meta_json_null_values_mean_no_override() {
        // 表单里的"没填"写成 null（整份替换语义），不能当成类型错误
        let mut r = row("deepseek");
        r.meta_json = Some(
            r#"{"path_chat":null,"auth_style":null,"auth_header_name":null,
                "supports_groupid_query":null,"inject_stream_usage":null,
                "base_url":null,"extra_headers":null,"group_id":null}"#
                .to_string(),
        );
        let config = config_from_row(&r).expect("null 不应导致失败");
        assert_eq!(config.chat_path, "/v1/chat/completions");
        assert_eq!(config.auth_style, AuthStyle::Bearer);
        assert_eq!(config.auth_header_name, None);
        assert_eq!(config.group_id, None);
        assert!(config.inject_stream_usage);
        assert!(config.extra_headers.is_empty());
    }
}
