//! 声明式站点（T25；02 §7.6）。
//!
//! **spec → 现有 `SiteTemplate` → 现有重放引擎**：注入、占位符替换、断言、8 类分类、
//! 结果落库**全部沿用 T15–T18 已验证的实现，一行不改**。
//! 本模块只负责三件事：**校验**、**编译**、**积分/余额路径提取**。
//!
//! **为什么不是新引擎**：头头诉求「很多 agent harness 都可以有签到领积分」的真问题是
//! **新增一家平台的边际成本**——内置插件要写 Rust + 发版，HAR 模板要抓包 + 逐条标变量。
//! 声明式站点把边际成本降到「填一份 JSON」，**且不动已验证的核心**。
//!
//! **密文边界（AGENT.md §5.3）**：spec 里只有 `credentialId`（**id，不是密文**）；
//! `{{token}}` 的真实值**只在注入那一刻**从保险库取出来放进请求，**不进模板、不回显、不进日志**。

use serde::{Deserialize, Serialize};

use crate::checkin::har::{
    json_lookup, JsonPathAssert, SiteTemplate, TemplateAssert, TemplateHeader, TemplateParam,
    TemplatePostData, TemplateRequest, TemplateVariable, VarTarget,
};

/// 声明式站点的插件标识（`checkin_site.plugin`）。
pub const PLUGIN_SPEC: &str = "spec";

/// 敏感占位符名（真实值取自保险库）。
pub const TOKEN_VAR: &str = "token";

/// 非敏感占位符名（当天日期）。
pub const TODAY_VAR: &str = "today";

/// 声明式站点的默认 cron（与内置插件的默认值一致）。
pub const DEFAULT_SCHEDULE: &str = "0 9 * * *";

/// 积分单位缺省值。
pub const DEFAULT_UNIT: &str = "point";

/// spec 里的一个请求头。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecHeader {
    pub name: String,
    pub value: String,
}

/// spec 里的一个查询参数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecParam {
    pub name: String,
    pub value: String,
}

/// spec 里的请求体。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecBody {
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub text: String,
}

/// 一条 JSON 路径断言。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecJsonPath {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<serde_json::Value>,
}

/// 断言（状态码 + JSON 路径）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecAssert {
    /// **期望**状态码（不是录制值）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub json_paths: Vec<SpecJsonPath>,
}

/// 积分提取规则（点号路径）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecExtract {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// 余额查询规则（**当前余额**，与「本次拿到」是两个概念）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecBalance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub url: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// 余额请求的头；**留空则沿用签到请求的头**（省得重复填 Authorization）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<SpecHeader>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<SpecParam>,
}

/// 一个声明式站点。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecSite {
    /// 目录内唯一 id（`[a-z0-9_-]`，≤64 字符）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 请求方法（缺省 GET）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// 签到接口的绝对 URL（**必须 http/https**）。
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<SpecHeader>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<SpecParam>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<SpecBody>,
    /// `{{token}}` 的来源：保险库里的**凭据 id**（不是密文）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default)]
    pub assert: SpecAssert,
    /// 本次拿到多少（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reward: Option<SpecExtract>,
    /// 当前余额怎么查（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<SpecBalance>,
    /// 默认 cron（缺省 `0 9 * * *`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
}

impl SpecSite {
    /// 请求方法（缺省 GET，已归一为大写）。
    pub fn method(&self) -> String {
        match self.method.as_deref().map(str::trim) {
            Some(method) if !method.is_empty() => method.to_ascii_uppercase(),
            _ => "GET".to_string(),
        }
    }

    /// 积分单位（缺省 `point`）。
    pub fn reward_unit(&self) -> String {
        self.reward
            .as_ref()
            .and_then(|reward| reward.unit.clone())
            .filter(|unit| !unit.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_UNIT.to_string())
    }

    /// 余额单位（缺省 `point`）。
    pub fn balance_unit(&self) -> String {
        self.balance
            .as_ref()
            .and_then(|balance| balance.unit.clone())
            .filter(|unit| !unit.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_UNIT.to_string())
    }

