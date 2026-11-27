//! MiniMax Code（网页版）签到插件（T18；02 §7.4 §3）——**浏览器辅轨型**。
//!
//! **为什么必须走辅轨**：其签到接口是
//! `GET /minimax-cloud/api/v1/signin/status`（读签到面板）与
//! `POST /minimax-cloud/api/v1/signin/claim`（领取当日奖励），请求带
//! **OAuth bearer + `x-timestamp` + `x-signature` + `yy`** 等头——
//! **签名每次都变**，HAR 模板化无法重放；自己实现签名算法 = 逆向客户端私有协议，
//! 撞 AGENT.md §5.7「不做签名伪造」。
//!
//! **辅轨的做法**：让**浏览器自己算签名**（页面自己的 JS 完成）——应用只负责
//! 打开页面、触发签到、**读页面自己发出的网络响应**拿结果。
//!
//! **凭据**：**不用手填**。用户在弹出的浏览器里登录一次，登录态存进**持久化
//! 浏览器 profile**（由浏览器自己加密保管，应用不读不导不入库）。

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::checkin::plugin::{LoginOutcome, PluginContext, SitePlugin};
use crate::checkin::replay::{ReplayClass, ReplayOutcome};
use crate::checkin::reward::Reward;

/// 插件标识（= `checkin_site.plugin`）。
pub const PLUGIN_ID: &str = "minimax_code";
/// 展示名。
pub const DISPLAY_NAME: &str = "MiniMax Code（网页版）";
/// 凭据提示。
pub const CREDENTIAL_HINT: &str =
    "不用手填凭据——点「登录」在弹出的浏览器里登录一次，登录态存本地浏览器 profile";
/// 默认 cron（避开整点与拥挤时段）。
pub const DEFAULT_SCHEDULE: &str = "20 8 * * *";

/// 默认网页地址（国内站；全球站是 agent.minimax.io —— 两边账号不通用）。
pub const DEFAULT_PAGE_URL: &str = "https://agent.minimax.cn/";

/// 辅轨 `checkin` 动作的超时（浏览器启动 + 页面加载，比 HTTP 慢得多）。
pub const CHECKIN_TIMEOUT_SECS: u64 = 90;

/// 辅轨 `login` 动作的超时（**要等用户手动登录**，给足时间）。
pub const LOGIN_TIMEOUT_SECS: u64 = 300;

/// 辅轨 `checkin` 的返回（**sidecar 的输出契约**）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinReport {
    /// 是否签到成功。
    pub ok: bool,
    /// 今天是不是**已经领过了**（幂等；**算成功**，不是失败）。
    #[serde(default)]
    pub already_claimed: bool,
    /// 本次拿到的积分。
    #[serde(default)]
    pub points: Option<i64>,
    /// 连签第几天。
    #[serde(default)]
    pub day: Option<i64>,
    /// 失败码（`not_logged_in` / `no_browser` / `network` / …）。
    #[serde(default)]
    pub code: Option<String>,
    /// 人话原因。
    #[serde(default)]
    pub message: Option<String>,
}

/// 失败码 → 分类（**与 T16 的 8 类同口径**）。
pub fn class_for(code: &str) -> ReplayClass {
    match code {
        "not_logged_in" => ReplayClass::CredentialInvalid,
        "no_browser" | "sidecar_unavailable" | "profile_error" => ReplayClass::TemplateInvalid,
        "network" | "timeout" | "sidecar_error" => ReplayClass::NetworkError,
        "rate_limited" => ReplayClass::RateLimited,
        // 页面改版 / 业务失败 → 断言不符（不是网络问题，也不是凭据问题）
        "no_signin_entry" | "no_claim_response" | "claim_failed" => ReplayClass::AssertFailed,
        _ => ReplayClass::HttpError,
    }
}

/// MiniMax Code 站点插件。
pub struct MiniMaxCodePlugin;

