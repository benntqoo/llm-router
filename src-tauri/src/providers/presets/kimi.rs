//! Kimi（月之暗面）预设（02 §6.1 表第 2 行）。

use super::{PresetDef, ToolSupport};
use crate::providers::adapter::{AuthStyle, ChannelKind};

/// Kimi 官方（api.moonshot.cn）。
pub fn preset() -> PresetDef {
    PresetDef {
        provider: "kimi",
        name: "Kimi 官方",
        base_url: "https://api.moonshot.cn",
        chat_path: "/v1/chat/completions",
        kind: ChannelKind::ApiKey,
        auth_style: AuthStyle::Bearer,
        // 02 §4.1 R-4：Kimi 声明支持
        inject_stream_usage: true,
        supports_groupid_query: false,
        extra_headers: &[],
        tool_support: ToolSupport::Unknown,
    }
}
