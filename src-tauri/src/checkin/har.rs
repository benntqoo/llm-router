//! HAR 1.2 解析、变量候选识别与模板生成（T15；02 §7.1.1 / §7.1.2）。
//!
//! **自写解析**（02 §1.3「HAR 解析：自写（HAR 1.2 JSON 子集）」）：
//! 只取 `log.entries[].request` 的 method/url/headers/queryString/postData
//! 与 `log.entries[].response.status`；**响应体与响应头一律丢弃**
//! （实测 ModelScope 签到响应的 `Set-Cookie` 带 `csrf_session` / `csrf_token`，属密文）。
//!
//! **密文边界（AGENT.md §5.3）**：
//! - 本模块对外（IPC）只给**掩码值**（`***`），敏感原值不出本模块；
//! - 真实值只在生成模板时取出，交给保险库（`credential` 表密文）；
//! - 模板落库前自检：**模板里不得残留被标记字段的原值**，否则拒绝保存。

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// 敏感值的掩码（对外只出现这个）。
pub const MASK: &str = "***";

/// 模板格式版本（将来改格式时递增）。
pub const TEMPLATE_VERSION: u32 = 1;

/// 变量名允许的字符集（占位符 `{{name}}`）。
pub const VAR_NAME_MAX_LEN: usize = 32;

/// 自动识别为敏感的名字片段（**子串匹配**，大小写不敏感；命中即建议标记）。
pub const SENSITIVE_NAME_PATTERNS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "x-csrf-token",
    "csrf-token",
    "csrf",
    "token",
    "session",
    "jwt",
    "acw_tc",
    "x-xsrf-token",
];

/// HAR 处理错误。
#[derive(Debug, thiserror::Error)]
pub enum HarError {
    #[error("读取 HAR 文件失败: {0}")]
    Io(String),

    #[error("HAR 不是合法 JSON: {0}")]
    Json(String),

    #[error("HAR 结构不符合 1.2 约定: {0}")]
    Structure(String),

    #[error("标记的字段在 HAR 里找不到: {0}")]
    TargetNotFound(String),

    #[error("模板里残留了敏感原值，已拒绝保存（防密文泄漏）")]
    SecretLeak,
}

/// HAR 处理结果。
pub type Result<T> = std::result::Result<T, HarError>;

// ────────────────────────────── 线格式（HAR 1.2 子集） ──────────────────────────────

#[derive(Debug, Deserialize)]
pub struct HarFile {
    pub log: HarLog,
}

#[derive(Debug, Deserialize)]
pub struct HarLog {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub entries: Vec<HarEntry>,
}

#[derive(Debug, Deserialize)]
pub struct HarEntry {
    #[serde(default)]
    pub request: HarRequest,
    #[serde(default)]
    pub response: HarResponse,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HarRequest {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: Vec<HarKv>,
    #[serde(default)]
    pub query_string: Vec<HarKv>,
    #[serde(default)]
    pub post_data: Option<HarPostData>,
}

#[derive(Debug, Deserialize, Default)]
pub struct HarResponse {
    #[serde(default)]
    pub status: i64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct HarKv {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HarPostData {
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub encoding: String,
}

// ────────────────────────────── 对外模型 ──────────────────────────────

/// 变量的位置（替换发生在"值内部"；02 §7.1.1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VarTarget {
    /// 整个请求头（如 `Authorization`）。
    Header { field: String },
    /// `Cookie` 头里的某个子项（`name=value` 的 `value` 部分）。
    HeaderPart { field: String, part: String },
    /// 查询参数。
    Query { field: String },
    /// JSON 请求体的点路径（如 `data.csrf`）。
    Body { field: String },
}

impl VarTarget {
    /// 展示用的位置说明。
    pub fn label(&self) -> String {
        match self {
            VarTarget::Header { field } => format!("请求头 {field}"),
            VarTarget::HeaderPart { field, part } => format!("请求头 {field} · {part}"),
            VarTarget::Query { field } => format!("查询参数 {field}"),
            VarTarget::Body { field } => format!("请求体 {field}"),
        }
    }
}

/// 一个可标记的字段（展示给 UI；敏感值已掩码）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldView {
    /// 变量位置。
    pub target: VarTarget,
    /// 值（敏感项为 `***`）。
    pub value: String,
    /// 是否敏感（敏感值的真实值必须进保险库，不进模板）。
    pub sensitive: bool,
    /// 是否被自动识别建议标记。
    pub suggested: bool,
}

/// 一条 HAR entry 的展示视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    pub index: usize,
    pub method: String,
    pub url: String,
    pub status: i64,
    pub fields: Vec<FieldView>,
}

