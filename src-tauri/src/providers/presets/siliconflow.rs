//! 硅基流动预设（02 §6.1 表第 7 行）。

use super::{PresetDef, ToolSupport};
use crate::providers::adapter::{AuthStyle, ChannelKind};

/// 硅基流动（SiliconFlow）。
pub fn preset() -> PresetDef {
    PresetDef {
        provider: "siliconflow",
        name: "硅基流动",
        base_url: "https://api.siliconflow.cn",
        chat_path: "/v1/chat/completions",
        kind: ChannelKind::ApiKey,
        auth_style: AuthStyle::Bearer,
        // 02 §4.1 R-4：硅基流动声明支持
        inject_stream_usage: true,
        supports_groupid_query: false,
        extra_headers: &[],
        tool_support: ToolSupport::Unknown,
    }
}
