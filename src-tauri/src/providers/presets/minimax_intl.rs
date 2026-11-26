//! MiniMax 国际站预设（02 §6.1 表第 4 行 + §4.2）。
//!
//! 已验证事实（官方文档 platform.minimax.io/docs/api-reference/text-post）：
//! - 端点 `POST https://api.minimax.io/v1/text/chatcompletion_v2`
//! - **URL 不带 GroupId**
//! - 仅 `Authorization: Bearer`
//!
//! 业务错误码映射见 [`super::minimax`]（两个 MiniMax profile 共用）。

use super::{PresetDef, ToolSupport};
use crate::providers::adapter::{AuthStyle, ChannelKind};

/// MiniMax 国际站。
pub fn preset() -> PresetDef {
    PresetDef {
        provider: "minimax_intl",
        name: "MiniMax 国际站",
        base_url: "https://api.minimax.io",
        chat_path: "/v1/text/chatcompletion_v2",
        kind: ChannelKind::ApiKey,
        auth_style: AuthStyle::Bearer,
        // 02 §4.1 R-4：MiniMax 流式自带 usage，**不注入**（400 风险）
        inject_stream_usage: false,
        // 02 §4.2：国际站 URL 不带 GroupId
        supports_groupid_query: false,
        extra_headers: &[],
        tool_support: ToolSupport::Unknown,
        // 02 §6.1.1：默认模型建议（未实测，可改）
        default_models: &[("abab6.5s-chat", "abab6.5s-chat")],
    }
}