/// 一次导入的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarImport {
    /// HAR 文件名（只留文件名，不落绝对路径）。
    pub source: String,
    pub entry_count: usize,
    pub entries: Vec<EntryView>,
}

/// 某个 target 是否按**敏感**处理。
///
/// - `Cookie` 头的子项：**一律敏感**（cookie = 会话凭据，不靠名字猜）；
/// - 其它字段：按名字关键词判定（csrf / token / session / jwt / authorization …）。
pub fn target_is_sensitive(target: &VarTarget) -> bool {
    match target {
        VarTarget::HeaderPart { .. } => true,
        _ => is_sensitive_name(&target_raw_name(target)),
    }
}

/// 变量取值在保险库里的凭证 kind（01 §4.4 的 kind 取值）。
///
/// Cookie 子项 → `cookie`；其它敏感头 / 查询参数 → `access_token`。
pub fn credential_kind(target: &VarTarget) -> &'static str {
    match target {
        VarTarget::HeaderPart { .. } => "cookie",
        _ => "access_token",
    }
}

/// 模板里的一个请求头。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateHeader {
    pub name: String,
    pub value: String,
}

/// 模板里的一个查询参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateParam {
    pub name: String,
    pub value: String,
}

/// 模板里的请求体。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePostData {
    pub mime_type: String,
    pub text: String,
}

/// 模板里的请求（标记过的值已换成 `{{name}}`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<TemplateHeader>,
    pub query: Vec<TemplateParam>,
    pub post_data: Option<TemplatePostData>,
}

/// 模板里的一个变量。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateVariable {
    pub name: String,
    pub target: VarTarget,
    /// 是否敏感（敏感值的真实值在保险库里，不进模板）。
    pub sensitive: bool,
    /// 是否由系统**自动补齐**（用户没标的敏感字段；名字带 `auto_` 前缀）。
    pub auto: bool,
    /// 敏感变量的真实值在保险库里的凭证 id（非敏感变量为 `None`）。
    pub credential_id: Option<String>,
    /// 非敏感变量的默认值（**敏感变量恒为 `None`**——真实值不进模板）。
    pub default: Option<String>,
}

/// 模板的断言（状态码；JSON 路径断言属 T16）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateAssert {
    pub status: i64,
}

/// 站点模板（落 `checkin_site.config_json`；02 §7.1.1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteTemplate {
    pub version: u32,
    /// 来源 HAR 文件名。
    pub source: String,
    /// 导入时 HAR 的 entry 总数（便于看出模板是从多大的抓包里挑出来的）。
    pub entry_count: usize,
    pub request: TemplateRequest,
    pub variables: Vec<TemplateVariable>,
    pub assert: TemplateAssert,
}

/// 用户标记好的一个变量（IPC 入参）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkedVariable {
    pub name: String,
    pub target: VarTarget,
}

// ────────────────────────────── 解析 ──────────────────────────────

/// 解析 HAR 文件字节。
pub fn parse_har(bytes: &[u8]) -> Result<HarFile> {
    let file: HarFile =
        serde_json::from_slice(bytes).map_err(|e| HarError::Json(e.to_string()))?;
    if file.log.version.is_empty() {
        return Err(HarError::Structure("log.version 缺失".to_string()));
    }
    Ok(file)
}

/// 读取并解析一个 HAR 文件（路径由用户在 UI 里选）。
pub fn import_from_path(path: &str) -> Result<HarImport> {
    let bytes = std::fs::read(path).map_err(|e| HarError::Io(e.to_string()))?;
    let file = parse_har(&bytes)?;
    Ok(to_import(&file, file_name(path)))
}

/// 取文件名（模板里只留文件名，不落绝对路径）。
pub fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown.har".to_string())
}

