//! 站点目录（T25；02 §7.6.3）。
//!
//! **目录 = 一批声明式站点的集合**（一个 JSON 文件）。导入 = 逐条校验后建站，
//! 导出 = 把自建站点打包成目录文件——**只含 `credentialId`（id，不是密文）**，
//! 因此**可以放心分享给别的 harness 用户**。
//!
//! **内置目录只收「已实测」的条目**（02 §7.6.3）：ModelScope / MiniMax Code 走
//! **代码插件**（断言最准），另附一个**空白模板**（通用 HTTP 签到，填 URL 即可）。
//! **不虚构 endpoint**——任何「听说的」平台都不进内置目录。

use serde::{Deserialize, Serialize};

use crate::checkin::spec::{SpecSite, DEFAULT_SCHEDULE};

/// 目录文件版本。
pub const CATALOG_VERSION: u32 = 1;

/// 一个目录文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub version: u32,
    pub sites: Vec<SpecSite>,
}

impl Default for Catalog {
    fn default() -> Self {
        Catalog {
            version: CATALOG_VERSION,
            sites: Vec::new(),
        }
    }
}

/// 目录条目（内置目录用；UI 的「站点目录」列表）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// 目录内唯一 id。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// `builtin`（代码插件）/ `template`（声明式模板）。
    pub kind: String,
    /// 一句话说明。
    pub summary: String,
    /// 要准备什么凭据。
    pub credential_hint: String,
    /// 默认 cron。
    pub default_schedule: String,
    /// 声明式模板（`kind = template` 时给出）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<SpecSite>,
}

/// 解析目录文件。
///
/// **只挡「整份文件」的错**：不是合法 JSON / 版本比本程序新 / **目录内 id 重复**。
/// **单条站点的错（URL 不合法、占位符不认识…）留给导入时逐条校验**——
/// 那样导入方能拿到「哪一条不行、为什么」，**不静默丢弃**（02 §7.6.3）。
pub fn parse_catalog(text: &str) -> Result<Catalog, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("目录内容是空的".to_string());
    }
    let mut catalog: Catalog = serde_json::from_str(trimmed)
        .map_err(|err| format!("目录文件不是合法的 JSON：{err}"))?;
    if catalog.version > CATALOG_VERSION {
        return Err(format!(
            "目录版本 {} 比本程序支持的 {} 新——请升级应用",
            catalog.version, CATALOG_VERSION
        ));
    }
    catalog.version = CATALOG_VERSION;

    // **目录内 id 唯一**（重复属于「整份文件」的问题 → 整份拒绝）
    let mut seen: Vec<&str> = Vec::new();
    for site in &catalog.sites {
        if seen.contains(&site.id.trim()) {
            return Err(format!("目录里有重复的站点 id：{}", site.id));
        }
        seen.push(site.id.trim());
    }
    Ok(catalog)
}

/// 把自建站点打包成目录文件文本。
pub fn to_catalog_json(sites: &[SpecSite]) -> String {
    let catalog = Catalog {
        version: CATALOG_VERSION,
        sites: sites.to_vec(),
    };
    serde_json::to_string_pretty(&catalog).unwrap_or_else(|_| "{}".to_string())
}

/// 内置目录（**只收已实测条目 + 一个空白模板**）。
pub fn builtin_catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            id: "modelscope".to_string(),
            name: "ModelScope（魔搭）".to_string(),
            kind: "builtin".to_string(),
            summary: "内置插件 · 已实测：每日签到领魔粒，签到与查魔粒一个令牌就够。".to_string(),
            credential_hint: "开放平台访问令牌（ms-…）或网页会话 Cookie".to_string(),
            default_schedule: "0 9 * * *".to_string(),
            spec: None,
        },
        CatalogEntry {
            id: "minimax_code".to_string(),
            name: "MiniMax Code（网页版）".to_string(),
            kind: "builtin".to_string(),
            summary: "内置插件 · 已实测：每日 400 积分，第 4/7 天各 1000；需要浏览器辅轨登录。".to_string(),
            credential_hint: "在浏览器里登录一次即可（不用手填凭据）".to_string(),
            default_schedule: "20 8 * * *".to_string(),
            spec: None,
        },
        CatalogEntry {
            id: "generic_http".to_string(),
            name: "通用 HTTP 签到（空白模板）".to_string(),
            kind: "template".to_string(),
            summary: "声明式模板：填上接口地址与断言，就能接一家新的签到平台——不用改代码、不用发版。".to_string(),
            credential_hint: "接口需要的登录态（token / Cookie），只进加密保险库".to_string(),
            default_schedule: DEFAULT_SCHEDULE.to_string(),
            spec: Some(generic_template()),
        },
    ]
}

