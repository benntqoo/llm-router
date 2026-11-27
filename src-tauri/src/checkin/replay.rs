//! 重放引擎（T16；02 §7.1.3）。
//!
//! **一句话**：模板 + 保险库里的真实取值 → 拼成一次 HTTP 请求 → 断言响应 → 出一个
//! **错误分类** →（由调用方）写一行 `checkin_result`。
//!
//! 本模块只做引擎：**不含**调度（T17）、通知（T19）、浏览器自动化辅轨、UI 触发入口。
//!
//! 密文边界（AGENT.md §5.3）：
//! - 敏感变量的真实值由调用方通过 [`SecretResolver`] 从保险库取，**只在注入那一步存在**；
//! - 响应体片段（[`ReplayOutcome::body_preview`]）**只在内存里**，不落 `checkin_result`
//!   ——响应体可能带 csrf / token 一类密文。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::har::{json_lookup, SiteTemplate, TemplateAssert};

/// 连接超时（秒）。
pub const CONNECT_TIMEOUT_SECS: u64 = 10;

/// 整次请求超时（秒）。
pub const REQUEST_TIMEOUT_SECS: u64 = 30;

/// 失败时保留的响应体片段上限（字节；**只留内存，不落库**）。
pub const BODY_PREVIEW_LIMIT: usize = 512;

/// 断言用的响应体上限（字节；超出会截断）。
pub const MAX_ASSERT_BODY_BYTES: usize = 64 * 1024;

/// 重放结果的错误分类（引擎的唯一输出口径；02 §7.1.3 §4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayClass {
    /// 断言全过。
    Success,
    /// 状态码不符 / JSON 路径不符。
    AssertFailed,
    /// 连接失败 / 超时 / 读响应体失败。
    NetworkError,
    /// 模板变量的凭据缺失。
    CredentialMissing,
    /// 模板不可用（JSON 解析失败 / 未知占位符 / 断言配置非法）。
    TemplateInvalid,
    /// 上游 401 / 403。
    CredentialInvalid,
    /// 上游 429。
    RateLimited,
    /// 其它 4xx / 5xx。
    HttpError,
}

impl ReplayClass {
    /// 分类的稳定字符串（落库 / IPC 用）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ReplayClass::Success => "success",
            ReplayClass::AssertFailed => "assert_failed",
            ReplayClass::NetworkError => "network_error",
            ReplayClass::CredentialMissing => "credential_missing",
            ReplayClass::TemplateInvalid => "template_invalid",
            ReplayClass::CredentialInvalid => "credential_invalid",
            ReplayClass::RateLimited => "rate_limited",
            ReplayClass::HttpError => "http_error",
        }
    }

    /// 解析分类字符串（落库反读用）。
    pub fn from_str_value(raw: &str) -> Option<ReplayClass> {
        match raw {
            "success" => Some(ReplayClass::Success),
            "assert_failed" => Some(ReplayClass::AssertFailed),
            "network_error" => Some(ReplayClass::NetworkError),
            "credential_missing" => Some(ReplayClass::CredentialMissing),
            "template_invalid" => Some(ReplayClass::TemplateInvalid),
            "credential_invalid" => Some(ReplayClass::CredentialInvalid),
            "rate_limited" => Some(ReplayClass::RateLimited),
            "http_error" => Some(ReplayClass::HttpError),
            _ => None,
        }
    }

    /// 是否**可能**重试（02 §3.3 判据；`http_error` 的 4xx/5xx 由状态码另判）。
    pub fn retryable_by_default(&self) -> bool {
        matches!(self, ReplayClass::NetworkError | ReplayClass::RateLimited)
    }
}

impl std::fmt::Display for ReplayClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 引擎的一次失败（注入阶段或断言阶段）。
#[derive(Debug, Clone)]
pub struct ReplayFailure {
    pub class: ReplayClass,
    pub message: String,
    /// 是否可重试。
    pub retryable: bool,
}

impl ReplayFailure {
    fn new(class: ReplayClass, message: impl Into<String>) -> Self {
        ReplayFailure {
            class,
            message: message.into(),
            retryable: class.retryable_by_default(),
        }
    }

