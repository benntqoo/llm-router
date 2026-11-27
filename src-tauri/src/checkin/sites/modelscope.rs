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
//! **成功路径也已实测**（2026-11-27，登录态真机）：
//! - **访问令牌**（`ms-…`）当 `Cookie: m_session_id=<令牌>` 用 → 签到接口 **200**、
//!   `Success = true`、`Message = "success"`（**277 ms**）；**同一个令牌**也能当
//!   `/openapi/v1/*` 的 Bearer——**网页会话 Cookie 也能开 `/openapi/v1/*`**（回同一份流水）。
//! - 魔粒流水的真实形状：`data.records[]`，**金额 = `total_amount`**、
//!   **日期 = `gmt_created`**（只有日期）、**类型 = `type`**、**规则 = `rule_key`**。
//!
//! **奖励口径**：**签到前后的差值**——当日 `rule_key = daily_active` 的魔粒总额，
//! 签到前取一次（基线）、签到后再取一次，**差值 > 0 才是本次拿到**；差值 = 0 →
//! **「今天已领过」**（`points = null`、不回写额度池）。详见 [`daily_magicubes`]。
//!
//! **凭据**：访问令牌 / 网页 Cookie（**必需**）。查不到魔粒**不影响签到成功**——
//! **不因为查不到积分就判签到失败**。

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
    "访问令牌（魔搭 → 个人中心 → 访问令牌 → 新建 → 复制，`ms-` 开头）——同一个令牌既能签到也能查魔粒";
/// 默认 cron。
pub const DEFAULT_SCHEDULE: &str = "0 9 * * *";

/// 站点根地址（可被站点配置覆盖——测试 / 自建站点用）。
pub const DEFAULT_BASE_URL: &str = "https://www.modelscope.cn";
/// 签到接口（触发当日签到）。
pub const SIGNIN_PATH: &str = "/api/v1/users/login/info";
/// 魔粒余额接口（开放平台；Bearer）。
pub const BALANCE_PATH: &str = "/openapi/v1/magicubes/balance";
/// **魔粒流水**接口（开放平台；Bearer）——**当日到账**从这儿算。
pub const TRANSACTIONS_PATH: &str = "/openapi/v1/magicubes/transactions";

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

/// **会话 Cookie** 由密文推导（**纯函数**，单测直接打）。
///
/// 密文有两种形态（都支持）：
/// 1. **访问令牌**（`ms-…`，不含 `=`）→ 拼成 **`m_session_id=<令牌>`**——
///    魔搭的开放平台访问令牌**可以直接当网页会话用**（第三方魔搭签到脚本的实测口径：
///    `Cookie: m_session_id=<ms-token>` → `GET /api/v1/users/login/info` 即触发当日签到）；
/// 2. **原始 Cookie 头**（含 `name=value`）→ 原样使用（老路径，保留）。
///
/// **为什么主推令牌**：手抄 Cookie 要用户开 F12 找请求头——「不是所有人都会获取 cookies」；
/// 而令牌在魔搭「个人中心 → 访问令牌」**点一下复制**就有，**不用 F12、不用人机验证、
/// 不用让应用去开自动化浏览器**（魔搭的登录人机验证专门拦自动化浏览器，而反检测是 §5.7 红线）。
pub fn session_cookie(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.contains('=') {
        trimmed.to_string()
    } else {
        format!("m_session_id={trimmed}")
    }
}

/// 这个密文是不是**访问令牌**（而不是原始 Cookie 头）。
pub fn is_access_token(secret: &str) -> bool {
    let trimmed = secret.trim();
    !trimmed.is_empty() && !trimmed.contains('=')
}

/// **开放平台 Bearer**（查魔粒用）——只有令牌形态才有。
pub fn bearer_token(secret: &str) -> Option<String> {
    if is_access_token(secret) {
        Some(secret.trim().to_string())
    } else {
        None
    }
}

/// **每日签到**那一条流水的规则键（真机实测 2026-11-27）。
///
/// 当日流水里还有 `infocomplement_profile_experience` / `infocomplement_email_verification`
/// 这类**一次性资料奖励**——它们不是「每日签到拿到的那笔」，**不算进奖励**。
pub const DAILY_RULE_KEY: &str = "daily_active";

/// **签到前后差值**的判定结果（**纯函数**，单测直接打）。
#[derive(Debug, Clone, PartialEq)]
pub enum DiffOutcome {
    /// 本次到账（差额 = 魔粒数）。
    Gained { amount: i64 },
    /// 今天已领过（当日总额没有增加）。
    AlreadyClaimed { daily_total: i64 },
    /// 查不到当日魔粒（基线或事后取不到）——**不猜**。
    Unknown,
}