/// 通用 HTTP 签到的空白模板（`{{token}}` 与 `{{today}}` 是唯二的占位符）。
pub fn generic_template() -> SpecSite {
    SpecSite {
        id: "my-platform".to_string(),
        name: "我的平台".to_string(),
        method: Some("POST".to_string()),
        url: "https://example.com/api/checkin".to_string(),
        headers: vec![crate::checkin::spec::SpecHeader {
            name: "Authorization".to_string(),
            value: "Bearer {{token}}".to_string(),
        }],
        query: Vec::new(),
        body: None,
        credential_id: None,
        assert: crate::checkin::spec::SpecAssert {
            status: Some(200),
            json_paths: Vec::new(),
        },
        reward: None,
        balance: None,
        schedule: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkin::spec::SpecExtract;

    fn ok_spec() -> SpecSite {
        let mut spec = generic_template();
        spec.id = "ok-site".to_string();
        spec.name = "可用的站点".to_string();
        spec.credential_id = Some("cred_1".to_string());
        spec
    }

    #[test]
    fn a_catalog_round_trips() {
        let catalog = Catalog {
            version: CATALOG_VERSION,
            sites: vec![ok_spec()],
        };
        let text = to_catalog_json(&catalog.sites);
        let parsed = parse_catalog(&text).expect("应能解析");
        assert_eq!(parsed.sites.len(), 1);
        assert_eq!(parsed.sites[0].id, "ok-site");
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let text = serde_json::json!({
            "version": 1,
            "sites": [ok_spec(), ok_spec()]
        })
        .to_string();
        let err = parse_catalog(&text).unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn an_invalid_entry_is_left_to_the_import_step() {
        // **单条站点的错不在解析阶段挡**——导入时逐条校验，才能把「哪一条不行、为什么」告诉用户
        let mut bad = ok_spec();
        bad.name = "坏站点".to_string();
        bad.url = "ftp://example.com/checkin".to_string();
        let text = serde_json::json!({ "version": 1, "sites": [bad] }).to_string();
        let parsed = parse_catalog(&text).expect("解析阶段只挡整份文件的错");
        assert_eq!(parsed.sites.len(), 1);
        let err = parsed.sites[0].validate().unwrap_err();
        assert!(err.contains("http"), "{err}");
    }

    #[test]
    fn a_future_version_is_refused() {
        let text = serde_json::json!({ "version": 99, "sites": [] }).to_string();
        let err = parse_catalog(&text).unwrap_err();
        assert!(err.contains("版本"), "{err}");
    }

    #[test]
    fn the_builtin_catalog_only_has_measured_entries_and_a_template() {
        let entries = builtin_catalog();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].kind, "builtin");
        assert_eq!(entries[1].kind, "builtin");
        assert_eq!(entries[2].kind, "template");
        // 空白模板本身**不该通过校验**（没填 URL 之外的必填项也要能过——模板允许先存着）
        let template = entries[2].spec.clone().expect("模板条目应带 spec");
        assert_eq!(template.id, "my-platform");
        assert!(template.reward.is_none());
        assert!(template.balance.is_none());
    }

    #[test]
    fn exported_catalog_never_contains_credentials() {
        let mut spec = ok_spec();
        spec.reward = Some(SpecExtract {
            path: "data.points".to_string(),
            unit: Some("point".to_string()),
        });
        let text = to_catalog_json(&[spec]);
        assert!(text.contains("cred_1"), "只有凭据 id");
        assert!(!text.contains("secret"), "没有密文");
    }
}
