//! 阿里百炼（DashScope 兼容模式）预设（02 §6.1 表第 6 行）。
//!
//! 注意：兼容模式的出站路径**不是** OpenAI 标准路径。

use super::{PresetDef, ToolSupport};
use crate::providers::adapter::{AuthStyle, ChannelKind};

/// 阿里云百炼（兼容模式）。
pub fn preset() -> PresetDef {
    PresetDef {
        provider: "dashscope",
        name: "阿里百炼",
        base_url: "https://dashscope.aliyuncs.com",
        chat_path: "/compatible-mode/v1/chat/completions",
        kind: ChannelKind::ApiKey,
        auth_style: AuthStyle::Bearer,
        // 02 §4.1 R-4：百炼声明支持
        inject_stream_usage: true,
        supports_groupid_query: false,
        extra_headers: &[],
        tool_support: ToolSupport::Unknown,
    }
}