/// 用**签到前后的差值**算「本次拿到多少」（**纯函数**）。
///
/// 口径（02 §7.4 §2）：当日那条 `daily_active` 流水**整天都在** → 只有差值才是
/// 「本次到账」；差值 = 0 → **今天已领过**（`points = null`、不回写额度池）。
pub fn reward_from_diff(before: Option<i64>, after: Option<i64>) -> DiffOutcome {
    match (before, after) {
        (Some(before), Some(after)) if after > before => DiffOutcome::Gained {
            amount: after - before,
        },
        (Some(_), Some(after)) => DiffOutcome::AlreadyClaimed {
            daily_total: after,
        },
        _ => DiffOutcome::Unknown,
    }
}

/// 当日**每日签到**魔粒的总额（**只认 `rule_key = daily_active`**）+ 命中的原始记录。
///
/// **口径（真机实测 2026-11-27）**：金额字段 = **`total_amount`**（不是 `amount` /
/// `points`）；日期字段 = **`gmt_created`**（**只有日期、没有时分秒**）；类型 =
/// `type = EARN`；规则 = `rule_key`。
///
/// **为什么要「当日总额」而不是「本次到账」**：当日那条 `daily_active` 流水
/// **整天都在**（重复签到不会让它消失）→ 直接取当日总额，会把**同一天的第 2、3 次运行**
/// 也当成新到账，**额度池被重复计数**。所以调用方要拿**签到前后的差值**
/// （见 [`ModelScopePlugin::run`]）。
///
/// 返回 `None` = **今天没有每日签到那一笔**（或对账接口没给出可识别的形状）——**不猜**。
pub fn daily_magicubes(json: &Value, today: &str) -> Option<(i64, Value)> {
    let mut total: i64 = 0;
    let mut found = false;
    let mut records: Vec<Value> = Vec::new();
    collect_daily_magicubes(json, today, &mut total, &mut found, &mut records, 0);
    if !found {
        return None;
    }
    Some((total, Value::Array(records)))
}

/// 递归收集「今天的每日签到魔粒」（**纯函数**）。
fn collect_daily_magicubes(
    value: &Value,
    today: &str,
    total: &mut i64,
    found: &mut bool,
    records: &mut Vec<Value>,
    depth: usize,
) {
    if depth > 8 || value.is_null() {
        return;
    }
    if let Some(items) = value.as_array() {
        for item in items {
            // 只有**数组里的元素**才算一条流水记录（避免父对象被重复计一次）
            if let Some(object) = item.as_object() {
                if is_daily_record(object, today) {
                    if let Some(amount) = record_amount(object) {
                        *total += amount;
                        *found = true;
                        records.push(item.clone());
                    }
                }
            }
            collect_daily_magicubes(item, today, total, found, records, depth + 1);
        }
        return;
    }
    if let Some(object) = value.as_object() {
        for item in object.values() {
            collect_daily_magicubes(item, today, total, found, records, depth + 1);
        }
    }
}

/// 这条流水是不是「今天的每日签到那一笔」（**纯函数**）。
fn is_daily_record(object: &serde_json::Map<String, Value>, today: &str) -> bool {
    // 类型 = EARN（大小写不敏感）
    let is_earn = object
        .get("type")
        .and_then(Value::as_str)
        .map(|kind| kind.eq_ignore_ascii_case("EARN"))
        .unwrap_or(false);
    if !is_earn {
        return false;
    }
    // 规则 = daily_active（**资料类一次性奖励不算**）
    let rule_ok = object
        .get("rule_key")
        .or_else(|| object.get("ruleKey"))
        .and_then(Value::as_str)
        .map(|rule| rule == DAILY_RULE_KEY)
        .unwrap_or(false);
    if !rule_ok {
        return false;
    }
    object_has_today(object, today)
}

/// 这条流水的金额（**实测字段 `total_amount` 在最前**，其余是兜底候选）。
fn record_amount(object: &serde_json::Map<String, Value>) -> Option<i64> {
    ["total_amount", "amount", "magicubes", "points", "value", "change", "delta"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_i64))
}

