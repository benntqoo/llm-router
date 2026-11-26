//! 渠道预设（02 §6.1）。
//!
//! 预设在 v1 是**纯数据**：新增一个 OpenAI 兼容渠道 = 填一张表
//! （base_url / 路径模板 / 鉴权样式 / group_id 等 meta），**零代码**。
//!
//! 口径说明：02 §6.1 标题写"8 profile"，但表体只有 7 行；03 交付计划 T8 写
//! "7 个 profile"。本实现按**表体的 7 行**落地，差异记入 T4 验收留档。

pub mod dashscope;
pub mod deepseek;
pub mod glm;
pub mod kimi;
pub mod minimax;
pub mod minimax_cn;
pub mod minimax_intl;
pub mod siliconflow;

use serde::Serialize;

use crate::providers::adapter::{AuthStyle, ChannelConfig, ChannelKind};

/// 工具调用（`tool_calls`）兼容度。
///
/// 02 §4.1 注：实测不过的渠道标 `partial` 并在 UI 提示；02 §10.1 判据
/// "全绿才能标 `stable`"。实测矩阵由 T10 执行，**在实测之前一律 `Unknown`**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSupport {
    /// 实测全绿
    Stable,
    /// 部分兼容（UI 需明示不可用特性）
    Partial,
    /// 未实测
    Unknown,
}

impl ToolSupport {
    /// 文本形式（前端用）。
    pub fn as_str(self) -> &'static str {
        match self {
            ToolSupport::Stable => "stable",
            ToolSupport::Partial => "partial",
            ToolSupport::Unknown => "unknown",
        }
    }
}

/// 预设定义（纯数据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetDef {
    /// 预设标识（写入 `channel.provider`）
    pub provider: &'static str,
    /// 渠道默认名（`channel.name` 初值）
    pub name: &'static str,
    /// 基础地址
    pub base_url: &'static str,
    /// chat 路径模板
    pub chat_path: &'static str,
    /// 渠道类型
    pub kind: ChannelKind,
    /// 鉴权样式
    pub auth_style: AuthStyle,
    /// 是否注入 `stream_options.include_usage`（R-4）
    pub inject_stream_usage: bool,
    /// 是否支持 `?GroupId=` 追加（R-5）
    pub supports_groupid_query: bool,
    /// 额外固定头（R-6，如 Copilot 伪装头）
    pub extra_headers: &'static [(&'static str, &'static str)],
    /// `tool_calls` 兼容度
    pub tool_support: ToolSupport,
    /// 默认模型：`(上游模型名, 对外别名)`（02 §6.1.1；**未实测建议值**，可改）
    pub default_models: &'static [(&'static str, &'static str)],
}

impl PresetDef {
    /// 转换为渠道配置（**不含**渠道行 `meta_json` 覆盖，合并见
    /// [`crate::providers::config_from_row`]）。
    pub fn to_config(&self, channel_id: &str) -> ChannelConfig {
        ChannelConfig {
            channel_id: channel_id.to_string(),
            provider: self.provider.to_string(),
            name: self.name.to_string(),
            kind: self.kind,
            base_url: self.base_url.to_string(),
            chat_path: self.chat_path.to_string(),
            auth_style: self.auth_style,
            auth_header_name: None,
            group_id: None,
            supports_groupid_query: self.supports_groupid_query,
            inject_stream_usage: self.inject_stream_usage,
            extra_headers: self
                .extra_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// 前端视图（IPC 用；`PresetDef` 本身是 `&'static` 引用，不便直接序列化）。
    pub fn view(&self) -> PresetView {
        PresetView {
            provider: self.provider.to_string(),
            name: self.name.to_string(),
            base_url: self.base_url.to_string(),
            chat_path: self.chat_path.to_string(),
            kind: self.kind.as_str().to_string(),
            auth_style: self.auth_style.as_str().to_string(),
            inject_stream_usage: self.inject_stream_usage,
            supports_groupid_query: self.supports_groupid_query,
            tool_support: self.tool_support.as_str().to_string(),
            default_models: self
                .default_models
                .iter()
                .map(|(upstream, alias)| ModelSuggestion {
                    upstream_id: upstream.to_string(),
                    alias: alias.to_string(),
                })
                .collect(),
        }
    }
}

/// 预设的前端视图（02 §6.1 的 profile 数据）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresetView {
    /// 预设标识（写入 `channel.provider`）。
    pub provider: String,
    /// 渠道默认名。
    pub name: String,
    /// 基础地址。
    pub base_url: String,
    /// chat 路径模板。
    pub chat_path: String,
    /// 渠道类型（`api_key`）。
    pub kind: String,
    /// 鉴权样式（`bearer`）。
    pub auth_style: String,
    /// 是否注入 `stream_options.include_usage`（R-4）。
    pub inject_stream_usage: bool,
    /// 是否支持 `?GroupId=` 追加（R-5）。
    pub supports_groupid_query: bool,
    /// `tool_calls` 兼容度（`stable` / `partial` / `unknown`）。
    pub tool_support: String,
    /// 默认模型建议（**未实测**，用户可改）。
    pub default_models: Vec<ModelSuggestion>,
}

/// 一条模型建议。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelSuggestion {
    /// 上游真实模型名（R-1 替换的目标）。
    pub upstream_id: String,
    /// 对外别名。
    pub alias: String,
}

