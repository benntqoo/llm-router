//! GLM（智谱）预设（02 §6.1 表第 3 行）。
//!
//! 注意：GLM 出站路径**不是** OpenAI 标准路径。

use super::{PresetDef, ToolSupport};
use crate::providers::adapter::{AuthStyle, ChannelKind};

/// 智谱 GLM 开放平台。
pub fn preset() -> PresetDef {
    PresetDef {
        provider: "glm",
        name: "GLM 官方",
        base_url: "https://open.bigmodel.cn",
        chat_path: "/api/paas/v4/chat/completions",
        kind: ChannelKind::ApiKey,
        auth_style: AuthStyle::Bearer,
        inject_stream_usage: true,
        supports_groupid_query: false,
        extra_headers: &[],
        tool_support: ToolSupport::Unknown,
        // 02 §6.1.1：默认模型建议（未实测，可改）
        default_models: &[("glm-4-flash", "glm-4-flash")],
    }
}
