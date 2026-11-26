//! DeepSeek 预设（02 §6.1 表第 1 行）。

use super::{PresetDef, ToolSupport};
use crate::providers::adapter::{AuthStyle, ChannelKind};

/// DeepSeek 官方。
pub fn preset() -> PresetDef {
    PresetDef {
        provider: "deepseek",
        name: "DeepSeek 官方",
        base_url: "https://api.deepseek.com",
        chat_path: "/v1/chat/completions",
        kind: ChannelKind::ApiKey,
        auth_style: AuthStyle::Bearer,
        // 02 §4.1 R-4：DeepSeek 声明支持
        inject_stream_usage: true,
        supports_groupid_query: false,
        extra_headers: &[],
        tool_support: ToolSupport::Unknown,
        // 02 §6.1.1：默认模型建议（未实测，可改）
        default_models: &[("deepseek-chat", "deepseek-chat")],
    }
}