/// 把解析结果转成展示视图（**敏感值掩码**）。
pub fn to_import(file: &HarFile, source: String) -> HarImport {
    let entry_count = file.log.entries.len();
    let entries = file
        .log
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| EntryView {
            index,
            method: entry.request.method.clone(),
            url: entry.request.url.clone(),
            status: entry.response.status,
            fields: fields_of(entry),
        })
        .collect();
    HarImport {
        source,
        entry_count,
        entries,
    }
}

/// 列出某条 entry 的全部可标记字段。
pub fn fields_of(entry: &HarEntry) -> Vec<FieldView> {
    let mut fields = Vec::new();

    for header in &entry.request.headers {
        let lname = header.name.to_ascii_lowercase();
        if lname == "cookie" {
            // Cookie 头按子项拆：每个 `name=value` 都是独立候选。
            // **Cookie 里的东西一律按密文处理**（不靠名字猜）——cookie 本来就是会话，
            // 留在模板里等于把会话凭证明文写进 SQLite（AGENT.md §5.3）。
            for (key, value) in parse_cookie_pairs(&header.value) {
                fields.push(FieldView {
                    target: VarTarget::HeaderPart {
                        field: header.name.clone(),
                        part: key.clone(),
                    },
                    value: MASK.to_string(),
                    sensitive: true,
                    suggested: true,
                });
                let _ = value;
            }
        } else {
            let sensitive = is_sensitive_name(&header.name);
            fields.push(FieldView {
                target: VarTarget::Header {
                    field: header.name.clone(),
                },
                value: if sensitive {
                    MASK.to_string()
                } else {
                    header.value.clone()
                },
                sensitive,
                suggested: sensitive,
            });
        }
    }

    for param in &entry.request.query_string {
        let sensitive = is_sensitive_name(&param.name);
        fields.push(FieldView {
            target: VarTarget::Query {
                field: param.name.clone(),
            },
            value: if sensitive {
                MASK.to_string()
            } else {
                param.value.clone()
            },
            sensitive,
            suggested: sensitive,
        });
    }

    for (path, value) in body_fields(entry) {
        let sensitive = is_sensitive_name(&path);
        fields.push(FieldView {
            target: VarTarget::Body { field: path.clone() },
            value: if sensitive {
                MASK.to_string()
            } else {
                value
            },
            sensitive,
            suggested: sensitive,
        });
    }

    fields
}

/// 名字是否敏感（子串匹配，大小写不敏感）。
pub fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_NAME_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

/// 拆 `Cookie` 头的 `name=value` 对（按**第一个** `=` 切分，值里可含 `=`）。
pub fn parse_cookie_pairs(value: &str) -> Vec<(String, String)> {
    value
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| match part.find('=') {
            Some(pos) => (part[..pos].trim().to_string(), part[pos + 1..].trim().to_string()),
            None => (part.to_string(), String::new()),
        })
        .collect()
}

/// 取 JSON 请求体里的字符串叶子（点路径）。
fn body_fields(entry: &HarEntry) -> Vec<(String, String)> {
    let Some(post) = &entry.request.post_data else {
        return Vec::new();
    };
    let text = if post.encoding.eq_ignore_ascii_case("base64") {
        match BASE64.decode(post.text.as_bytes()) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => return Vec::new(),
            },
            Err(_) => return Vec::new(),
        }
    } else {
        post.text.clone()
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_leaves(&value, &mut String::new(), &mut out);
    out
}

fn collect_leaves(value: &serde_json::Value, path: &mut String, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let saved = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                collect_leaves(child, path, out);
                path.truncate(saved);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let saved = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(&index.to_string());
                collect_leaves(child, path, out);
                path.truncate(saved);
            }
        }
        serde_json::Value::String(text) => {
            out.push((path.clone(), text.clone()));
        }
        _ => {}
    }
}

