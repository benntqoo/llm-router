//! ModelScope（魔搭）签到插件（T18；02 §7.4 §2）。
//!
//! **接口面已实测**（2026-11-27，匿名态 + 假凭据探测）：
//! - `GET /api/v1/users/login/info` → **触发当日签到**；匿名 **401**「禁止访问」；
//!   成功时 `Success == true`（`/api/v1/users/binding` 匿名可读，回 `Code:200`，
//!   证明 `/api/v1/*` 的成功响应体形状是 `Code` / `Data` / `Message` / `Success`）。
//! - `GET /openapi/v1/magicubes/balance` → 魔粒余额；匿名 **401** `InvalidAuthentication`。
//! - 两套认证体系：`/api/v1/*` = **网页会话 Cookie**；`/openapi/v1/*` = **开放平台
//!   Bearer token**（`/openapi/v1/users/me` 的报错原文是 `missing or invalid
//!   Authorization header`）。
//!
//! **凭据**：网页 Cookie（**必需**）+ 开放平台 token（**可选**，只有它能查魔粒）。
//! 没有 token 时**签到仍然成功**，只是拿不到积分——**不因为查不到积分就判签到失败**。

use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::checkin::har::json_lookup;
use crate::checkin::plugin::{PluginContext, SitePlugin};
use crate::checkin::replay::{ReplayClass, ReplayOutcome};
use crate::checkin::reward::Reward;

/// 插件标识（= `checkin_site.plugin`）。
pub const PLUGIN_ID: &str = "modelscope";
/// 展示名。
pub const DISPLAY_NAME: &str = "ModelScope（魔搭）";
/// 凭据提示。
pub const CREDENTIAL_HINT: &str =
    "网页会话 Cookie（F12 → Network → 任一 modelscope.cn 请求 → Request Headers → Cookie）";
/// 默认 cron。
pub const DEFAULT_SCHEDULE: &str = "0 9 * * *";

/// 站点根地址（可被站点配置覆盖——测试 / 自建站点用）。
pub const DEFAULT_BASE_URL: &str = "https://www.modelscope.cn";
/// 签到接口（触发当日签到）。
pub const SIGNIN_PATH: &str = "/api/v1/users/login/info";
/// 魔粒余额接口（开放平台；Bearer）。
pub const BALANCE_PATH: &str = "/openapi/v1/magicubes/balance";

/// 积分字段的**候选路径**（按顺序试，第一个取到数值的即用）。
///
/// 登录态下的成功响应字段名**未实测**（02 §7.4 §5），所以默认给一串候选；
/// 实测后收敛成一条并回填 02。站点配置 `reward.path` 可覆盖。
pub const DEFAULT_REWARD_PATHS: &[&str] = &[
    "data.points",
    "data.balance",
    "data.magicubes",
    "data.total",
    "points",
    "balance",
];

/// 签到响应的判定结果（**纯函数**，单测直接打）。
#[derive(Debug, Clone, PartialEq)]
pub enum SigninVerdict {
    /// 签到成功。
    Success,
    /// 业务失败（`Success != true`）——带站点给的中文原因。
    Business(String),
    /// 状态码类失败——带分类与原因。
    Status(ReplayClass, String),
    /// 响应不是 JSON。
    NotJson(String),
}

/// 判定签到响应（**纯函数**）。
///
/// 口径（02 §7.4 §2）：**判成功必须断言 JSON 业务字段**，不能只看状态码——
/// ModelScope 的业务失败也可能是 2xx。
pub fn evaluate_signin(status: u16, body: &[u8]) -> SigninVerdict {
    match status {
        401 | 403 => {
            return SigninVerdict::Status(
                ReplayClass::CredentialInvalid,
                "ModelScope 登录态无效（Cookie 过期或没填）——重新从浏览器抄一次 Cookie".to_string(),
            );
        }
        429 => {
            return SigninVerdict::Status(
                ReplayClass::RateLimited,
                "ModelScope 限流了（HTTP 429）——下个应触发周期再试".to_string(),
            );
        }
        code if !(200..300).contains(&code) => {
            return SigninVerdict::Status(
                ReplayClass::HttpError,
                format!("ModelScope 返回了 HTTP {code}"),
            );
        }
        _ => {}
    }
    let json: Value = match serde_json::from_slice(body) {
        Ok(json) => json,
        Err(err) => {
            return SigninVerdict::NotJson(format!("ModelScope 响应不是 JSON：{err}"));
        }
    };
    match json.get("Success").and_then(|value| value.as_bool()) {
        Some(true) => SigninVerdict::Success,
        _ => {
            let reason = json
                .get("Message")
                .and_then(|value| value.as_str())
                .unwrap_or("站点没给原因");
            SigninVerdict::Business(format!("ModelScope 签到未成功：{reason}"))
        }
    }
}