    fn retryable(class: ReplayClass, message: impl Into<String>) -> Self {
        ReplayFailure {
            class,
            message: message.into(),
            retryable: true,
        }
    }
}

/// 一次重放的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayOutcome {
    /// 是否签到成功（**仅当**分类为 `success`）。
    pub ok: bool,
    /// 错误分类。
    pub class: ReplayClass,
    /// 人类可读的中文原因（T19 通知直接用）。
    pub message: String,
    /// 上游状态码（没拿到响应则为 `None`）。
    pub status: Option<u16>,
    /// 耗时（毫秒）。
    pub elapsed_ms: u64,
    /// 是否可重试（T17 调度层用）。
    pub retryable: bool,
    /// 失败时的响应体片段（**只在内存里，不落库**）。
    pub body_preview: Option<String>,
    /// 本次拿到的积分（**T18 才有**；HAR 重放恒为 `None`）→ 落 `checkin_result.reward_json`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reward: Option<crate::checkin::reward::Reward>,
}

impl ReplayOutcome {
    fn failure(failure: ReplayFailure, elapsed_ms: u64) -> Self {
        ReplayOutcome {
            ok: false,
            class: failure.class,
            message: failure.message,
            status: None,
            elapsed_ms,
            retryable: failure.retryable,
            body_preview: None,
            reward: None,
        }
    }
}

/// 取凭据时的失败原因。
#[derive(Debug, Clone)]
pub enum ResolveError {
    /// 凭据不存在（没录 / 被删）。
    NotFound,
    /// 保险库不可用（锁着 / 解密失败）。
    Unavailable(String),
}

/// 敏感变量的取值来源（由调用方接到保险库上）。
///
/// `Send + Sync`：重放是异步的，解析器要能跨线程（tokio 的任务要求 future 是 `Send`）。
pub trait SecretResolver: Send + Sync {
    /// 取一个凭据的真实值；`Err` 表示拿不到（**不区分原因给调用方看**，只给调用方看错误信息）。
    fn resolve(&self, credential_id: &str) -> Result<String, ResolveError>;
}

/// 注入后的请求（不含任何模板占位符）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// 请求体原文（`None` = 无请求体）。
    pub body: Option<String>,
}

/// 重放引擎。
#[derive(Debug, Clone)]
pub struct ReplayEngine {
    client: reqwest::Client,
}