/// 按点路径取 JSON 里的字符串叶子。
fn body_value(value: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = value;
    for segment in path.split('.') {
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            serde_json::Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    match current {
        serde_json::Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// 按点路径替换 JSON 里的字符串叶子。
fn body_set(value: &mut serde_json::Value, path: &str, new_value: &str) -> bool {
    let segments: Vec<&str> = path.split('.').collect();
    fn walk(node: &mut serde_json::Value, segments: &[&str], new_value: &str) -> bool {
        let Some((head, rest)) = segments.split_first() else {
            return false;
        };
        match node {
            serde_json::Value::Object(map) => {
                let Some(child) = map.get_mut(*head) else {
                    return false;
                };
                if rest.is_empty() {
                    if let serde_json::Value::String(slot) = child {
                        *slot = new_value.to_string();
                        return true;
                    }
                    return false;
                }
                walk(child, rest, new_value)
            }
            serde_json::Value::Array(items) => {
                let Ok(index) = head.parse::<usize>() else {
                    return false;
                };
                let Some(child) = items.get_mut(index) else {
                    return false;
                };
                if rest.is_empty() {
                    if let serde_json::Value::String(slot) = child {
                        *slot = new_value.to_string();
                        return true;
                    }
                    return false;
                }
                walk(child, rest, new_value)
            }
            _ => false,
        }
    }
    walk(value, &segments, new_value)
}

// ────────────────────────────── 模板生成 ──────────────────────────────

/// 校验变量名（只允许 `[A-Za-z0-9_-]`，非空，长度有上限）。
pub fn validate_var_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(HarError::TargetNotFound("变量名不能为空".to_string()));
    }
    if name.len() > VAR_NAME_MAX_LEN {
        return Err(HarError::TargetNotFound(format!(
            "变量名过长（上限 {VAR_NAME_MAX_LEN} 字符）"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(HarError::TargetNotFound(format!(
            "变量名只能含字母、数字、下划线与连字符: {name}"
        )));
    }
    Ok(())
}

/// 占位符文本。
pub fn placeholder(name: &str) -> String {
    format!("{{{{{name}}}}}")
}

/// target 的原始字段名（判断敏感、生成自动变量名都用它）。
pub fn target_raw_name(target: &VarTarget) -> String {
    match target {
        VarTarget::Header { field } => field.clone(),
        VarTarget::HeaderPart { part, .. } => part.clone(),
        VarTarget::Query { field } => field.clone(),
        VarTarget::Body { field } => field.clone(),
    }
}

/// 某个 target 是否已被某个标记覆盖（含"整个头被标记 → 其子项也算覆盖"）。
fn is_covered(target: &VarTarget, mark: &VarTarget) -> bool {
    match (target, mark) {
        (VarTarget::Header { field }, VarTarget::Header { field: other }) => {
            field.eq_ignore_ascii_case(other)
        }
        (
            VarTarget::HeaderPart { field, part },
            VarTarget::HeaderPart {
                field: other_field,
                part: other_part,
            },
        ) => field.eq_ignore_ascii_case(other_field) && part.eq_ignore_ascii_case(other_part),
        (VarTarget::HeaderPart { field, .. }, VarTarget::Header { field: other }) => {
            field.eq_ignore_ascii_case(other)
        }
        (VarTarget::Query { field }, VarTarget::Query { field: other }) => {
            field.eq_ignore_ascii_case(other)
        }
        (VarTarget::Body { field }, VarTarget::Body { field: other }) => {
            field.eq_ignore_ascii_case(other)
        }
        _ => false,
    }
}

/// 自动变量名：`auto_<字段名>`，冲突时加序号。
fn auto_name(raw: &str, used: &BTreeMap<String, ()>) -> String {
    let mut base = String::from("auto_");
    let mut prev_underscore = false;
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            base.push(lower);
            prev_underscore = false;
        } else if !prev_underscore {
            base.push('_');
            prev_underscore = true;
        }
    }
    while base.ends_with('_') {
        base.pop();
    }
    // 给序号后缀留位置
    if base.len() > VAR_NAME_MAX_LEN.saturating_sub(6) {
        base.truncate(VAR_NAME_MAX_LEN.saturating_sub(6));
        while base.ends_with('_') {
            base.pop();
        }
    }
    let mut candidate = base.clone();
    let mut counter = 2usize;
    while used.contains_key(&candidate) {
        candidate = format!("{base}_{counter}");
        counter += 1;
    }
    candidate
}

/// 取某个 target 在 entry 里的**原始值**（用于写保险库）。
pub fn extract_value(entry: &HarEntry, target: &VarTarget) -> Option<String> {
    match target {
        VarTarget::Header { field } => entry
            .request
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(field))
            .map(|h| h.value.clone()),
        VarTarget::HeaderPart { field, part } => {
            let header = entry
                .request
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(field))?;
            parse_cookie_pairs(&header.value)
                .into_iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(part))
                .map(|(_, value)| value)
        }
        VarTarget::Query { field } => entry
            .request
            .query_string
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(field))
            .map(|p| p.value.clone()),
        VarTarget::Body { field } => {
            let post = entry.request.post_data.as_ref()?;
            let text = if post.encoding.eq_ignore_ascii_case("base64") {
                let bytes = BASE64.decode(post.text.as_bytes()).ok()?;
                String::from_utf8(bytes).ok()?
            } else {
                post.text.clone()
            };
            let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
            body_value(&value, field)
        }
    }
}