/// 提取魔粒数量（**纯函数**）。
///
/// `path` 为 `Some` 时只用它；否则按 [`DEFAULT_REWARD_PATHS`] 依次试。
pub fn extract_reward(json: &Value, path: Option<&str>) -> Option<i64> {
    match path {
        Some(path) => json_lookup(json, path).and_then(|value| value.as_i64()),
        None => DEFAULT_REWARD_PATHS
            .iter()
            .find_map(|path| json_lookup(json, path).and_then(|value| value.as_i64())),
    }
}

/// ModelScope 站点插件。
pub struct ModelScopePlugin;

#[async_trait]
impl SitePlugin for ModelScopePlugin {
    fn id(&self) -> &'static str {
        PLUGIN_ID
    }

    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }

    fn credential_hint(&self) -> &'static str {
        CREDENTIAL_HINT
    }

    fn default_schedule(&self) -> &'static str {
        DEFAULT_SCHEDULE
    }

    async fn run(&self, ctx: &PluginContext<'_>) -> ReplayOutcome {
        let started = Instant::now();
        let base = ctx
            .config
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/');

        // 1) 网页 Cookie（必需）
        let cookie = match read_credential(ctx, ctx.config.credential_id.as_deref()) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };

        // 2) 签到
        let response = match ctx
            .client
            .get(format!("{base}{SIGNIN_PATH}"))
            .header(reqwest::header::COOKIE, cookie.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                return failure(
                    ReplayClass::NetworkError,
                    format!("ModelScope 签到请求失败：{err}"),
                    None,
                    started,
                );
            }
        };
        let status = response.status().as_u16();
        let body = match response.bytes().await {
            Ok(bytes) => bytes.to_vec(),
            Err(err) => {
                return failure(
                    ReplayClass::NetworkError,
                    format!("ModelScope 读响应体失败：{err}"),
                    Some(status),
                    started,
                );
            }
        };

        // 3) 判定
        match evaluate_signin(status, &body) {
            SigninVerdict::Success => {}
            SigninVerdict::Business(reason) => {
                return failure(ReplayClass::AssertFailed, reason, Some(status), started);
            }
            SigninVerdict::Status(class, reason) => {
                return failure(class, reason, Some(status), started);
            }
            SigninVerdict::NotJson(reason) => {
                return failure(ReplayClass::AssertFailed, reason, Some(status), started);
            }
        }

        // 4) 查魔粒（**可选**；查不到不影响签到成功）
        let reward = self.fetch_reward(ctx, base, &cookie).await;
        let message = match &reward {
            Some(reward) => format!("ModelScope 签到成功（魔粒 +{}）", reward.amount),
            None => "ModelScope 签到成功".to_string(),
        };
        ReplayOutcome {
            ok: true,
            class: ReplayClass::Success,
            message,
            status: Some(status),
            elapsed_ms: started.elapsed().as_millis() as u64,
            retryable: false,
            body_preview: None,
            reward,
        }
    }
}

impl ModelScopePlugin {
    /// 查魔粒余额（**失败就返回 `None`**——不因为查不到积分就判签到失败）。
    async fn fetch_reward(
        &self,
        ctx: &PluginContext<'_>,
        base: &str,
        cookie: &str,
    ) -> Option<Reward> {
        // 优先用开放平台 token；没有就拿网页 Cookie 试（能不能开 openapi 待实测）
        let token = ctx
            .config
            .token_credential_id
            .as_deref()
            .and_then(|id| read_credential_quiet(ctx, id));
        let mut request = ctx
            .client
            .get(format!("{base}{BALANCE_PATH}"))
            .header(reqwest::header::ACCEPT, "application/json");
        request = match &token {
            Some(token) => request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}")),
            None => request.header(reqwest::header::COOKIE, cookie.to_string()),
        };
        let response = request.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        // reqwest 没开 `json` feature → 自己读字节再解析
        let bytes = response.bytes().await.ok()?;
        let json: Value = serde_json::from_slice(&bytes).ok()?;
        let amount = extract_reward(&json, ctx.config.reward_path.as_deref())?;
        Some(Reward::new(amount, "magicube", json))
    }
}