    /// 校验（**导入与保存都跑**）；返回中文原因。
    pub fn validate(&self) -> Result<(), String> {
        let id = self.id.trim();
        if id.is_empty() {
            return Err("站点 id 不能为空".to_string());
        }
        if id.len() > 64 {
            return Err("站点 id 太长（≤64 字符）".to_string());
        }
        if !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err("站点 id 只能是小写字母、数字、下划线和连字符".to_string());
        }
        let name = self.name.trim();
        if name.is_empty() {
            return Err("站点名不能为空".to_string());
        }
        if name.chars().count() > 64 {
            return Err("站点名太长（≤64 字符）".to_string());
        }
        validate_http_url(&self.url, "签到 URL")?;
        if let Some(body) = &self.body {
            if body.text.trim().is_empty() && !body.mime_type.trim().is_empty() {
                return Err("请求体只有 MIME 类型没有内容".to_string());
            }
        }
        if let Some(status) = self.assert.status {
            if !(100..=599).contains(&status) {
                return Err(format!("断言状态码 {status} 不合法（100–599）"));
            }
        }
        for rule in &self.assert.json_paths {
            if rule.path.trim().is_empty() {
                return Err("JSON 路径断言的路径不能为空".to_string());
            }
        }
        if let Some(reward) = &self.reward {
            if reward.path.trim().is_empty() {
                return Err("积分字段路径不能为空".to_string());
            }
        }
        if let Some(balance) = &self.balance {
            validate_http_url(&balance.url, "余额 URL")?;
            if balance.path.trim().is_empty() {
                return Err("余额字段路径不能为空".to_string());
            }
        }
        // 占位符只认两种（02 §7.6.1）：`{{token}}` / `{{today}}`
        let used = self.placeholders();
        for name in &used {
            if name != TOKEN_VAR && name != TODAY_VAR {
                return Err(format!(
                    "不支持的占位符 {{{{{name}}}}}——只支持 {{{{token}}}} 与 {{{{today}}}}"
                ));
            }
        }
        Ok(())
    }

    /// 这个站点**要不要登录态**（用了 `{{token}}` 就要）。
    ///
    /// **不作为校验项**：从别的机器导进来的目录带着 `{{token}}` 但那边没有这个凭据——
    /// 站点照样建得出来，只是**跑的时候**判 `credential_missing`（**上游一个请求都不发**）。
    pub fn needs_credential(&self) -> bool {
        self.checkin_placeholders()
            .iter()
            .chain(self.balance_placeholders().iter())
            .any(|name| name == TOKEN_VAR)
    }

    /// 用到的占位符名（去重；签到 + 余额两处都算）。
    pub fn placeholders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for name in self.checkin_placeholders().into_iter().chain(self.balance_placeholders()) {
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// 签到请求里的占位符。
    fn checkin_placeholders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for text in std::iter::once(self.url.as_str())
            .chain(self.headers.iter().map(|header| header.value.as_str()))
            .chain(self.query.iter().map(|param| param.value.as_str()))
        {
            push_placeholders(text, &mut out);
        }
        if let Some(body) = &self.body {
            push_placeholders(&body.text, &mut out);
        }
        out
    }

    /// 余额请求里的占位符。
    ///
    /// **余额请求的头缺省沿用签到请求的头** → 扫描时也要算上那一份，
    /// 否则「签到头里有 `{{token}}`、余额头留空」时，余额模板会缺 `token` 变量
    /// → 注入阶段报「未知占位符」（真机 e2e 踩到过一次）。
    fn balance_placeholders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(balance) = &self.balance {
            push_placeholders(&balance.url, &mut out);
            let headers = if balance.headers.is_empty() {
                &self.headers
            } else {
                &balance.headers
            };
            for header in headers {
                push_placeholders(&header.value, &mut out);
            }
            for param in &balance.query {
                push_placeholders(&param.value, &mut out);
            }
        }
        out
    }

    /// 编译成**现有**站点模板（02 §7.6.2）——`today` 在**执行时**注入，跨天自动变。
    pub fn to_template(&self, today: &str) -> Result<SiteTemplate, String> {
        self.validate()?;
        let (base_url, url_query) = crate::checkin::replay::split_query(self.url.trim());
        let mut query: Vec<TemplateParam> = Vec::new();
        if let Some(raw) = url_query {
            for pair in raw.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (name, value) = match pair.split_once('=') {
                    Some((name, value)) => (name, value),
                    None => (pair, ""),
                };
                query.push(TemplateParam {
                    name: name.to_string(),
                    value: value.to_string(),
                });
            }
        }
        for param in &self.query {
            query.push(TemplateParam {
                name: param.name.clone(),
                value: param.value.clone(),
            });
        }

        let variables = self.variables(today);
        let request = TemplateRequest {
            method: self.method(),
            url: base_url.to_string(),
            headers: self
                .headers
                .iter()
                .map(|header| TemplateHeader {
                    name: header.name.clone(),
                    value: header.value.clone(),
                })
                .collect(),
            query,
            post_data: self.body.as_ref().map(|body| TemplatePostData {
                mime_type: body.mime_type.clone(),
                text: body.text.clone(),
            }),
        };
        let assert = TemplateAssert {
            status: self.assert.status,
            json_paths: self
                .assert
                .json_paths
                .iter()
                .map(|rule| JsonPathAssert {
                    path: rule.path.clone(),
                    equals: rule.equals.clone(),
                })
                .collect(),
        };
        Ok(SiteTemplate {
            version: 1,
            source: format!("spec:{}", self.id),
            entry_count: 1,
            request,
            variables,
            assert,
        })
    }

    /// 编译成**余额查询**模板（无断言 → 状态码 2xx/3xx 即成功）。
    pub fn to_balance_template(&self, today: &str) -> Result<SiteTemplate, String> {
        let balance = self
            .balance
            .as_ref()
            .ok_or_else(|| "这个站点没配余额查询".to_string())?;
        validate_http_url(&balance.url, "余额 URL")?;
        let (base_url, url_query) = crate::checkin::replay::split_query(balance.url.trim());
        let mut query: Vec<TemplateParam> = Vec::new();
        if let Some(raw) = url_query {
            for pair in raw.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (name, value) = match pair.split_once('=') {
                    Some((name, value)) => (name, value),
                    None => (pair, ""),
                };
                query.push(TemplateParam {
                    name: name.to_string(),
                    value: value.to_string(),
                });
            }
        }
        for param in &balance.query {
            query.push(TemplateParam {
                name: param.name.clone(),
                value: param.value.clone(),
            });
        }
        // 余额请求的头**缺省沿用签到请求的头**（省得重复填 Authorization）
        let headers = if balance.headers.is_empty() {
            self.headers.clone()
        } else {
            balance.headers.clone()
        };

        Ok(SiteTemplate {
            version: 1,
            source: format!("spec-balance:{}", self.id),
            entry_count: 1,
            request: TemplateRequest {
                method: balance
                    .method
                    .as_deref()
                    .map(str::trim)
                    .filter(|method| !method.is_empty())
                    .map(|method| method.to_ascii_uppercase())
                    .unwrap_or_else(|| "GET".to_string()),
                url: base_url.to_string(),
                headers: headers
                    .into_iter()
                    .map(|header| TemplateHeader {
                        name: header.name,
                        value: header.value,
                    })
                    .collect(),
                query,
                post_data: None,
            },
            variables: self.balance_variables(today),
            assert: TemplateAssert {
                status: None,
                json_paths: Vec::new(),
            },
        })
    }

    /// 签到请求的变量表（`token` 敏感 → 保险库；`today` 非敏感 → 当天日期）。
    fn variables(&self, today: &str) -> Vec<TemplateVariable> {
        let uses_token = self
            .checkin_placeholders()
            .iter()
            .any(|name| name == TOKEN_VAR);
        self.variables_with(today, uses_token)
    }

    /// 余额请求的变量表。
    fn balance_variables(&self, today: &str) -> Vec<TemplateVariable> {
        let uses_token = self
            .balance_placeholders()
            .iter()
            .any(|name| name == TOKEN_VAR);
        self.variables_with(today, uses_token)
    }

    /// 变量表（**没有凭据也照样放 `token` 变量**）。
    ///
    /// 那样注入阶段会判 **`credential_missing`**（**上游一个请求都不发**），
    /// 而不是变成「未知占位符」的模板错——**两者的排查方向完全不同**。
    fn variables_with(&self, today: &str, uses_token: bool) -> Vec<TemplateVariable> {
        let mut variables = Vec::new();
        if uses_token {
            variables.push(TemplateVariable {
                name: TOKEN_VAR.to_string(),
                // **仅元信息**：注入按名字取值，这个 target 不参与替换（02 §7.6.2）。
                target: VarTarget::Header {
                    field: "Authorization".to_string(),
                },
                sensitive: true,
                auto: false,
                credential_id: self.credential_id.clone().filter(|id| !id.is_empty()),
                default: None,
            });
        }
        variables.push(TemplateVariable {
            name: TODAY_VAR.to_string(),
            target: VarTarget::Query {
                field: TODAY_VAR.to_string(),
            },
            sensitive: false,
            auto: false,
            credential_id: None,
            default: Some(today.to_string()),
        });
        variables
    }
}