#[async_trait]
impl SitePlugin for MiniMaxCodePlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn credential_hint(&self) -> &'static str {
        CREDENTIAL_HINT
    }

    fn needs_sidecar(&self) -> bool {
        true
    }

    fn default_schedule(&self) -> &'static str {
        DEFAULT_SCHEDULE
    }

    async fn run(&self, ctx: &PluginContext<'_>) -> ReplayOutcome {
        let started = std::time::Instant::now();
        let Some(sidecar) = ctx.sidecar.as_ref() else {
            return failure(
                ReplayClass::TemplateInvalid,
                "浏览器辅轨不可用——没找到 sidecar 脚本或 Node 运行时（开发态：在 sidecar/ 目录跑一次 `npm install`）".to_string(),
                started,
            );
        };
        let page = ctx
            .config
            .page_url
            .as_deref()
            .unwrap_or(DEFAULT_PAGE_URL);
        let value = match sidecar
            .run(
                "checkin",
                &[page.to_string()],
                Duration::from_secs(CHECKIN_TIMEOUT_SECS),
            )
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return failure(ReplayClass::NetworkError, format!("辅轨执行失败：{err}"), started);
            }
        };
        let report: CheckinReport = match serde_json::from_value(value) {
            Ok(report) => report,
            Err(err) => {
                return failure(
                    ReplayClass::HttpError,
                    format!("辅轨返回解析失败：{err}"),
                    started,
                );
            }
        };

        if report.ok || report.already_claimed {
            let message = if report.already_claimed {
                match report.day {
                    Some(day) => format!("MiniMax Code 今天已经领过了（连签第 {day} 天）"),
                    None => "MiniMax Code 今天已经领过了".to_string(),
                }
            } else {
                match (report.points, report.day) {
                    (Some(points), Some(day)) => {
                        format!("MiniMax Code 签到成功（+{points} 积分，连签第 {day} 天）")
                    }
                    (Some(points), None) => format!("MiniMax Code 签到成功（+{points} 积分）"),
                    _ => "MiniMax Code 签到成功".to_string(),
                }
            };
            let reward = match report.points {
                Some(amount) => Some(Reward::new(
                    amount,
                    "point",
                    serde_json::json!({"points": amount, "day": report.day}),
                )),
                None => None,
            };
            return ReplayOutcome {
                ok: true,
                class: ReplayClass::Success,
                message,
                status: None,
                elapsed_ms: started.elapsed().as_millis() as u64,
                retryable: false,
                body_preview: None,
                reward,
            };
        }

        let code = report.code.as_deref().unwrap_or("unknown");
        let message = report
            .message
            .unwrap_or_else(|| "MiniMax Code 签到失败（辅轨没给原因）".to_string());
        failure(class_for(code), message, started)
    }

    async fn login(&self, ctx: &PluginContext<'_>) -> Option<LoginOutcome> {
        let sidecar = ctx.sidecar.as_ref()?;
        let page = ctx
            .config
            .page_url
            .as_deref()
            .unwrap_or(DEFAULT_PAGE_URL);
        let value = sidecar
            .run(
                "login",
                &[page.to_string()],
                Duration::from_secs(LOGIN_TIMEOUT_SECS),
            )
            .await
            .ok()?;
        let ok = value
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(if ok { "登录完成" } else { "登录未完成" })
            .to_string();
        Some(LoginOutcome { ok, message })
    }
}

/// 造一个失败结果。
fn failure(class: ReplayClass, message: String, started: std::time::Instant) -> ReplayOutcome {
    ReplayOutcome {
        ok: false,
        class,
        message,
        status: None,
        elapsed_ms: started.elapsed().as_millis() as u64,
        retryable: class.retryable_by_default(),
        body_preview: None,
        reward: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_codes_map_to_the_eight_classes() {
        assert_eq!(class_for("not_logged_in"), ReplayClass::CredentialInvalid);
        assert_eq!(class_for("no_browser"), ReplayClass::TemplateInvalid);
        assert_eq!(class_for("network"), ReplayClass::NetworkError);
        assert_eq!(class_for("timeout"), ReplayClass::NetworkError);
        assert_eq!(class_for("whatever"), ReplayClass::HttpError);
    }

    #[test]
    fn report_deserializes_the_sidecar_contract() {
        let report: CheckinReport =
            serde_json::from_str(r#"{"ok":true,"points":400,"day":4}"#).unwrap();
        assert!(report.ok);
        assert!(!report.already_claimed);
        assert_eq!(report.points, Some(400));
        assert_eq!(report.day, Some(4));

        let claimed: CheckinReport =
            serde_json::from_str(r#"{"ok":false,"alreadyClaimed":true,"day":7}"#).unwrap();
        assert!(!claimed.ok);
        assert!(claimed.already_claimed);
    }
}