/// 这条记录是不是**今天**的（字符串日期或 unix 时间戳都认）。
fn object_has_today(object: &serde_json::Map<String, Value>, today: &str) -> bool {
    let compact = today.replace('-', "");
    for key in [
        "gmt_created",
        "create_time",
        "createTime",
        "date",
        "day",
        "gmt_create",
        "time",
        "update_time",
        "occur_time",
    ] {
        let Some(raw) = object.get(key) else { continue };
        if let Some(text) = raw.as_str() {
            if text.contains(today) || text.replace('-', "").contains(&compact) {
                return true;
            }
            continue;
        }
        if let Some(stamp) = raw.as_i64() {
            // unix 秒（或毫秒）落在今天
            let seconds = if stamp > 1_000_000_000_000 { stamp / 1000 } else { stamp };
            if let Some(date) = chrono::DateTime::from_timestamp(seconds, 0) {
                let local = date.with_timezone(&chrono::Local);
                if local.format("%Y-%m-%d").to_string() == today {
                    return true;
                }
            }
        }
    }
    false
}

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
                "ModelScope 登录态无效（令牌过期或没填）——去魔搭「个人中心 → 访问令牌」重新复制一个".to_string(),
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

        // 1) 登录态（必需）：**访问令牌**（`ms-…`）或**原始 Cookie 头**
        let secret = match read_credential(ctx, ctx.config.credential_id.as_deref()) {
            Ok(value) => value,
            Err(outcome) => return outcome,
        };
        // 令牌 → `m_session_id=<令牌>`；原始 Cookie 头 → 原样用
        let cookie = session_cookie(&secret);

        // 2) 签到**前**的基线：当日每日签到魔粒总额
        //
        // **为什么要基线**：站点**不返回**「今日魔粒 / 今天已领」信号（签到响应体只是
        // 用户资料），而当日那条 `daily_active` 流水**整天都在** → 只有**签到前后的差值**
        // 才是「本次拿到多少」（02 §7.4 §2）。
        let explicit_path = ctx.config.reward_path.clone();
        let before = match explicit_path {
            Some(_) => None,
            None => self.daily_total(ctx, base, &cookie, &secret).await,
        };

        // 3) 签到
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

        // 4) 判定
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

        // 5) 奖励（**可选**；查不到不影响签到成功）
        let (reward, message) = self.resolve_reward(ctx, base, &cookie, &secret, before, explicit_path).await;
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
    /// 算出「本次拿到多少」+ 回执文案（**查不到也给成功**，只是没积分）。
    ///
    /// 两种口径：
    /// - **站点显式配了 `reward.path`** → 直接按路径取（**用户自己说了算**，不做差值）；
    /// - **默认** → **签到前后差值**：差值 > 0 = 本次到账；差值 = 0 = **今天已领过**
    ///   （`points = null`、**不回写额度池**，与 MiniMax Code 的口径一致）。
    async fn resolve_reward(
        &self,
        ctx: &PluginContext<'_>,
        base: &str,
        cookie: &str,
        secret: &str,
        before: Option<(i64, Value)>,
        explicit_path: Option<String>,
    ) -> (Option<Reward>, String) {
        if let Some(path) = explicit_path.as_deref() {
            return match self.transactions_json(ctx, base, cookie, secret).await {
                Some(json) => match extract_reward(&json, Some(path)) {
                    Some(amount) => (
                        Some(Reward::new(amount, "magicube", json)),
                        format!("ModelScope 签到成功（魔粒 +{amount}）"),
                    ),
                    None => (
                        None,
                        format!("ModelScope 签到成功（站点指定的积分路径 `{path}` 没取到数）"),
                    ),
                },
                None => (
                    None,
                    "ModelScope 签到成功（对账接口没回，查不到积分）".to_string(),
                ),
            };
        }
        let after = self.daily_total(ctx, base, cookie, secret).await;
        let before_total = before.as_ref().map(|(total, _)| *total);
        let after_total = after.as_ref().map(|(total, _)| *total);
        let records = after
            .as_ref()
            .map(|(_, records)| records.clone())
            .unwrap_or(Value::Null);
        match reward_from_diff(before_total, after_total) {
            DiffOutcome::Gained { amount } => {
                let raw = serde_json::json!({
                    "daily_active_total": amount,
                    "records": records,
                });
                (
                    Some(Reward::new(amount, "magicube", raw)),
                    format!("ModelScope 签到成功（本次到账 +{amount} 魔粒）"),
                )
            }
            DiffOutcome::AlreadyClaimed { daily_total } => (
                None,
                format!(
                    "ModelScope 签到成功（今天已领过：当日每日签到魔粒 {daily_total}，本次没有新增）"
                ),
            ),
            DiffOutcome::Unknown => (
                None,
                "ModelScope 签到成功（查不到当日魔粒：对账接口没回或形状变了）".to_string(),
            ),
        }
    }

    /// 取**当日每日签到魔粒的总额** + 命中的原始记录。
    ///
    /// **`None` = 查不到**（对账接口没回：网络失败 / 401 / 形状变了）；
    /// **`Some((0, …))` = 对账接口回了，但今天还没有每日签到那一笔**——**基线就是 0**，
    /// 不是「查不到」（否则第一次签到会拿不到本次到账的差额）。
    async fn daily_total(
        &self,
        ctx: &PluginContext<'_>,
        base: &str,
        cookie: &str,
        secret: &str,
    ) -> Option<(i64, Value)> {
        let json = self.transactions_json(ctx, base, cookie, secret).await?;
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        Some(
            daily_magicubes(&json, &today)
                .unwrap_or((0, Value::Array(Vec::new()))),
        )
    }

    /// 魔粒流水接口的响应体（**Bearer 优先，回落网页会话 Cookie**；失败 → `None`）。
    ///
    /// **实测**：两种认证都回**同一份流水**（Cookie 形态也能开 `/openapi/v1/*`）。
    async fn transactions_json(
        &self,
        ctx: &PluginContext<'_>,
        base: &str,
        cookie: &str,
        secret: &str,
    ) -> Option<Value> {
        // Bearer：令牌形态的密文优先；否则用站点另配的开放平台 token
        let token = bearer_token(secret).or_else(|| {
            ctx.config
                .token_credential_id
                .as_deref()
                .and_then(|id| read_credential_quiet(ctx, id))
        });
        let mut request = ctx
            .client
            .get(format!("{base}{TRANSACTIONS_PATH}"))
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
        serde_json::from_slice(&bytes).ok()
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
            "ModelScope 站点还没填登录态——把魔搭「个人中心 → 访问令牌」里的 `ms-` 令牌粘进来".to_string(),
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

    #[test]
    fn an_access_token_becomes_the_session_cookie() {
        // **实测口径**：令牌（ms-…）→ `m_session_id=<令牌>`
        assert_eq!(session_cookie("ms-abc123"), "m_session_id=ms-abc123");
        assert_eq!(session_cookie("  ms-abc123  "), "m_session_id=ms-abc123");
        // 原始 Cookie 头 → 原样用（老路径保留）
        assert_eq!(session_cookie("a=1; b=2"), "a=1; b=2");
        // 判定：含 `=` 的是 Cookie 头，不含的是令牌
        assert!(is_access_token("ms-abc123"));
        assert!(!is_access_token("a=1"));
        assert!(!is_access_token("   "));
        assert_eq!(bearer_token("ms-abc123").as_deref(), Some("ms-abc123"));
        assert!(bearer_token("a=1").is_none());
    }

    #[test]
    fn measured_shape_counts_only_the_daily_active_record() {
        // **真机实测形状**（2026-11-27）：`data.records[]`，金额 = `total_amount`，
        // 日期 = `gmt_created`，类型 = `type`，规则 = `rule_key`。
        let json: Value = serde_json::from_str(
            r#"{"data":{"page":1,"page_size":20,"total":3,"records":[
                {"type":"EARN","rule_key":"daily_active","total_amount":200,"count":1,"gmt_created":"2026-11-27","expire_days":1},
                {"type":"EARN","rule_key":"infocomplement_profile_experience","total_amount":50,"count":1,"gmt_created":"2026-11-27"},
                {"type":"EARN","rule_key":"infocomplement_email_verification","total_amount":50,"count":1,"gmt_created":"2026-11-27"},
                {"type":"EARN","rule_key":"daily_active","total_amount":200,"count":1,"gmt_created":"2026-11-26"}
            ]},"success":true}"#,
        )
        .unwrap();
        // **只算当日 daily_active 那一条** = 200（资料类一次性奖励 50+50 不算；昨天的也不算）
        let (total, records) = daily_magicubes(&json, "2026-11-27").expect("应有当日每日签到记录");
        assert_eq!(total, 200);
        assert_eq!(records.as_array().map(|a| a.len()), Some(1));
        // 换一天 → 当天没有每日签到记录 → None（不猜）
        assert!(daily_magicubes(&json, "2026-11-28").is_none());
        // 空壳 → None
        let empty: Value = serde_json::from_str(r#"{"data":{}}"#).unwrap();
        assert!(daily_magicubes(&empty, "2026-11-27").is_none());
    }

    #[test]
    fn unix_timestamps_count_as_today_too() {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let stamp = chrono::Local::now().timestamp();
        let json = serde_json::json!({
            "data": [{"type": "EARN", "rule_key": "daily_active", "amount": 42, "create_time": stamp}]
        });
        let (total, _) = daily_magicubes(&json, &today).expect("unix 时间戳也算今天");
        assert_eq!(total, 42);
    }

    #[test]
    fn the_signin_diff_decides_how_much_this_run_earned() {
        // 差值 > 0 → 本次到账
        assert_eq!(
            reward_from_diff(Some(0), Some(200)),
            DiffOutcome::Gained { amount: 200 }
        );
        // 差值 = 0 → 今天已领过（当日总额还在）
        assert_eq!(
            reward_from_diff(Some(200), Some(200)),
            DiffOutcome::AlreadyClaimed { daily_total: 200 }
        );
        // 取不到基线 / 取不到事后 → 查不到（不猜、不冒进）
        assert_eq!(reward_from_diff(None, Some(200)), DiffOutcome::Unknown);
        assert_eq!(reward_from_diff(Some(0), None), DiffOutcome::Unknown);
    }
}