/// 把一段文本里的占位符名并进 `out`（去重）。
fn push_placeholders(text: &str, out: &mut Vec<String>) {
    for name in placeholders_in(text) {
        if !out.contains(&name) {
            out.push(name);
        }
    }
}

/// 从一段文本里抽出 `{{name}}` 里的 name。
pub fn placeholders_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let (_, tail) = rest.split_at(start);
        match tail[2..].find("}}") {
            Some(end) => {
                let name = tail[2..2 + end].trim().to_string();
                if !name.is_empty() {
                    out.push(name);
                }
                rest = &tail[2 + end + 2..];
            }
            None => break,
        }
    }
    out
}

/// 从响应体里按点号路径取积分数值。
///
/// 接受数字与「数字形态的字符串」（有的平台回 `"400"`）。
pub fn extract_amount(path: &str, body: &serde_json::Value) -> Option<i64> {
    let value = json_lookup(body, path)?;
    match value {
        serde_json::Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f.round() as i64)),
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 校验一个 http/https 绝对 URL。
fn validate_http_url(url: &str, label: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} 不能为空"));
    }
    let parsed = match reqwest::Url::parse(trimmed) {
        Ok(parsed) => parsed,
        Err(err) => return Err(format!("{label} 不合法：{err}")),
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("{label} 必须是 http 或 https（当前是 {}）", parsed.scheme()));
    }
    if parsed.host_str().unwrap_or("").is_empty() {
        return Err(format!("{label} 缺少主机名"));
    }
    Ok(())
}