/// 读一条凭据（**失败 → 直接给结果**，不让插件 panic）。
fn read_credential(
    ctx: &PluginContext<'_>,
    credential_id: Option<&str>,
) -> Result<String, ReplayOutcome> {
    let Some(credential_id) = credential_id else {
        return Err(failure_now(
            ReplayClass::CredentialMissing,
            "ModelScope 站点还没填登录态（网页 Cookie）".to_string(),
        ));
    };
    let vault = ctx.vault.read().map_err(|err| {
        failure_now(
            ReplayClass::CredentialMissing,
            format!("保险库不可用：{err}"),
        )
    })?;
    let secret = vault.get_credential(credential_id).map_err(|_| {
        failure_now(
            ReplayClass::CredentialMissing,
            "保险库里没有这条登录态（可能已被删）".to_string(),
        )
    })?;
    Ok(String::from_utf8_lossy(secret.as_bytes()).to_string())
}

/// 读一条凭据（**拿不到就返回 `None`**，不报错——用于"可选凭据"）。
fn read_credential_quiet(ctx: &PluginContext<'_>, credential_id: &str) -> Option<String> {
    let vault = ctx.vault.read().ok()?;
    let secret = vault.get_credential(credential_id).ok()?;
    Some(String::from_utf8_lossy(secret.as_bytes()).to_string())
}

/// 立刻造一个失败结果（耗时 0）。
fn failure_now(class: ReplayClass, message: String) -> ReplayOutcome {
    ReplayOutcome {
        ok: false,
        class,
        message,
        status: None,
        elapsed_ms: 0,
        retryable: class.retryable_by_default(),
        body_preview: None,
        reward: None,
    }
}

/// 造一个失败结果（带耗时与状态码）。
fn failure(
    class: ReplayClass,
    message: String,
    status: Option<u16>,
    started: Instant,
) -> ReplayOutcome {
    ReplayOutcome {
        ok: false,
        class,
        message,
        status,
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
    fn anonymous_signin_is_a_credential_problem() {
        // **实测口径**：匿名态是 HTTP 401 +「禁止访问」
        let body = r#"{"Code":10019901001,"Message":"禁止访问","Success":false}"#.as_bytes();
        match evaluate_signin(401, body) {
            SigninVerdict::Status(ReplayClass::CredentialInvalid, message) => {
                assert!(message.contains("登录态"), "{message}");
            }
            other => panic!("匿名态应判凭据失效，实得 {other:?}"),
        }
    }

    #[test]
    fn business_failure_is_not_a_status_problem() {
        // 2xx 但 Success=false → 断言不符（不是状态码问题）
        let body = r#"{"Code":10019901001,"Message":"禁止访问","Success":false}"#.as_bytes();
        match evaluate_signin(200, body) {
            SigninVerdict::Business(message) => {
                assert!(message.contains("禁止访问"), "{message}");
            }
            other => panic!("业务失败应给中文原因，实得 {other:?}"),
        }
    }

    #[test]
    fn success_needs_the_success_field() {
        let ok = br#"{"Code":200,"Data":{"UserName":"u"},"Message":"success","Success":true}"#;
        assert_eq!(evaluate_signin(200, ok), SigninVerdict::Success);
        // 没有 Success 字段 → 不算成功（不能默认当成成功）
        let missing = br#"{"Code":200,"Data":{}}"#;
        assert!(matches!(
            evaluate_signin(200, missing),
            SigninVerdict::Business(_)
        ));
    }

    #[test]
    fn rate_limit_and_http_error_are_classified() {
        assert!(matches!(
            evaluate_signin(429, b"{}"),
            SigninVerdict::Status(ReplayClass::RateLimited, _)
        ));
        assert!(matches!(
            evaluate_signin(500, b"{}"),
            SigninVerdict::Status(ReplayClass::HttpError, _)
        ));
    }

    #[test]
    fn non_json_body_is_reported() {
        assert!(matches!(
            evaluate_signin(200, b"<html>login</html>"),
            SigninVerdict::NotJson(_)
        ));
    }

    #[test]
    fn reward_extraction_tries_candidates_in_order() {
        let json: Value = serde_json::from_str(r#"{"data":{"points":400}}"#).unwrap();
        assert_eq!(extract_reward(&json, None), Some(400));
        // 显式路径优先
        assert_eq!(extract_reward(&json, Some("data.points")), Some(400));
        assert_eq!(extract_reward(&json, Some("data.balance")), None);
        // 别的形状
        let other: Value = serde_json::from_str(r#"{"balance":1200}"#).unwrap();
        assert_eq!(extract_reward(&other, None), Some(1200));
        // 取不到 → None（不猜）
        let empty: Value = serde_json::from_str(r#"{"foo":1}"#).unwrap();
        assert_eq!(extract_reward(&empty, None), None);
    }
}
