//! MiniMax 国内站预设（02 §6.1 表第 5 行 + §4.2）。
//!
//! 口径分裂（02 §4.2）：多数第三方称 v2 无需 GroupId（Bearer 已绑组织），
//! 部分示例带 `?GroupId=`。本实现按"**用户显式配置才拼**"处理：
//! `channel.meta_json.group_id` 非空且 `supports_groupid_query = true` 时
//! 追加 `?GroupId=<value>`（R-5）。
//!
//! 1004 鉴权失败时 UI 必须提示"检查 GroupId 配置，而非重新生成 key"
//! （见 [`super::minimax::map_business_error`]）。

use super::{PresetDef, ToolSupport};
use crate::providers::adapter::{AuthStyle, ChannelKind};

/// MiniMax 国内站。
pub fn preset() -> PresetDef {
    PresetDef {
        provider: "minimax_cn",
        name: "MiniMax 国内站",
        base_url: "https://api.minimaxi.com",
        chat_path: "/v1/text/chatcompletion_v2",
        kind: ChannelKind::ApiKey,
        auth_style: AuthStyle::Bearer,
        // 02 §4.1 R-4：MiniMax 流式自带 usage，**不注入**（400 风险）
        inject_stream_usage: false,
        // 02 §4.2：国内站 GroupId 走 URL query，且**仅在用户显式配置时**才拼
        supports_groupid_query: true,
        extra_headers: &[],
        tool_support: ToolSupport::Unknown,
        // 02 §6.1.1：默认模型建议（未实测，可改）
        default_models: &[("abab6.5s-chat", "abab6.5s-chat")],
    }
}