/// 声明式站点存在 `checkin_site.config_json` 里的形状。
///
/// **复用 `PluginSiteConfig` 的字段名**（`credentialId` / `channelId` / `accountId`），
/// 于是 `checkin::plugin::parse_config` 不用改就能读出归属与登录态；
/// 多出来的 `spec` 字段被 serde 忽略（不影响老代码）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpecSiteConfig {
    /// `{{token}}` 的来源（保险库凭据 id；**不是密文**）。
    pub credential_id: Option<String>,
    /// 归属渠道 id（凭据必须挂在渠道上）。
    pub channel_id: Option<String>,
    /// 归属账号 id（积分回写落在它的 `quota_json`）。
    pub account_id: Option<String>,
    /// 站点定义本体。
    pub spec: SpecSite,
}

impl SpecSiteConfig {
    /// 序列化成 `config_json` 的文本。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// **执行用的 spec**：把归属配置里的 `credentialId` 灌进 spec。
    ///
    /// **站点自己的登录态优先**——spec 里那个可能是从别的机器的目录带来的（那边没有这个凭据）。
    pub fn effective_spec(&self) -> SpecSite {
        let mut spec = self.spec.clone();
        if let Some(credential_id) = self.credential_id.as_ref().filter(|id| !id.is_empty()) {
            spec.credential_id = Some(credential_id.clone());
        }
        spec
    }
}