impl ReplayEngine {
    /// 构造引擎（超时按 02 §7.1.3：连接 10s / 整请求 30s）。
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent(concat!("llm-router/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(ReplayEngine { client })
    }

    /// 用自定义超时构造引擎（测试用：把超时压到毫秒级）。
    pub fn with_timeouts(connect: Duration, request: Duration) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(request)
            .user_agent(concat!("llm-router/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(ReplayEngine { client })
    }

    /// HTTP 客户端（**给内置站点插件用**：T18 的 ModelScope 插件自己发请求，
    /// 但复用引擎同一套超时口径——连接 10s / 整请求 30s）。
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// 重放一次签到请求。
    ///
    /// 引擎**自己不重试**（重试与冷却属 T17 调度层；02 §7.1.3 §2）。
    pub async fn replay(
        &self,
        template: &SiteTemplate,
        resolver: &dyn SecretResolver,
    ) -> ReplayOutcome {
        let started = Instant::now();
        let request = match inject(template, resolver) {
            Ok(request) => request,
            Err(failure) => return ReplayOutcome::failure(failure, started.elapsed().as_millis() as u64),
        };

        let method = match reqwest::Method::from_bytes(request.method.as_bytes()) {
            Ok(method) => method,
            Err(_) => {
                return ReplayOutcome::failure(
                    ReplayFailure::new(
                        ReplayClass::TemplateInvalid,
                        format!("模板里的请求方法不合法: {}", request.method),
                    ),
                    started.elapsed().as_millis() as u64,
                )
            }
        };
        let url = match reqwest::Url::parse(&request.url) {
            Ok(url) => url,
            Err(err) => {
                return ReplayOutcome::failure(
                    ReplayFailure::new(
                        ReplayClass::TemplateInvalid,
                        format!("模板里的 URL 不合法: {}（{err}）", request.url),
                    ),
                    started.elapsed().as_millis() as u64,
                )
            }
        };

        let mut builder = self.client.request(method, url);
        for (name, value) in &request.headers {
            let header_name = match reqwest::header::HeaderName::from_bytes(name.as_bytes()) {
                Ok(name) => name,
                Err(_) => {
                    return ReplayOutcome::failure(
                        ReplayFailure::new(
                            ReplayClass::TemplateInvalid,
                            format!("模板里的请求头名不合法: {name}"),
                        ),
                        started.elapsed().as_millis() as u64,
                    )
                }
            };
            let header_value = match reqwest::header::HeaderValue::from_str(value) {
                Ok(value) => value,
                Err(_) => {
                    return ReplayOutcome::failure(
                        ReplayFailure::new(
                            ReplayClass::TemplateInvalid,
                            format!("模板里的请求头值不合法: {name}"),
                        ),
                        started.elapsed().as_millis() as u64,
                    )
                }
            };
            builder = builder.header(header_name, header_value);
        }
        if let Some(body) = request.body.clone() {
            builder = builder.body(body);
        }

        let response = match builder.send().await {
            Ok(response) => response,
            Err(err) => {
                // 连接失败 / 超时 / 读响应头失败 —— 02 §7.1.3：网络类一律可重试
                let message = if err.is_timeout() {
                    format!(
                        "网络失败：请求超时（整请求上限 {}s）",
                        REQUEST_TIMEOUT_SECS
                    )
                } else if err.is_connect() {
                    format!("网络失败：连不上上游（{err}）")
                } else {
                    format!("网络失败：{err}")
                };
                return ReplayOutcome::failure(
                    ReplayFailure::retryable(ReplayClass::NetworkError, message),
                    started.elapsed().as_millis() as u64,
                );
            }
        };

        let status = response.status().as_u16();
        let raw_body = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => {
                return ReplayOutcome {
                    ok: false,
                    class: ReplayClass::NetworkError,
                    message: format!("网络失败：读上游响应体失败（{err}）"),
                    status: Some(status),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                    retryable: true,
                    body_preview: None,
                    reward: None,
                };
            }
        };
        let body: Vec<u8> = raw_body
            .iter()
            .copied()
            .take(MAX_ASSERT_BODY_BYTES)
            .collect();

        let mut outcome = evaluate(template, status, &body);
        outcome.elapsed_ms = started.elapsed().as_millis() as u64;
        if !outcome.ok {
            outcome.body_preview = preview(&body);
        }
        outcome
    }
}

/// 断言 + 分类（纯函数；单测直接打）。
pub fn evaluate(template: &SiteTemplate, status: u16, body: &[u8]) -> ReplayOutcome {
    let assertion = match check_assertion(template, status, body) {
        Ok(summary) => {
            let outcome = ReplayOutcome {
                ok: true,
                class: ReplayClass::Success,
                message: summary,
                status: Some(status),
                elapsed_ms: 0,
                retryable: false,
                body_preview: None,
                reward: None,
            };
            return outcome;
        }
        Err(failure) => failure,
    };

    // 断言没过时，用状态码给出**更可操作**的原因（02 §7.1.3 §4）
    let failure = refine_by_status(assertion, status);

    let mut outcome = ReplayOutcome::failure(failure, 0);
    outcome.status = Some(status);
    outcome
}

/// 按上游状态码把断言失败细化成更可操作的分类（02 §7.1.3 §4）。
fn refine_by_status(failure: ReplayFailure, status: u16) -> ReplayFailure {
    let message = failure.message.clone();
    match status {
        401 | 403 => ReplayFailure::new(
            ReplayClass::CredentialInvalid,
            format!("上游返回 {status}：凭证可能已失效（{message}）"),
        ),
        429 => ReplayFailure::retryable(
            ReplayClass::RateLimited,
            format!("上游返回 {status}：被限流（{message}）"),
        ),
        500..=599 => ReplayFailure::retryable(
            ReplayClass::HttpError,
            format!("上游返回 {status}：上游服务异常（{message}）"),
        ),
        400..=499 => ReplayFailure::new(
            ReplayClass::HttpError,
            format!("上游返回 {status}（{message}）"),
        ),
        _ => failure,
    }
}

/// 跑一遍断言；通过返回一段中文摘要，失败返回 [`ReplayFailure`]。
pub fn check_assertion(
    template: &SiteTemplate,
    status: u16,
    body: &[u8],
) -> Result<String, ReplayFailure> {
    let assert = &template.assert;

    if let Some(expected) = assert.status {
        if expected != status as i64 {
            return Err(ReplayFailure::new(
                ReplayClass::AssertFailed,
                format!("状态码不符：期望 {expected}，实际 {status}"),
            ));
        }
    }

    for rule in &assert.json_paths {
        let value = match serde_json::from_slice::<serde_json::Value>(body) {
            Ok(value) => value,
            Err(_) => {
                return Err(ReplayFailure::new(
                    ReplayClass::AssertFailed,
                    format!(
                        "JSON 路径 {}：上游响应不是 JSON，无法断言",
                        rule.path
                    ),
                ))
            }
        };
        let actual = match json_lookup(&value, &rule.path) {
            Some(actual) => actual,
            None => {
                return Err(ReplayFailure::new(
                    ReplayClass::AssertFailed,
                    format!("JSON 路径 {}：响应里不存在这个路径", rule.path),
                ))
            }
        };
        if let Some(expected) = &rule.equals {
            if actual != expected {
                return Err(ReplayFailure::new(
                    ReplayClass::AssertFailed,
                    format!(
                        "JSON 路径 {}：期望 {}，实际 {}",
                        rule.path,
                        expected,
                        actual
                    ),
                ));
            }
        }
    }

    // 没配任何断言 → 只判"请求没出错且状态码是 2xx/3xx"
    if assert.status.is_none() && assert.json_paths.is_empty() {
        if !(200..400).contains(&status) {
            return Err(ReplayFailure::new(
                ReplayClass::HttpError,
                format!("上游返回 {status}"),
            ));
        }
    }

    let mut summary = format!("签到成功（HTTP {status}）");
    if !assert.json_paths.is_empty() {
        summary.push_str(&format!("，JSON 路径断言 {} 条全过", assert.json_paths.len()));
    }
    Ok(summary)
}

/// 注入变量并构造请求（纯函数；单测直接打）。
///
/// 失败分类：`credential_missing`（拿不到凭据）/ `template_invalid`（未知占位符、值缺失）。
pub fn inject(
    template: &SiteTemplate,
    resolver: &dyn SecretResolver,
) -> Result<InjectedRequest, ReplayFailure> {
    // 1) 取每个变量的真实值
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for variable in &template.variables {
        let value = if variable.sensitive {
            let credential_id = variable.credential_id.as_deref().unwrap_or("");
            if credential_id.is_empty() {
                return Err(ReplayFailure::new(
                    ReplayClass::CredentialMissing,
                    format!("变量 {} 没有关联凭据（模板未标 credentialId）", variable.name),
                ));
            }
            match resolver.resolve(credential_id) {
                Ok(value) => value,
                Err(ResolveError::NotFound) => {
                    return Err(ReplayFailure::new(
                        ReplayClass::CredentialMissing,
                        format!(
                            "变量 {} 缺少凭据：{credential_id} 不存在（没录或已删除）",
                            variable.name
                        ),
                    ))
                }
                Err(ResolveError::Unavailable(reason)) => {
                    return Err(ReplayFailure::new(
                        ReplayClass::CredentialMissing,
                        format!("变量 {} 取不到凭据：{reason}", variable.name),
                    ))
                }
            }
        } else {
            match variable.default.clone() {
                Some(value) => value,
                None => {
                    return Err(ReplayFailure::new(
                        ReplayClass::TemplateInvalid,
                        format!("非敏感变量 {} 没有默认值", variable.name),
                    ))
                }
            }
        };
        values.insert(variable.name.clone(), value);
    }

    // 2) 替换占位符（值内部替换；未知的占位符直接拒绝重放）
    let replace = |raw: &str| -> Result<String, ReplayFailure> {
        substitute(raw, &values)
    };

    // 3) URL：先剥掉自带 query，再按模板的 query 数组重建（避免同一个参数发两遍）
    let (base_url, _) = split_query(&template.request.url);
    let base_url = replace(&base_url)?;
    let mut url = match reqwest::Url::parse(&base_url) {
        Ok(url) => url,
        Err(err) => {
            return Err(ReplayFailure::new(
                ReplayClass::TemplateInvalid,
                format!("模板里的 URL 不合法：{base_url}（{err}）"),
            ))
        }
    };
    // 先把 URL 自带的 query 摘掉（模板数组为空时也不能留下一个空 query）
    url.set_query(None);
    if !template.request.query.is_empty() {
        let mut query = url.query_pairs_mut();
        for param in &template.request.query {
            let value = replace(&param.value)?;
            query.append_pair(&param.name, &value);
        }
    }

    // 4) 请求头（剔除 Host / Content-Length：由客户端按实际 URL 与请求体生成）
    let mut headers: Vec<(String, String)> = Vec::new();
    for header in &template.request.headers {
        let name = header.name.trim();
        if name.eq_ignore_ascii_case("host") || name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        headers.push((name.to_string(), replace(&header.value)?));
    }

    // 5) 请求体
    let body = match &template.request.post_data {
        Some(post) => {
            let text = replace(&post.text)?;
            let mime = post.mime_type.trim();
            if !mime.is_empty()
                && !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".to_string(), mime.to_string()));
            }
            Some(text)
        }
        None => None,
    };