/// 生成模板（**未写保险库**：敏感值的 `credential_id` 留空，由调用方回填）。
///
/// 返回 `(模板, [(变量, 原始值)])`——原始值交给调用方写保险库，不进模板。
pub fn build_template(
    file: &HarFile,
    source: &str,
    entry_index: usize,
    marked: &[MarkedVariable],
) -> Result<(SiteTemplate, Vec<(TemplateVariable, String)>)> {
    let entry = file.log.entries.get(entry_index).ok_or_else(|| {
        HarError::TargetNotFound(format!("HAR 里没有第 {entry_index} 条 entry"))
    })?;

    // 变量名唯一性
    let mut used_names: BTreeMap<String, ()> = BTreeMap::new();
    for variable in marked {
        validate_var_name(&variable.name)?;
        if used_names.insert(variable.name.clone(), ()).is_some() {
            return Err(HarError::TargetNotFound(format!(
                "变量名重复: {}",
                variable.name
            )));
        }
    }

    // **自动补齐未标记的敏感字段**（02 §7.1.2：模板里不得出现任何敏感原值）。
    // 用户没标的敏感字段不能留在模板里（会泄漏），也不能丢掉（重放会失效）——
    // 于是自动变成一个变量，名字带 `auto_` 前缀，用户可在列表里看到并改名。
    let mut effective: Vec<(MarkedVariable, bool)> = marked
        .iter()
        .map(|variable| (variable.clone(), false))
        .collect();
    for field in fields_of(entry) {
        if !field.sensitive {
            continue;
        }
        if effective
            .iter()
            .any(|(mark, _)| is_covered(&field.target, &mark.target))
        {
            continue;
        }
        let name = auto_name(&target_raw_name(&field.target), &used_names);
        used_names.insert(name.clone(), ());
        effective.push((
            MarkedVariable {
                name,
                target: field.target.clone(),
            },
            true,
        ));
    }

    // 取出每个变量的原始值（敏感值的原值**只在这里出现一次**）
    let mut originals: Vec<(TemplateVariable, String)> = Vec::new();
    for (variable, auto) in &effective {
        let value = extract_value(entry, &variable.target).ok_or_else(|| {
            HarError::TargetNotFound(format!(
                "{} 在 HAR 里找不到",
                variable.target.label()
            ))
        })?;
        let sensitive = target_is_sensitive(&variable.target);
        originals.push((
            TemplateVariable {
                name: variable.name.clone(),
                target: variable.target.clone(),
                sensitive,
                auto: *auto,
                credential_id: None,
                default: if sensitive {
                    None
                } else {
                    Some(value.clone())
                },
            },
            value,
        ));
    }

    // 组装请求（把标记过的值换成占位符）
    let mut headers = Vec::new();
    for header in &entry.request.headers {
        let mut value = header.value.clone();
        // 整个头被标记
        for (variable, _) in &originals {
            if let VarTarget::Header { field } = &variable.target {
                if field.eq_ignore_ascii_case(&header.name) {
                    value = placeholder(&variable.name);
                }
            }
        }
        // Cookie 头里的子项被标记
        let mut cookie_marks: Vec<(&str, &str)> = Vec::new();
        for (variable, _) in &originals {
            if let VarTarget::HeaderPart { field, part } = &variable.target {
                if field.eq_ignore_ascii_case(&header.name) {
                    cookie_marks.push((part.as_str(), variable.name.as_str()));
                }
            }
        }
        if !cookie_marks.is_empty() {
            let pairs = parse_cookie_pairs(&header.value);
            let rebuilt = pairs
                .into_iter()
                .map(|(key, val)| {
                    let marked = cookie_marks
                        .iter()
                        .find(|(part, _)| part.eq_ignore_ascii_case(&key))
                        .map(|(_, name)| placeholder(name));
                    (key, marked.unwrap_or(val))
                })
                .map(|(key, val)| format!("{key}={val}"))
                .collect::<Vec<_>>()
                .join("; ");
            value = rebuilt;
        }
        headers.push(TemplateHeader {
            name: header.name.clone(),
            value,
        });
    }

    let mut query = Vec::new();
    for param in &entry.request.query_string {
        let mut value = param.value.clone();
        for (variable, _) in &originals {
            if let VarTarget::Query { field } = &variable.target {
                if field.eq_ignore_ascii_case(&param.name) {
                    value = placeholder(&variable.name);
                }
            }
        }
        query.push(TemplateParam {
            name: param.name.clone(),
            value,
        });
    }

    let post_data = entry.request.post_data.as_ref().map(|post| {
        // body 若是 base64 编码则原样保留（不是 JSON，改不了）
        let mut text = post.text.clone();
        let body_marks: Vec<(&str, &str)> = originals
            .iter()
            .filter_map(|(variable, _)| match &variable.target {
                VarTarget::Body { field } => Some((field.as_str(), variable.name.as_str())),
                _ => None,
            })
            .collect();
        if !body_marks.is_empty() {
            if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) {
                for (path, name) in &body_marks {
                    body_set(&mut value, path, &placeholder(name));
                }
                text = value.to_string();
            }
        }
        TemplatePostData {
            mime_type: post.mime_type.clone(),
            text,
        }
    });

    let template = SiteTemplate {
        version: TEMPLATE_VERSION,
        source: source.to_string(),
        entry_count: file.log.entries.len(),
        request: TemplateRequest {
            method: entry.request.method.clone(),
            url: entry.request.url.clone(),
            headers,
            query,
            post_data,
        },
        variables: originals.iter().map(|(v, _)| v.clone()).collect(),
        assert: TemplateAssert {
            status: entry.response.status,
        },
    };

    // 落库前自检：模板里不得残留敏感原值（防密文泄漏）
    let json = serde_json::to_string(&template).map_err(|e| HarError::Structure(e.to_string()))?;
    for (variable, original) in &originals {
        if variable.sensitive && !original.is_empty() && json.contains(original.as_str()) {
            return Err(HarError::SecretLeak);
        }
    }

    Ok((template, originals))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份最小但真实的 HAR 1.2 结构（字段名与浏览器导出一致）。
    const SAMPLE: &str = r#"{
      "log": {
        "version": "1.2",
        "creator": { "name": "test", "version": "1" },
        "entries": [
          {
            "startedDateTime": "2026-11-26T15:00:00.000Z",
            "time": 120,
            "request": {
              "method": "GET",
              "url": "https://example.invalid/api/v1/users/login/info",
              "httpVersion": "HTTP/1.1",
              "cookies": [],
              "headers": [
                { "name": "Accept", "value": "application/json" },
                { "name": "Cookie", "value": "acw_tc=abc; csrf_session=sess-1; csrf_token=tok-1" },
                { "name": "X-Trace-Id", "value": "trace-1" }
              ],
              "queryString": [],
              "headersSize": -1,
              "bodySize": 0
            },
            "response": {
              "status": 401,
              "statusText": "UNAUTHORIZED",
              "httpVersion": "HTTP/1.1",
              "cookies": [],
              "headers": [
                { "name": "Set-Cookie", "value": "csrf_session=NEW-SECRET; csrf_token=NEW-TOKEN" }
              ],
              "content": { "size": 12, "mimeType": "application/json", "text": "{\"a\":1}" },
              "contentSize": 12
            },
            "cache": {},
            "timings": { "send": 1, "wait": 100, "receive": 19 }
          },
          {
            "startedDateTime": "2026-11-26T15:00:01.000Z",
            "time": 20,
            "request": {
              "method": "POST",
              "url": "https://example.invalid/api/v1/checkin",
              "httpVersion": "HTTP/1.1",
              "cookies": [],
              "headers": [
                { "name": "Content-Type", "value": "application/json" },
                { "name": "X-CSRF-Token", "value": "tok-2" }
              ],
              "queryString": [ { "name": "channel", "value": "web" } ],
              "postData": {
                "mimeType": "application/json",
                "text": "{\"csrf\":\"body-token\",\"note\":\"hello\"}"
              }
            },
            "response": {
              "status": 200,
              "statusText": "OK",
              "httpVersion": "HTTP/1.1",
              "cookies": [],
              "headers": [],
              "content": { "size": 2, "mimeType": "application/json", "text": "{}" },
              "contentSize": 2
            },
            "cache": {},
            "timings": { "send": 1, "wait": 10, "receive": 9 }
          }
        ]
      }
    }"#;

    fn parsed() -> HarFile {
        parse_har(SAMPLE.as_bytes()).expect("样例 HAR 应能解析")
    }

    #[test]
    fn parses_entries_and_keeps_only_the_subset() {
        let file = parsed();
        assert_eq!(file.log.version, "1.2");
        assert_eq!(file.log.entries.len(), 2);
        let first = &file.log.entries[0];
        assert_eq!(first.request.method, "GET");
        assert_eq!(first.response.status, 401);
    }

    #[test]
    fn sensitive_values_are_masked_in_the_import_view() {
        let file = parsed();
        let view = to_import(&file, "sample.har".to_string());
        let first = &view.entries[0];
        let cookie = first
            .fields
            .iter()
            .find(|f| f.target.label() == "请求头 Cookie · csrf_token")
            .expect("应有 csrf_token 候选");
        assert_eq!(cookie.value, MASK, "敏感值必须掩码");
        assert!(cookie.sensitive);
        assert!(cookie.suggested);
        // 非敏感值照常显示
        let accept = first
            .fields
            .iter()
            .find(|f| f.target.label() == "请求头 Accept")
            .expect("应有 Accept");
        assert_eq!(accept.value, "application/json");
        assert!(!accept.sensitive);
    }

    #[test]
    fn response_headers_and_body_are_never_taken() {
        let file = parsed();
        let view = to_import(&file, "sample.har".to_string());
        let json = serde_json::to_string(&view).unwrap();
        assert!(
            !json.contains("NEW-SECRET"),
            "响应的 Set-Cookie 不得进入导入视图"
        );
        assert!(
            !json.contains("csrf_session=NEW"),
            "响应体/响应头不得进入导入视图"
        );
    }

    #[test]
    fn marks_two_variables_and_keeps_the_rest_of_the_cookie() {
        let file = parsed();
        let marked = vec![
            MarkedVariable {
                name: "cookie".to_string(),
                target: VarTarget::HeaderPart {
                    field: "Cookie".to_string(),
                    part: "csrf_session".to_string(),
                },
            },
            MarkedVariable {
                name: "csrf".to_string(),
                target: VarTarget::HeaderPart {
                    field: "Cookie".to_string(),
                    part: "csrf_token".to_string(),
                },
            },
        ];
        let (template, originals) =
            build_template(&file, "sample.har", 0, &marked).expect("应能生成模板");

        // 用户标了 2 个；`acw_tc` 未标但敏感 → 系统自动补 1 个（02 §7.1.2）
        assert_eq!(template.variables.len(), 3);
        assert_eq!(template.variables[0].name, "cookie");
        assert!(template.variables[0].sensitive);
        assert!(!template.variables[0].auto);
        assert!(template.variables[0].credential_id.is_none());
        assert!(template.variables[0].default.is_none(), "敏感值不进模板");
        assert_eq!(template.variables[2].name, "auto_acw_tc");
        assert!(template.variables[2].auto);
        assert_eq!(originals[0].1, "sess-1");
        assert_eq!(originals[1].1, "tok-1");
        assert_eq!(originals[2].1, "abc");

        let cookie = template
            .request
            .headers
            .iter()
            .find(|h| h.name == "Cookie")
            .expect("应有 Cookie 头");
        assert_eq!(
            cookie.value,
            "acw_tc={{auto_acw_tc}}; csrf_session={{cookie}}; csrf_token={{csrf}}"
        );
        assert_eq!(template.assert.status, 401);
    }

    #[test]
    fn template_contains_no_plain_secret() {
        let file = parsed();
        let marked = vec![MarkedVariable {
            name: "csrf".to_string(),
            target: VarTarget::HeaderPart {
                field: "Cookie".to_string(),
                part: "csrf_token".to_string(),
            },
        }];
        let (template, _) = build_template(&file, "sample.har", 0, &marked).unwrap();
        let json = serde_json::to_string(&template).unwrap();
        assert!(!json.contains("tok-1"), "模板里不得有敏感原值");
        assert!(!json.contains("sess-1"));
        assert!(json.contains("{{csrf}}"));
    }

    #[test]
    fn non_sensitive_variable_keeps_its_default_inline() {
        let file = parsed();
        let marked = vec![MarkedVariable {
            name: "trace".to_string(),
            target: VarTarget::Header {
                field: "X-Trace-Id".to_string(),
            },
        }];
        let (template, _) = build_template(&file, "sample.har", 0, &marked).unwrap();
        assert!(!template.variables[0].sensitive);
        assert_eq!(template.variables[0].default.as_deref(), Some("trace-1"));
        let header = template
            .request
            .headers
            .iter()
            .find(|h| h.name == "X-Trace-Id")
            .unwrap();
        assert_eq!(header.value, "{{trace}}");
    }

    #[test]
    fn body_and_query_marks_work() {
        let file = parsed();
        let marked = vec![
            MarkedVariable {
                name: "csrf".to_string(),
                target: VarTarget::Header {
                    field: "X-CSRF-Token".to_string(),
                },
            },
            MarkedVariable {
                name: "body_csrf".to_string(),
                target: VarTarget::Body {
                    field: "csrf".to_string(),
                },
            },
            MarkedVariable {
                name: "channel".to_string(),
                target: VarTarget::Query {
                    field: "channel".to_string(),
                },
            },
        ];
        let (template, _) = build_template(&file, "sample.har", 1, &marked).unwrap();
        let header = template
            .request
            .headers
            .iter()
            .find(|h| h.name == "X-CSRF-Token")
            .unwrap();
        assert_eq!(header.value, "{{csrf}}");
        assert_eq!(template.request.query[0].value, "{{channel}}");
        let body = template.request.post_data.as_ref().unwrap();
        assert!(body.text.contains("{{body_csrf}}"));
        assert!(body.text.contains("hello"), "其它字段要原样保留");
    }

    #[test]
    fn duplicate_variable_names_are_rejected() {
        let file = parsed();
        let marked = vec![
            MarkedVariable {
                name: "a".to_string(),
                target: VarTarget::Header {
                    field: "Accept".to_string(),
                },
            },
            MarkedVariable {
                name: "a".to_string(),
                target: VarTarget::Query {
                    field: "channel".to_string(),
                },
            },
        ];
        assert!(build_template(&file, "s.har", 0, &marked).is_err());
    }

    #[test]
    fn invalid_variable_names_are_rejected() {
        assert!(validate_var_name("").is_err());
        assert!(validate_var_name("has space").is_err());
        assert!(validate_var_name("ok-name_1").is_ok());
        assert!(validate_var_name(&"x".repeat(VAR_NAME_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn missing_target_is_reported() {
        let file = parsed();
        let marked = vec![MarkedVariable {
            name: "x".to_string(),
            target: VarTarget::Header {
                field: "Not-There".to_string(),
            },
        }];
        let err = build_template(&file, "s.har", 0, &marked).unwrap_err();
        assert!(matches!(err, HarError::TargetNotFound(_)));
    }

    #[test]
    fn broken_json_is_rejected() {
        assert!(matches!(
            parse_har(b"{not json"),
            Err(HarError::Json(_))
        ));
    }

    #[test]
    fn cookie_pairs_keep_equals_inside_the_value() {
        let pairs = parse_cookie_pairs("a=1; b=x=y=z; c=");
        assert_eq!(pairs[0], ("a".to_string(), "1".to_string()));
        assert_eq!(pairs[1], ("b".to_string(), "x=y=z".to_string()));
        assert_eq!(pairs[2], ("c".to_string(), String::new()));
    }

    #[test]
    fn sensitive_name_detection_is_case_insensitive() {
        assert!(is_sensitive_name("Cookie"));
        assert!(is_sensitive_name("X-CSRF-Token"));
        assert!(is_sensitive_name("csrf_session"));
        assert!(is_sensitive_name("acw_tc"));
        assert!(!is_sensitive_name("Accept"));
        assert!(!is_sensitive_name("X-Trace-Id"));
    }
}