/// 从站点行里读出声明式配置（**解析失败要给原因，不静默吞掉**）。
pub fn parse_site_config(site: &crate::store::checkin::SiteRow) -> Result<SpecSiteConfig, String> {
    let json = site
        .config_json
        .as_deref()
        .ok_or_else(|| "站点没有定义（config_json 为空）".to_string())?;
    let config: SpecSiteConfig = serde_json::from_str(json)
        .map_err(|err| format!("站点的声明式定义读不出来：{err}"))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> SpecSite {
        SpecSite {
            id: "my-platform".to_string(),
            name: "我的平台".to_string(),
            method: Some("post".to_string()),
            url: "https://example.com/api/checkin?from=harness".to_string(),
            headers: vec![SpecHeader {
                name: "Authorization".to_string(),
                value: "Bearer {{token}}".to_string(),
            }],
            query: vec![SpecParam {
                name: "day".to_string(),
                value: "{{today}}".to_string(),
            }],
            body: None,
            credential_id: Some("cred_1".to_string()),
            assert: SpecAssert {
                status: Some(200),
                json_paths: vec![SpecJsonPath {
                    path: "data.ok".to_string(),
                    equals: Some(json!(true)),
                }],
            },
            reward: Some(SpecExtract {
                path: "data.credits".to_string(),
                unit: Some("magicube".to_string()),
            }),
            balance: Some(SpecBalance {
                method: None,
                url: "https://example.com/api/balance".to_string(),
                path: "data.balance".to_string(),
                unit: None,
                headers: Vec::new(),
                query: Vec::new(),
            }),
            schedule: None,
        }
    }

    #[test]
    fn a_valid_spec_passes_validation() {
        assert!(sample().validate().is_ok());
    }

    #[test]
    fn non_http_urls_are_rejected() {
        let mut spec = sample();
        spec.url = "file:///etc/passwd".to_string();
        let err = spec.validate().unwrap_err();
        assert!(err.contains("http"), "{err}");

        spec.url = "ftp://example.com/x".to_string();
        assert!(spec.validate().unwrap_err().contains("http"));
    }

    #[test]
    fn bad_ids_are_rejected() {
        let mut spec = sample();
        spec.id = "Bad Id!".to_string();
        assert!(spec.validate().unwrap_err().contains("id"));
        spec.id = String::new();
        assert!(spec.validate().unwrap_err().contains("id"));
    }

    #[test]
    fn unknown_placeholders_are_rejected() {
        let mut spec = sample();
        spec.headers[0].value = "Bearer {{session}}".to_string();
        let err = spec.validate().unwrap_err();
        assert!(err.contains("占位符"), "{err}");
    }

    #[test]
    fn token_placeholder_without_credential_is_allowed_but_flagged() {
        // **不因「还没填登录态」就拒建**——从别的机器导进来的目录带着 `{{token}}`，
        // 那边没有这个凭据；站点照样建得出来，跑的时候才判 `credential_missing`。
        let mut spec = sample();
        spec.credential_id = None;
        assert!(spec.validate().is_ok());
        assert!(spec.needs_credential());
    }

    #[test]
    fn compiles_into_the_existing_template_shape() {
        let spec = sample();
        let template = spec.to_template("2026-11-28").unwrap();
        assert_eq!(template.request.method, "POST");
        // URL 自带的 query 被摘出来并进 query 数组（不重复发）
        assert_eq!(template.request.url, "https://example.com/api/checkin");
        let names: Vec<&str> = template
            .request
            .query
            .iter()
            .map(|param| param.name.as_str())
            .collect();
        assert_eq!(names, vec!["from", "day"]);
        assert_eq!(template.assert.status, Some(200));
        assert_eq!(template.assert.json_paths.len(), 1);

        // 变量：token 敏感（走保险库）、today 非敏感（默认值 = 当天）
        let token = template
            .variables
            .iter()
            .find(|variable| variable.name == TOKEN_VAR)
            .expect("应有 token 变量");
        assert!(token.sensitive);
        assert_eq!(token.credential_id.as_deref(), Some("cred_1"));
        assert!(token.default.is_none());

        let today = template
            .variables
            .iter()
            .find(|variable| variable.name == TODAY_VAR)
            .expect("应有 today 变量");
        assert!(!today.sensitive);
        assert_eq!(today.default.as_deref(), Some("2026-11-28"));
    }

    #[test]
    fn balance_template_reuses_the_checkin_headers() {
        let spec = sample();
        let template = spec.to_balance_template("2026-11-28").unwrap();
        assert_eq!(template.request.method, "GET");
        assert_eq!(template.request.url, "https://example.com/api/balance");
        assert_eq!(template.request.headers.len(), 1);
        assert_eq!(template.request.headers[0].name, "Authorization");
        // 余额请求**不配断言** → 2xx/3xx 即成功
        assert!(template.assert.status.is_none());
        assert!(template.assert.json_paths.is_empty());
    }

    #[test]
    fn amounts_are_extracted_from_numbers_and_numeric_strings() {
        let body = json!({"data": {"credits": 400, "balance": "1200"}});
        assert_eq!(extract_amount("data.credits", &body), Some(400));
        assert_eq!(extract_amount("data.balance", &body), Some(1200));
        assert_eq!(extract_amount("data.missing", &body), None);
        assert_eq!(
            extract_amount("data.credits", &json!({"data": {"credits": "many"}})),
            None
        );
    }

    #[test]
    fn placeholders_are_scanned_out_of_every_field() {
        let spec = sample();
        let names = spec.placeholders();
        assert!(names.contains(&TOKEN_VAR.to_string()));
        assert!(names.contains(&TODAY_VAR.to_string()));
        assert_eq!(placeholders_in("a {{x}} b {{ y }}"), vec!["x", "y"]);
    }
}