    let method = template.request.method.trim();
    let method = if method.is_empty() {
        "GET".to_string()
    } else {
        method.to_ascii_uppercase()
    };

    Ok(InjectedRequest {
        method,
        url: url.to_string(),
        headers,
        body,
    })
}

/// 把 `{{name}}` 替换成真实值；**未知占位符直接报错**（02 §7.1.3 §1）。
pub fn substitute(raw: &str, values: &BTreeMap<String, String>) -> Result<String, ReplayFailure> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("{{") {
        let (before, tail) = rest.split_at(start);
        out.push_str(before);
        let Some(end) = tail.find("}}") else {
            // 没有闭合的 "{{"：按原文保留（不是合法占位符，交给上游或用户看）
            out.push_str(tail);
            return Ok(out);
        };
        let name = &tail[2..end];
        match values.get(name) {
            Some(value) => out.push_str(value),
            None => {
                return Err(ReplayFailure::new(
                    ReplayClass::TemplateInvalid,
                    format!("模板里有未知占位符 {{{{{name}}}}}（变量表里没有这个变量）"),
                ))
            }
        }
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// 把 URL 拆成（不带 query 的部分, query 部分）。
pub fn split_query(url: &str) -> (String, Option<String>) {
    match url.find('?') {
        Some(pos) => {
            let (base, tail) = url.split_at(pos);
            let query = tail[1..].split('#').next().unwrap_or("").to_string();
            (
                base.to_string(),
                if query.is_empty() {
                    None
                } else {
                    Some(query)
                },
            )
        }
        None => (url.to_string(), None),
    }
}

/// 响应体片段（截断；**只留内存**）。
fn preview(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let slice = &body[..body.len().min(BODY_PREVIEW_LIMIT)];
    Some(String::from_utf8_lossy(slice).to_string())
}

/// 解析模板 JSON（失败 → `template_invalid`）。
pub fn parse_template(json: &str) -> Result<SiteTemplate, ReplayFailure> {
    serde_json::from_str::<SiteTemplate>(json).map_err(|err| {
        ReplayFailure::new(
            ReplayClass::TemplateInvalid,
            format!("模板 JSON 解析失败：{err}"),
        )
    })
}

/// 断言配置是否为空（没配任何断言）。
pub fn has_no_assertion(assert: &TemplateAssert) -> bool {
    assert.status.is_none() && assert.json_paths.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkin::har::{JsonPathAssert, TemplateVariable, VarTarget};

    /// 假解析器：按 id 返回固定值。
    struct FakeResolver {
        values: BTreeMap<String, String>,
    }

    impl FakeResolver {
        fn new(pairs: &[(&str, &str)]) -> Self {
            FakeResolver {
                values: pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }
        }
    }

    impl SecretResolver for FakeResolver {
        fn resolve(&self, credential_id: &str) -> Result<String, ResolveError> {
            self.values
                .get(credential_id)
                .cloned()
                .ok_or(ResolveError::NotFound)
        }
    }

    fn variable(name: &str, target: VarTarget, sensitive: bool, credential: Option<&str>) -> TemplateVariable {
        TemplateVariable {
            name: name.to_string(),
            target,
            sensitive,
            auto: false,
            credential_id: credential.map(|c| c.to_string()),
            default: if sensitive {
                None
            } else {
                Some("plain".to_string())
            },
        }
    }

    fn template_with_cookie() -> SiteTemplate {
        SiteTemplate {
            version: 1,
            source: "sample.har".to_string(),
            entry_count: 1,
            request: crate::checkin::har::TemplateRequest {
                method: "GET".to_string(),
                url: "https://example.invalid/api/v1/users/login/info".to_string(),
                headers: vec![crate::checkin::har::TemplateHeader {
                    name: "Cookie".to_string(),
                    value: "acw_tc={{acw_tc}}; csrf_session={{cookie}}".to_string(),
                }],
                query: Vec::new(),
                post_data: None,
            },
            variables: vec![
                variable(
                    "acw_tc",
                    VarTarget::HeaderPart {
                        field: "Cookie".to_string(),
                        part: "acw_tc".to_string(),
                    },
                    true,
                    Some("cred-1"),
                ),
                variable(
                    "cookie",
                    VarTarget::HeaderPart {
                        field: "Cookie".to_string(),
                        part: "csrf_session".to_string(),
                    },
                    true,
                    Some("cred-2"),
                ),
            ],
            assert: TemplateAssert {
                status: Some(200),
                json_paths: Vec::new(),
            },
        }
    }

    #[test]
    fn injects_cookie_parts_from_the_vault() {
        let template = template_with_cookie();
        let resolver = FakeResolver::new(&[("cred-1", "ACW-VALUE"), ("cred-2", "SESSION-VALUE")]);
        let request = inject(&template, &resolver).expect("注入应成功");
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://example.invalid/api/v1/users/login/info");
        assert_eq!(
            request.headers,
            vec![(
                "Cookie".to_string(),
                "acw_tc=ACW-VALUE; csrf_session=SESSION-VALUE".to_string()
            )]
        );
        assert!(request.body.is_none());
    }

    #[test]
    fn missing_credential_stops_before_any_request() {
        let template = template_with_cookie();
        let resolver = FakeResolver::new(&[("cred-1", "ACW-VALUE")]);
        let failure = inject(&template, &resolver).unwrap_err();
        assert_eq!(failure.class, ReplayClass::CredentialMissing);
        assert!(failure.message.contains("cred-2"));
        assert!(!failure.retryable);
    }

    #[test]
    fn unknown_placeholder_is_rejected() {
        let template = template_with_cookie();
        let resolver = FakeResolver::new(&[("cred-1", "ACW"), ("cred-2", "SESSION")]);
        let mut broken = template.clone();
        broken.request.headers[0].value = "acw_tc={{acw_tc}}; other={{not_in_table}}".to_string();
        let failure = inject(&broken, &resolver).unwrap_err();
        assert_eq!(failure.class, ReplayClass::TemplateInvalid);
        assert!(failure.message.contains("not_in_table"));
    }

    #[test]
    fn query_is_rebuilt_from_the_array_not_the_url() {
        let template = template_with_cookie();
        let mut with_query = template.clone();
        with_query.request.url = "https://example.invalid/api?token=old&page=1".to_string();
        with_query.request.query = vec![
            crate::checkin::har::TemplateParam {
                name: "token".to_string(),
                value: "{{tok}}".to_string(),
            },
            crate::checkin::har::TemplateParam {
                name: "page".to_string(),
                value: "1".to_string(),
            },
        ];
        with_query.variables.push(variable(
            "tok",
            VarTarget::Query {
                field: "token".to_string(),
            },
            true,
            Some("cred-3"),
        ));
        let resolver = FakeResolver::new(&[
            ("cred-1", "ACW"),
            ("cred-2", "SESSION"),
            ("cred-3", "TOKEN-VALUE"),
        ]);
        let request = inject(&with_query, &resolver).expect("注入应成功");
        assert_eq!(
            request.url,
            "https://example.invalid/api?token=TOKEN-VALUE&page=1",
            "URL 自带的 query 必须被剥掉再重建，不能发两遍"
        );
    }

    #[test]
    fn host_and_content_length_headers_are_dropped() {
        let template = template_with_cookie();
        let resolver = FakeResolver::new(&[("cred-1", "ACW"), ("cred-2", "SESSION")]);
        let mut with_host = template.clone();
        with_host.request.headers.push(crate::checkin::har::TemplateHeader {
            name: "Host".to_string(),
            value: "old.example.invalid".to_string(),
        });
        with_host.request.headers.push(crate::checkin::har::TemplateHeader {
            name: "Content-Length".to_string(),
            value: "999".to_string(),
        });
        let request = inject(&with_host, &resolver).expect("注入应成功");
        assert!(
            !request
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("host")),
            "Host 由客户端按 URL 生成"
        );
        assert!(
            !request
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        );
    }

    #[test]
    fn json_body_leaves_are_replaced() {
        let template = template_with_cookie();
        let mut with_body = template.clone();
        with_body.request.post_data = Some(crate::checkin::har::TemplatePostData {
            mime_type: "application/json".to_string(),
            text: "{\"csrf\":\"{{csrf}}\",\"keep\":\"yes\"}".to_string(),
        });
        with_body.variables.push(variable(
            "csrf",
            VarTarget::Body {
                field: "csrf".to_string(),
            },
            true,
            Some("cred-4"),
        ));
        let resolver = FakeResolver::new(&[
            ("cred-1", "ACW"),
            ("cred-2", "SESSION"),
            ("cred-4", "CSRF-VALUE"),
        ]);
        let request = inject(&with_body, &resolver).expect("注入应成功");
        let body = request.body.expect("应有请求体");
        assert!(body.contains("CSRF-VALUE"));
        assert!(body.contains("keep"));
        assert!(request
            .headers
            .iter()
            .any(|(name, value)| name == "Content-Type" && value == "application/json"));
    }

    #[test]
    fn assertion_passes_on_expected_status_and_json_paths() {
        let template = template_with_cookie();
        let body = br#"{"data":{"ok":true,"credits":400}}"#;
        let summary = check_assertion(&template, 200, body).expect("断言应通过");
        assert!(summary.contains("签到成功"));
    }

    #[test]
    fn assertion_rejects_wrong_status() {
        let template = template_with_cookie();
        let failure = check_assertion(&template, 401, b"{}").unwrap_err();
        assert_eq!(failure.class, ReplayClass::AssertFailed);
        assert!(failure.message.contains("期望 200"));
        assert!(failure.message.contains("实际 401"));
    }

    #[test]
    fn json_path_assertion_detects_missing_and_mismatched_values() {
        let template = template_with_cookie();
        let mut with_paths = template.clone();
        with_paths.assert.json_paths = vec![JsonPathAssert {
            path: "data.ok".to_string(),
            equals: Some(serde_json::json!(true)),
        }];
        let missing = check_assertion(&with_paths, 200, br#"{"data":{}}"#).unwrap_err();
        assert_eq!(missing.class, ReplayClass::AssertFailed);
        assert!(missing.message.contains("不存在"));

        let mismatched = check_assertion(&with_paths, 200, br#"{"data":{"ok":false}}"#).unwrap_err();
        assert!(mismatched.message.contains("期望 true"));

        let not_json = check_assertion(&with_paths, 200, b"not json").unwrap_err();
        assert!(not_json.message.contains("不是 JSON"));
    }

    #[test]
    fn json_path_supports_array_indexes() {
        let template = template_with_cookie();
        let mut with_paths = template.clone();
        with_paths.assert.json_paths = vec![JsonPathAssert {
            path: "data.items.0.id".to_string(),
            equals: Some(serde_json::json!(7)),
        }];
        assert!(check_assertion(&with_paths, 200, br#"{"data":{"items":[{"id":7}]}}"#).is_ok());
        assert!(check_assertion(&with_paths, 200, br#"{"data":{"items":[{"id":8}]}}"#).is_err());
    }

    #[test]
    fn no_assertion_falls_back_to_2xx_or_3xx() {
        let template = template_with_cookie();
        let mut bare = template.clone();
        bare.assert = TemplateAssert {
            status: None,
            json_paths: Vec::new(),
        };
        assert!(check_assertion(&bare, 204, b"").is_ok());
        let failure = check_assertion(&bare, 500, b"").unwrap_err();
        assert_eq!(failure.class, ReplayClass::HttpError);
    }

    #[test]
    fn upstream_status_drives_the_classification() {
        let template = template_with_cookie();
        let unauthorized = evaluate(&template, 401, b"{}");
        assert_eq!(unauthorized.class, ReplayClass::CredentialInvalid);
        assert!(!unauthorized.retryable);

        let limited = evaluate(&template, 429, b"{}");
        assert_eq!(limited.class, ReplayClass::RateLimited);
        assert!(limited.retryable, "429 可重试（02 §3.3）");

        let server_error = evaluate(&template, 503, b"{}");
        assert_eq!(server_error.class, ReplayClass::HttpError);
        assert!(server_error.retryable, "5xx 可重试（02 §3.3）");

        let bad_request = evaluate(&template, 400, b"{}");
        assert_eq!(bad_request.class, ReplayClass::HttpError);
        assert!(!bad_request.retryable, "4xx 不可重试（02 §3.3）");
    }

    #[test]
    fn failed_assertion_on_2xx_is_assert_failed() {
        let template = template_with_cookie();
        // 状态码对得上，但 JSON 路径断言不通过 → 断言失败（不是"成功"）
        let mut with_paths = template.clone();
        with_paths.assert.json_paths = vec![JsonPathAssert {
            path: "data.ok".to_string(),
            equals: Some(serde_json::json!(true)),
        }];
        let outcome = evaluate(&with_paths, 200, br#"{"data":{"ok":false}}"#);
        assert_eq!(outcome.class, ReplayClass::AssertFailed);
        assert!(!outcome.ok);
        assert!(outcome.status.is_some());
    }

    #[test]
    fn substitute_reports_unknown_placeholders() {
        let mut values = BTreeMap::new();
        values.insert("a".to_string(), "1".to_string());
        assert_eq!(
            substitute("x={{a}}y", &values).expect("应替换"),
            "x=1y".to_string()
        );
        let failure = substitute("{{b}}", &values).unwrap_err();
        assert_eq!(failure.class, ReplayClass::TemplateInvalid);
    }

    #[test]
    fn split_query_keeps_the_base_and_the_tail() {
        let (base, query) = split_query("https://x.invalid/a?b=1&c=2");
        assert_eq!(base, "https://x.invalid/a");
        assert_eq!(query.as_deref(), Some("b=1&c=2"));
        let (base, query) = split_query("https://x.invalid/a");
        assert_eq!(base, "https://x.invalid/a");
        assert!(query.is_none());
    }

    #[test]
    fn class_names_round_trip() {
        for class in [
            ReplayClass::Success,
            ReplayClass::AssertFailed,
            ReplayClass::NetworkError,
            ReplayClass::CredentialMissing,
            ReplayClass::TemplateInvalid,
            ReplayClass::CredentialInvalid,
            ReplayClass::RateLimited,
            ReplayClass::HttpError,
        ] {
            assert_eq!(
                ReplayClass::from_str_value(class.as_str()),
                Some(class),
                "{} 应能反读",
                class.as_str()
            );
        }
        assert!(ReplayClass::from_str_value("nope").is_none());
    }
}
