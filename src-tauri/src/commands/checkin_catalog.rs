//! 站点目录与声明式站点的 IPC（T25；02 §7.6）。
//!
//! 五个动作：
//! 1. **列内置目录**（已实测条目 + 空白模板）；
//! 2. **新增/更新一个声明式站点**（登录态只进保险库）；
//! 3. **导入一份目录**（逐条校验、非法条目带原因、默认不启用）；
//! 4. **导出自建站点为目录**（**只含凭据 id，不含密文**）；
//! 5. **查当前余额**（与「本次拿到」是两个概念，**不落 `quota_json`**）。
//!
//! **密文边界（AGENT.md §5.3）**：登录态只写进保险库；站点行里只有 `credentialId`（id）。

use tauri::State;

use crate::checkin::catalog::{self, CatalogEntry};
use crate::checkin::spec::{self, SpecSite, SpecSiteConfig, PLUGIN_SPEC};
use crate::commands::checkin::{summarize, SiteSummary};
use crate::store::checkin::{self, SiteRow};
use crate::vault::crypto::SecretBytes;
use crate::AppState;

/// 一次目录导入的结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogImportSummary {
    /// 成功建站（同名 spec id 视为更新）。
    pub imported: Vec<SiteSummary>,
    /// **失败条目带原因**（不静默丢弃）。
    pub errors: Vec<CatalogImportError>,
}

/// 目录里一条没建成的站点。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogImportError {
    /// 站点名。
    pub name: String,
    /// 站点 id。
    pub id: String,
    /// 人话原因。
    pub reason: String,
}

/// 余额查询的结果（**「当前余额」，不是「本次拿到」**）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceSummary {
    pub site_id: String,
    pub site_name: String,
    /// 当前余额数值。
    pub amount: i64,
    /// 单位（默认 `point`）。
    pub unit: String,
    /// 人话说明。
    pub message: String,
}

/// 内置目录（**只收已实测条目 + 一个空白模板**）。
#[tauri::command]
pub fn checkin_catalog_list() -> Result<Vec<CatalogEntry>, String> {
    Ok(catalog::builtin_catalog())
}

/// 新增（或更新）一个声明式站点。
///
/// `secret` = 登录态（token / Cookie）；**只进保险库**，不回显、不进日志。
#[tauri::command]
pub fn checkin_add_spec_site(
    state: State<'_, AppState>,
    spec: SpecSite,
    secret: Option<String>,
) -> Result<SiteSummary, String> {
    checkin_add_spec_site_of(&state, &spec, secret.as_deref())
}

/// 新增声明式站点（可测版本）。
pub fn checkin_add_spec_site_of(
    state: &AppState,
    spec: &SpecSite,
    secret: Option<&str>,
) -> Result<SiteSummary, String> {
    spec.validate()?;
    let site = upsert_spec_site(state, spec, secret)?;
    summarize(state, &site)
}

/// 导入一份目录（**逐条校验；整份非法直接拒绝，半吊子不导入**）。
#[tauri::command]
pub fn checkin_import_catalog(
    state: State<'_, AppState>,
    content: String,
) -> Result<CatalogImportSummary, String> {
    checkin_import_catalog_of(&state, &content)
}

/// 导入目录（可测版本）。
pub fn checkin_import_catalog_of(
    state: &AppState,
    content: &str,
) -> Result<CatalogImportSummary, String> {
    let parsed = catalog::parse_catalog(content)?;
    let mut imported = Vec::new();
    let mut errors = Vec::new();
    for spec in &parsed.sites {
        match upsert_spec_site(state, spec, None) {
            Ok(site) => match summarize(state, &site) {
                Ok(summary) => imported.push(summary),
                Err(reason) => errors.push(CatalogImportError {
                    name: spec.name.clone(),
                    id: spec.id.clone(),
                    reason,
                }),
            },
            Err(reason) => errors.push(CatalogImportError {
                name: spec.name.clone(),
                id: spec.id.clone(),
                reason,
            }),
        }
    }
    Ok(CatalogImportSummary { imported, errors })
}

/// 导出自建（声明式）站点为目录文本（**只含凭据 id，不含密文**）。
#[tauri::command]
pub fn checkin_export_catalog(state: State<'_, AppState>) -> Result<String, String> {
    checkin_export_catalog_of(&state)
}

/// 导出目录（可测版本）。
pub fn checkin_export_catalog_of(state: &AppState) -> Result<String, String> {
    let sites = state
        .store
        .with_conn(|conn| checkin::list_sites(conn))
        .map_err(|e| e.to_string())?;
    let mut specs: Vec<SpecSite> = Vec::new();
    for site in sites {
        if site.plugin != PLUGIN_SPEC {
            continue;
        }
        let config = match spec::parse_site_config(&site) {
            Ok(config) => config,
            Err(_) => continue,
        };
        let mut spec = config.spec.clone();
        // 导出的目录带上**本机的凭据 id**（只是 id，别的机器上无效、需要自己填）
        spec.credential_id = config.credential_id.clone();
        specs.push(spec);
    }
    Ok(catalog::to_catalog_json(&specs))
}