/// 全部 v1 预设（02 §6.1 表体的 7 行，顺序与文档一致）。
pub fn all() -> Vec<PresetDef> {
    vec![
        deepseek::preset(),
        kimi::preset(),
        glm::preset(),
        minimax_intl::preset(),
        minimax_cn::preset(),
        dashscope::preset(),
        siliconflow::preset(),
    ]
}

/// 按 `channel.provider` 查预设。
pub fn find(provider: &str) -> Option<PresetDef> {
    all().into_iter().find(|p| p.provider == provider)
}

/// 全部预设的前端视图（IPC 用）。
pub fn all_views() -> Vec<PresetView> {
    all().iter().map(PresetDef::view).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_seven_presets() {
        let all = all();
        assert_eq!(all.len(), 7, "02 §6.1 表体为 7 行");
        let providers: Vec<&str> = all.iter().map(|p| p.provider).collect();
        assert_eq!(
            providers,
            vec![
                "deepseek",
                "kimi",
                "glm",
                "minimax_intl",
                "minimax_cn",
                "dashscope",
                "siliconflow",
            ]
        );
    }

    #[test]
    fn registry_lookup_by_provider() {
        assert!(find("deepseek").is_some());
        assert!(find("minimax_cn").is_some());
        assert!(find("copilot").is_none(), "Copilot 属 OAuth 型，不在 v1 API Key 预设表内");
        assert!(find("not-exist").is_none());
    }

    #[test]
    fn all_presets_are_api_key_bearer_and_openai_style_paths() {
        for preset in all() {
            assert_eq!(preset.kind, ChannelKind::ApiKey);
            assert_eq!(preset.auth_style, AuthStyle::Bearer);
            assert!(preset.base_url.starts_with("https://"));
            assert!(preset.chat_path.starts_with('/'));
            assert!(!preset.supports_groupid_query || preset.provider == "minimax_cn");
        }
    }

    #[test]
    fn minimax_presets_do_not_inject_stream_usage() {
        // 02 §4.1 R-4：MiniMax 流式自带 usage，注入多余字段有 400 风险
        assert!(!minimax_intl::preset().inject_stream_usage);
        assert!(!minimax_cn::preset().inject_stream_usage);
    }

    #[test]
    fn other_presets_inject_stream_usage() {
        for preset in all() {
            if preset.provider.starts_with("minimax") {
                continue;
            }
            assert!(
                preset.inject_stream_usage,
                "{} 应声明支持 stream_options.include_usage",
                preset.provider
            );
        }
    }

    #[test]
    fn to_config_fills_preset_fields() {
        let config = glm::preset().to_config("ch-1");
        assert_eq!(config.channel_id, "ch-1");
        assert_eq!(config.provider, "glm");
        assert_eq!(config.base_url, "https://open.bigmodel.cn");
        assert_eq!(config.chat_path, "/api/paas/v4/chat/completions");
        assert_eq!(config.group_id, None);
        assert!(config.extra_headers.is_empty());
    }
}
