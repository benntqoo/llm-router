//! 站点插件机制（T18；02 §7.4 §1）。
//!
//! `checkin_site.plugin` 决定「这次签到怎么打」：
//! - `har`：用户自建模板 → 走 **T16 重放引擎**（`checkin::replay`）；
//! - `modelscope` / `minimax_code`：**内置插件** → 走 `checkin::sites::*`。
//!
//! 内置插件与 HAR 模板**共用同一套结果语义**（02 §7.1.3 §4 的 8 类
//! `ReplayClass`）——调度层、落库、通知（T19）都认同一份形状。
//!
//! **内置插件带正确的断言**：这解掉 T16/T17 留档里那条已知限制——导入的模板
//! 带着**录制时的状态码**（ModelScope 匿名态 401、真实成功 200），用户得自己
//! 改，而**没有断言编辑 UI**。内置插件的断言随插件发布，用户不用改。

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::checkin::replay::ReplayOutcome;
use crate::checkin::sites;
use crate::store::checkin::SiteRow;
use crate::vault::Vault;

/// 插件执行上下文。
pub struct PluginContext<'a> {
    /// 站点行。
    pub site: &'a SiteRow,
    /// 站点私有配置（`config_json` 反序列化；解析失败用默认值）。
    pub config: PluginSiteConfig,
    /// HTTP 客户端（连接 10s / 整请求 30s，与重放引擎同口径）。
    pub client: &'a reqwest::Client,
    /// 保险库（取凭据）。
    pub vault: &'a Arc<RwLock<Vault>>,
    /// 浏览器辅轨客户端（**只有走辅轨的插件用得到**；`None` = 辅轨不可用）。
    pub sidecar: Option<crate::checkin::sidecar::Sidecar>,
}

/// 内置插件的站点私有配置（`checkin_site.config_json`）。
///
/// **内置插件的 `config_json` 不是 HAR 模板**，而是这份结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginSiteConfig {
    /// 登录态凭据在保险库里的 id（`credential.id`）。
    pub credential_id: Option<String>,
    /// 第二凭据（开放平台 token 等）的 id——**可选**。
    pub token_credential_id: Option<String>,
    /// 积分字段路径（覆盖插件默认值；点号路径 + 数组下标）。
    pub reward_path: Option<String>,
    /// 覆盖默认的站点根地址（**测试 / 自建站点**用）。
    pub base_url: Option<String>,
    /// 覆盖默认的**网页地址**（辅轨型插件用；如 MiniMax Code 的国内站 / 全球站）。
    pub page_url: Option<String>,
    /// 归属渠道 id（凭据挂在这里——`credential.channel_id` 是 NOT NULL 外键）。
    pub channel_id: Option<String>,
    /// 归属账号 id（积分回写落在它的 `quota_json`——02 §7.4 §4）。
    pub account_id: Option<String>,
}

impl PluginSiteConfig {
    /// 序列化成 `config_json` 的文本。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// 登录交接的结果（**只有辅轨型插件会用到**）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginOutcome {
    /// 是否登录成功。
    pub ok: bool,
    /// 人话原因。
    pub message: String,
}

/// 一个内置站点插件。
#[async_trait]
pub trait SitePlugin: Send + Sync {
    /// 插件标识（= `checkin_site.plugin`）。
    fn id(&self) -> &'static str;

    /// 展示名（UI 用）。
    fn display_name(&self) -> &'static str;

    /// 用户要准备什么凭据（UI 提示用）。
    fn credential_hint(&self) -> &'static str;

    /// 是否需要浏览器辅轨。
    fn needs_sidecar(&self) -> bool {
        false
    }

    /// 默认 cron 建议（新建站点时的默认值；UI 可改）。
    fn default_schedule(&self) -> &'static str {
        "0 9 * * *"
    }

    /// 跑一次签到。
    async fn run(&self, ctx: &PluginContext<'_>) -> ReplayOutcome;

    /// **登录交接**（拉起浏览器让用户登录一次）。
    ///
    /// 默认不支持（返回 `None`）——只有辅轨型插件需要。
    async fn login(&self, _ctx: &PluginContext<'_>) -> Option<LoginOutcome> {
        None
    }
}

/// 内置插件清单项（给 UI 用）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinSite {
    /// 插件标识。
    pub id: &'static str,
    /// 展示名。
    pub name: &'static str,
    /// 凭据提示。
    pub credential_hint: &'static str,
    /// 是否需要浏览器辅轨。
    pub needs_sidecar: bool,
    /// 默认 cron。
    pub default_schedule: &'static str,
}

/// 按 id 取内置插件（`None` = 没有这个内置插件）。
pub fn builtin(plugin_id: &str) -> Option<Box<dyn SitePlugin>> {
    match plugin_id {
        sites::modelscope::PLUGIN_ID => Some(Box::new(sites::modelscope::ModelScopePlugin)),
        sites::minimax_code::PLUGIN_ID => {
            Some(Box::new(sites::minimax_code::MiniMaxCodePlugin))
        }
        _ => None,
    }
}

/// 内置插件清单（UI 的「添加站点」用）。
pub fn builtin_catalog() -> Vec<BuiltinSite> {
    vec![
        BuiltinSite {
            id: sites::modelscope::PLUGIN_ID,
            name: sites::modelscope::DISPLAY_NAME,
            credential_hint: sites::modelscope::CREDENTIAL_HINT,
            needs_sidecar: false,
            default_schedule: sites::modelscope::DEFAULT_SCHEDULE,
        },
        BuiltinSite {
            id: sites::minimax_code::PLUGIN_ID,
            name: sites::minimax_code::DISPLAY_NAME,
            credential_hint: sites::minimax_code::CREDENTIAL_HINT,
            needs_sidecar: true,
            default_schedule: sites::minimax_code::DEFAULT_SCHEDULE,
        },
    ]
}

/// 解析站点的插件配置（解析失败退回默认值——**不该因为配置脏了就整个签到失败**）。
pub fn parse_config(site: &SiteRow) -> PluginSiteConfig {
    site.config_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<PluginSiteConfig>(json).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_default_schedule_matches_each_plugin() {
        // **回归**：清单里的默认 cron 必须与插件自己的默认值一致——
        // 否则 UI 显示的默认 cron 与实际落库的不是同一个（真机踩过：
        // MiniMax Code 清单写 `20 8 * * *`，插件却回落到 trait 默认的 `0 9 * * *`）。
        for item in builtin_catalog() {
            let plugin = builtin(item.id).expect("清单项应有对应插件");
            assert_eq!(
                plugin.default_schedule(),
                item.default_schedule,
                "插件 {} 的默认 cron 与清单不一致",
                item.id
            );
        }
    }

    #[test]
    fn builtin_lookup_returns_none_for_unknown_plugins() {
        assert!(builtin("har").is_none(), "HAR 模板不是内置插件");
        assert!(builtin("not-a-plugin").is_none());
        assert!(builtin("modelscope").is_some());
        assert!(builtin("minimax_code").is_some());
    }

    #[test]
    fn a_dirty_config_falls_back_to_defaults() {
        // **不该因为配置脏了就整个签到失败**
        let site = SiteRow {
            id: "site_x".to_string(),
            name: "脏配置".to_string(),
            plugin: "modelscope".to_string(),
            config_json: Some("这不是 JSON".to_string()),
            schedule: None,
            enabled: false,
        };
        let config = parse_config(&site);
        assert!(config.credential_id.is_none());
        assert!(config.account_id.is_none());
    }
}