/// 查一个声明式站点的**当前余额**。
///
/// **与「本次拿到」严格区分**：余额只回前端展示，**不落 `quota_json`**（不与限额自学习字段打架）。
#[tauri::command]
pub async fn checkin_balance(
    state: State<'_, AppState>,
    id: String,
) -> Result<BalanceSummary, String> {
    checkin_balance_of(&state, &id).await
}

/// 查余额（可测版本）。
pub async fn checkin_balance_of(state: &AppState, id: &str) -> Result<BalanceSummary, String> {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("站点不存在：{id}"))?;
    if row.plugin != PLUGIN_SPEC {
        return Err("只有声明式站点支持查余额".to_string());
    }
    let config = spec::parse_site_config(&row)?;
    let spec = config.effective_spec();
    let balance = spec
        .balance
        .as_ref()
        .ok_or_else(|| "这个站点没配余额查询（balance）".to_string())?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let template = spec.to_balance_template(&today)?;

    let engine = crate::checkin::replay::ReplayEngine::new().map_err(|e| e.to_string())?;
    let resolver = crate::checkin::scheduler::VaultResolver::new(state.vault.clone());
    let (outcome, body) = engine.replay_with_body(&template, &resolver).await;
    if !outcome.ok {
        return Err(format!("查余额失败：{}", outcome.message));
    }
    let body = body.ok_or_else(|| "余额接口返回的不是 JSON".to_string())?;
    let amount = spec::extract_amount(&balance.path, &body).ok_or_else(|| {
        format!("余额字段路径在响应里不存在：{}", balance.path)
    })?;
    Ok(BalanceSummary {
        site_id: row.id,
        site_name: row.name,
        amount,
        unit: spec.balance_unit(),
        message: format!("当前余额 {amount} {}", spec.balance_unit()),
    })
}

/// 建站或更新站（**同 spec id 视为更新** → 目录导入幂等）。
fn upsert_spec_site(
    state: &AppState,
    spec: &SpecSite,
    secret: Option<&str>,
) -> Result<SiteRow, String> {
    // **所有建站路径都过这一道校验**（新增、目录导入都走这里）
    spec.validate()?;

    // 1) 找已有站点（按 spec id 找，不按名字——名字可以改，id 才是身份）
    let existing = state
        .store
        .with_conn(|conn| find_spec_site(conn, &spec.id))
        .map_err(|e| e.to_string())?;

    // 2) 归属渠道（凭据必须挂在渠道上）与默认账号（积分回写落在它的 quota_json）
    let channel_id = {
        let vault = state.vault.read().map_err(|e| e.to_string())?;
        vault.ensure_channel(&format!("签到 · {}", spec.name))
            .map_err(|e| e.to_string())?
    };
    let account_id = crate::store::quota::ensure_account(&state.store, &channel_id)
        .map_err(|e| e.to_string())?;

    // 3) 登录态：**只进保险库**
    let mut credential_id = existing
        .as_ref()
        .and_then(|site| spec::parse_site_config(site).ok())
        .and_then(|config| config.credential_id);
    if let Some(secret) = secret.map(str::trim).filter(|s| !s.is_empty()) {
        let vault = state.vault.read().map_err(|e| e.to_string())?;
        let bytes = SecretBytes::from_slice(secret.as_bytes());
        let id = vault
            .put_credential(&channel_id, "access_token", &bytes)
            .map_err(|e| e.to_string())?;
        // 只打印 id，明文不落日志
        println!("[checkin] 声明式站点 {} 的登录态已存进保险库（明文不落日志）", spec.id);
        credential_id = Some(id);
    }

    // 4) 站点行
    let config = SpecSiteConfig {
        credential_id,
        channel_id: Some(channel_id),
        account_id: Some(account_id),
        spec: {
            let mut stored = spec.clone();
            // 站点自己的登录态由 config 持有，spec 里不重复存（避免两份 id 打架）
            stored.credential_id = None;
            stored
        },
    };
    let row = SiteRow {
        id: existing
            .as_ref()
            .map(|site| site.id.clone())
            .unwrap_or_else(checkin::new_site_id),
        name: spec.name.clone(),
        plugin: PLUGIN_SPEC.to_string(),
        config_json: Some(config.to_json()),
        // 已有站点保留原 cron；新站点用 spec 里的，没有就用默认
        schedule: existing
            .as_ref()
            .and_then(|site| site.schedule.clone())
            .or_else(|| spec.schedule.clone())
            .or_else(|| Some(spec::DEFAULT_SCHEDULE.to_string())),
        // 02 §7.3：站点插件默认全关
        enabled: existing.as_ref().map(|site| site.enabled).unwrap_or(false),
    };
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &row))
        .map_err(|e| e.to_string())?;
    Ok(row)
}

/// 按 spec id 找站点（**id 是身份，名字可以改**）。
fn find_spec_site(
    conn: &rusqlite::Connection,
    spec_id: &str,
) -> crate::store::Result<Option<SiteRow>> {
    for site in checkin::list_sites(conn)? {
        if site.plugin != PLUGIN_SPEC {
            continue;
        }
        if let Ok(config) = spec::parse_site_config(&site) {
            if config.spec.id == spec_id {
                return Ok(Some(site));
            }
        }
    }
    Ok(None)
}
