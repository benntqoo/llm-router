//! 签到站点模板 IPC（T15；02 §7.1.1 / §7.1.2）。
//!
//! 四个动作：**导入 HAR** → 选 entry → **标记变量** → **保存模板**（+ 列出 / 删除）。
//!
//! 密文边界（AGENT.md §5.3）：
//! - 导入返回给 UI 的是**掩码值**，敏感原值不出 Rust 进程；
//! - 保存时前端回传**文件路径 + 标记清单**，Rust 重新读取并解析（不留导入态）；
//! - 变量的真实取值写**保险库**（`credential` 表密文），模板里只留占位符；
//! - 模板落库前自检：不得残留被标记字段的原值。

use tauri::State;

use crate::checkin::har::{self, credential_kind, HarImport, MarkedVariable, SiteTemplate};
use crate::store::checkin::{self, SiteRow, PLUGIN_HAR};
use crate::vault::crypto::SecretBytes;
use crate::AppState;

/// 保存模板的入参（IPC payload；字段名按 Tauri 惯例用 camelCase）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveTemplateInput {
    /// HAR 文件路径（用户在 UI 里选的本地文件）。
    pub path: String,
    /// 选中的 entry 下标。
    pub entry_index: usize,
    /// 站点名。
    pub site_name: String,
    /// 归属渠道 id（已有渠道）。
    pub channel_id: Option<String>,
    /// 归属渠道名（没有 id 时按名字建一个）。
    pub channel_name: Option<String>,
    /// 用户标记好的变量清单。
    #[serde(default)]
    pub variables: Vec<MarkedVariable>,
}

/// 模板摘要（列表展示用；不含任何密文）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSummary {
    pub source: String,
    pub method: String,
    pub url: String,
    /// 录制时的响应状态码（重放断言的初值；JSON 路径断言属 T16）。
    pub status: i64,
    pub variable_count: usize,
    pub variable_names: Vec<String>,
}

/// 签到站点摘要。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteSummary {
    pub id: String,
    pub name: String,
    pub plugin: String,
    pub enabled: bool,
    pub schedule: Option<String>,
    pub template: Option<TemplateSummary>,
    /// 下一次应触发时刻（Unix 秒；没有 cron 则为 `None`）。
    pub next_run_at: Option<i64>,
    /// 最近一次尝试（不论成败）的时刻（Unix 秒）。
    pub last_attempt_at: Option<i64>,
    /// 最近一次尝试是否成功。
    pub last_ok: Option<bool>,
    /// 系统兜底任务名（Windows 计划任务 / macOS launchd）。
    pub system_task: Option<String>,
    /// 系统兜底任务是否已注册。
    pub system_task_registered: bool,
    /// 系统兜底任务的频率描述。
    pub system_task_frequency: Option<String>,
    /// 系统兜底同步时的错误（注册失败等）。
    pub system_task_error: Option<String>,
    /// 内置插件的登录态是否已填（HAR 模板恒为 `false`）。
    pub credential_ready: bool,
    /// 内置插件是否需要浏览器辅轨。
    pub needs_sidecar: bool,
    /// 内置插件的凭据提示（HAR 模板为 `None`）。
    pub credential_hint: Option<String>,
    /// 该站点归属账号的积分快照（02 §7.4 §4）。
    pub reward: Option<crate::store::account::RewardSnapshot>,
}

/// 一次执行的摘要（IPC 返回）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub site_id: String,
    pub site_name: String,
    pub ok: bool,
    /// 错误分类（`success` / `assert_failed` / …）。
    pub class: String,
    pub message: String,
    pub status: Option<u16>,
    pub elapsed_ms: u64,
    pub retryable: bool,
}

/// 一次签到结果的展示视图。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultView {
    pub id: i64,
    pub site_id: String,
    pub ts: i64,
    pub ok: bool,
    pub reward_json: Option<String>,
    pub message: Option<String>,
}

/// 系统兜底同步的摘要。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSyncSummary {
    /// 已注册的任务名。
    pub registered: Vec<String>,
    /// 已注销的任务名。
    pub unregistered: Vec<String>,
    /// 出错的站点（站点名 + 原因）。
    pub errors: Vec<String>,
    /// 当前平台是否支持。
    pub supported: bool,
}

/// 导入一个 HAR 文件（**敏感值掩码**）。
#[tauri::command]
pub fn checkin_import_har(path: String) -> Result<HarImport, String> {
    checkin_import_har_of(&path)
}

/// 导入一个 HAR 文件（可测版本）。
pub fn checkin_import_har_of(path: &str) -> Result<HarImport, String> {
    har::import_from_path(path).map_err(|e| e.to_string())
}

/// 保存一个签到站点模板（含写保险库）。
#[tauri::command]
pub fn checkin_save_template(
    state: State<'_, AppState>,
    input: SaveTemplateInput,
) -> Result<SiteSummary, String> {
    checkin_save_template_of(&state, &input)
}

/// 保存一个签到站点模板（可测版本）。
pub fn checkin_save_template_of(
    state: &AppState,
    input: &SaveTemplateInput,
) -> Result<SiteSummary, String> {
    let site_name = input.site_name.trim();
    if site_name.is_empty() {
        return Err("站点名不能为空".to_string());
    }

    // 1) 重新读取并解析 HAR（导入态不驻留内存；02 §7.1.2）
    let bytes = std::fs::read(&input.path).map_err(|e| format!("读取 HAR 失败: {e}"))?;
    let file = har::parse_har(&bytes).map_err(|e| e.to_string())?;
    let source = har::file_name(&input.path);

    // 2) 生成模板（标记过的值换成占位符；原始值只在这里出现一次）
    let (mut template, originals) = har::build_template(
        &file,
        &source,
        input.entry_index,
        &input.variables,
    )
    .map_err(|e| e.to_string())?;

    // 3) 解析归属渠道（02 §7.1.2：credential.channel_id 是 NOT NULL 外键）
    let channel_id = resolve_channel(state, input)?;
    let vault = state.vault.read().map_err(|e| e.to_string())?;

    // 4) 敏感变量的真实值写保险库（密文），模板里只留占位符 + 凭证 id
    let mut old_credential_ids: Vec<String> = Vec::new();
    for (index, (variable, original)) in originals.iter().enumerate() {
        if !variable.sensitive {
            continue;
        }
        let secret = SecretBytes::from_slice(original.as_bytes());
        let credential_id = vault
            .put_credential(&channel_id, credential_kind(&variable.target), &secret)
            .map_err(|e| e.to_string())?;
        template.variables[index].credential_id = Some(credential_id);
    }
    drop(vault);

    // 5) 落库前自检：模板里不得残留敏感原值（**只查可能含密文的请求面**，
    //    不查整段 JSON——随机 `credential_id` 会让短密文假阳性）
    let surface = har::sensitive_surface(&template);
    for (variable, original) in &originals {
        if variable.sensitive && !original.is_empty() && surface.contains(original.as_str()) {
            return Err("模板里残留了敏感原值，已拒绝保存（防密文泄漏）".to_string());
        }
    }
    let config_json = serde_json::to_string(&template).map_err(|e| e.to_string())?;

    // 6) 同名站点 → 复用同一行（重录 HAR 覆盖模板）；旧凭据在模板更新后清理
    let existing = state
        .store
        .with_conn(|conn| checkin::find_site_by_name(conn, site_name))
        .map_err(|e| e.to_string())?;
    if let Some(previous) = &existing {
        if let Some(previous_template) = previous
            .config_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<SiteTemplate>(json).ok())
        {
            for variable in &previous_template.variables {
                if let Some(id) = &variable.credential_id {
                    old_credential_ids.push(id.clone());
                }
            }
        }
    }
    let site_id = existing
        .as_ref()
        .map(|site| site.id.clone())
        .unwrap_or_else(checkin::new_site_id);

    let row = SiteRow {
        id: site_id,
        name: site_name.to_string(),
        plugin: PLUGIN_HAR.to_string(),
        config_json: Some(config_json),
        // 调度属 T17：已有站点保留原 cron，新站点留空
        schedule: existing.as_ref().and_then(|site| site.schedule.clone()),
        // 02 §7.3：站点插件默认全关
        enabled: existing.as_ref().map(|site| site.enabled).unwrap_or(false),
    };
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &row))
        .map_err(|e| e.to_string())?;

    // 7) 清理上一版模板引用的旧凭据（**模板已更新后**才删，避免中途失败丢凭据）
    {
        let vault = state.vault.read().map_err(|e| e.to_string())?;
        let new_ids: Vec<String> = template
            .variables
            .iter()
            .filter_map(|v| v.credential_id.clone())
            .collect();
        for id in old_credential_ids {
            if !new_ids.contains(&id) {
                let _ = vault.delete_credential(&id);
            }
        }
    }

    summarize(state, &row)
}

/// 列出全部签到站点。
#[tauri::command]
pub fn checkin_list_sites(state: State<'_, AppState>) -> Result<Vec<SiteSummary>, String> {
    checkin_list_sites_of(&state)
}

/// 列出全部签到站点（可测版本）。
pub fn checkin_list_sites_of(state: &AppState) -> Result<Vec<SiteSummary>, String> {
    let rows = state
        .store
        .with_conn(|conn| checkin::list_sites(conn))
        .map_err(|e| e.to_string())?;
    rows.iter()
        .map(|row| summarize(state, row))
        .collect()
}

/// 删一个签到站点（连同它在保险库里的凭据）。
#[tauri::command]
pub fn checkin_delete_site(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    checkin_delete_site_of(&state, &id)
}

/// 删一个签到站点（可测版本）。
pub fn checkin_delete_site_of(state: &AppState, id: &str) -> Result<bool, String> {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, id))
        .map_err(|e| e.to_string())?;
    let removed = state
        .store
        .with_conn(|conn| checkin::delete_site(conn, id))
        .map_err(|e| e.to_string())?;
    if removed {
        if let Some(row) = row {
            if let Some(json) = row.config_json.as_deref() {
                if let Ok(template) = serde_json::from_str::<SiteTemplate>(json) {
                    let vault = state.vault.read().map_err(|e| e.to_string())?;
                    for variable in &template.variables {
                        if let Some(credential_id) = &variable.credential_id {
                            let _ = vault.delete_credential(credential_id);
                        }
                    }
                }
            }
        }
    }
    Ok(removed)
}

// ────────────────────────────── 调度（T17） ──────────────────────────────

/// 设站点的 cron 表达式与启用开关，并**同步系统兜底任务**。
///
/// 口径（02 §7.2.1 §4）：启用且有 cron → 注册系统兜底任务；否则 → 注销。
#[tauri::command]
pub fn checkin_set_schedule(
    state: State<'_, AppState>,
    id: String,
    schedule: Option<String>,
    enabled: bool,
) -> Result<SiteSummary, String> {
    checkin_set_schedule_of(&state, &id, schedule, enabled)
}

/// 设 cron 与启用开关（可测版本）。
pub fn checkin_set_schedule_of(
    state: &AppState,
    id: &str,
    schedule: Option<String>,
    enabled: bool,
) -> Result<SiteSummary, String> {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("站点不存在: {id}"))?;
    let schedule = schedule
        .map(|expr| expr.trim().to_string())
        .filter(|expr| !expr.is_empty());
    // cron 表达式非法 → 直接报错（不让用户存一个永远不触发的表达式）
    if let Some(expr) = schedule.as_deref() {
        crate::checkin::cron::parse(expr).map_err(|err| err.to_string())?;
    }
    let mut updated = row.clone();
    updated.schedule = schedule;
    updated.enabled = enabled;
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &updated))
        .map_err(|e| e.to_string())?;

    // 同步系统兜底任务
    let mut error = None;
    let should_register = updated.enabled && updated.schedule.is_some();
    if should_register {
        let expr = updated.schedule.clone().unwrap_or_default();
        if let Err(err) = crate::checkin::system::register(id, &expr) {
            error = Some(err);
        }
    } else {
        if let Err(err) = crate::checkin::system::unregister(id) {
            error = Some(err);
        }
    }

    let mut summary = summarize(state, &updated)?;
    summary.system_task_error = error;
    Ok(summary)
}

/// 立即执行一次（**忽略去重**）。
#[tauri::command]
pub async fn checkin_run_now(
    state: State<'_, AppState>,
    id: String,
) -> Result<RunSummary, String> {
    checkin_run_now_of(&state, &id).await
}

/// 立即执行一次（可测版本）。
pub async fn checkin_run_now_of(state: &AppState, id: &str) -> Result<RunSummary, String> {
    let engine = crate::checkin::replay::ReplayEngine::new().map_err(|e| e.to_string())?;
    let outcome = crate::checkin::scheduler::run_now(&state.store, &state.vault, &engine, id).await?;
    let name = state
        .store
        .with_conn(|conn| checkin::get_site(conn, id))
        .map_err(|e| e.to_string())?
        .map(|row| row.name)
        .unwrap_or_else(|| id.to_string());
    Ok(RunSummary {
        site_id: id.to_string(),
        site_name: name,
        ok: outcome.ok,
        class: outcome.class.as_str().to_string(),
        message: outcome.message.clone(),
        status: outcome.status,
        elapsed_ms: outcome.elapsed_ms,
        retryable: outcome.retryable,
    })
}

/// 列签到结果（最近 N 条）。
#[tauri::command]
pub fn checkin_results(
    state: State<'_, AppState>,
    id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<ResultView>, String> {
    checkin_results_of(&state, id.as_deref(), limit.unwrap_or(50))
}

/// 列签到结果（可测版本）。
pub fn checkin_results_of(
    state: &AppState,
    id: Option<&str>,
    limit: usize,
) -> Result<Vec<ResultView>, String> {
    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, id, limit))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| ResultView {
            id: row.id,
            site_id: row.site_id,
            ts: row.ts,
            ok: row.ok,
            reward_json: row.reward_json,
            message: row.message,
        })
        .collect())
}

/// 同步系统兜底任务（按当前站点状态全量对齐）。
///
/// 启用且有 cron → 注册；停用 / 没 cron / 站点已删 → 注销。
#[tauri::command]
pub fn checkin_system_sync(state: State<'_, AppState>) -> Result<SystemSyncSummary, String> {
    checkin_system_sync_of(&state)
}

/// 同步系统兜底任务（可测版本）。
pub fn checkin_system_sync_of(state: &AppState) -> Result<SystemSyncSummary, String> {
    let mut summary = SystemSyncSummary {
        registered: Vec::new(),
        unregistered: Vec::new(),
        errors: Vec::new(),
        supported: crate::checkin::system::SUPPORTED,
    };
    if !summary.supported {
        summary
            .errors
            .push("当前平台没有系统兜底实现（只支持 Windows 计划任务 / macOS launchd）".to_string());
        return Ok(summary);
    }
    let rows = state
        .store
        .with_conn(|conn| checkin::list_sites(conn))
        .map_err(|e| e.to_string())?;
    for row in &rows {
        let expr = row.schedule.as_deref().map(str::trim).unwrap_or("");
        if row.enabled && !expr.is_empty() {
            match crate::checkin::system::register(&row.id, expr) {
                Ok(task) => summary.registered.push(task.name),
                Err(err) => summary.errors.push(format!("{}: {err}", row.name)),
            }
        } else {
            match crate::checkin::system::unregister(&row.id) {
                Ok(true) => summary.unregistered.push(crate::checkin::system::task_name(&row.id)),
                Ok(false) => {}
                Err(err) => summary.errors.push(format!("{}: {err}", row.name)),
            }
        }
    }
    Ok(summary)
}

// ────────────────────────────── 站点插件（T18） ──────────────────────────────

/// 内置插件清单（UI 的「添加站点」用）。
#[tauri::command]
pub fn checkin_builtin_sites() -> Result<Vec<crate::checkin::plugin::BuiltinSite>, String> {
    Ok(crate::checkin::plugin::builtin_catalog())
}

/// 建一个内置站点（ModelScope / MiniMax Code）。
///
/// 口径（02 §7.4 §4）：站点归属一个**占位渠道**（`provider = 'custom'`），
/// 渠道下取默认账号——**凭据挂渠道、积分挂账号**（两张表的外键都是 NOT NULL）。
/// 站点**默认不启用**（02 §7.3）。
#[tauri::command]
pub fn checkin_add_builtin_site(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<SiteSummary, String> {
    checkin_add_builtin_site_of(&state, &plugin_id)
}

/// 建内置站点（可测版本）。
pub fn checkin_add_builtin_site_of(
    state: &AppState,
    plugin_id: &str,
) -> Result<SiteSummary, String> {
    let plugin = crate::checkin::plugin::builtin(plugin_id)
        .ok_or_else(|| format!("没有这个内置插件：{plugin_id}"))?;
    let name = plugin.display_name().to_string();

    // 1) 占位渠道（凭据必须挂在渠道上）
    let channel_id = {
        let vault = state.vault.read().map_err(|e| e.to_string())?;
        vault
            .ensure_channel(&format!("签到 · {name}"))
            .map_err(|e| e.to_string())?
    };

    // 2) 默认账号（积分回写落在它的 quota_json）
    let account_id = crate::store::quota::ensure_account(&state.store, &channel_id)
        .map_err(|e| e.to_string())?;

    // 3) 站点行（同名站点复用同一行）
    let existing = state
        .store
        .with_conn(|conn| checkin::find_site_by_name(conn, &name))
        .map_err(|e| e.to_string())?;
    let config = crate::checkin::plugin::PluginSiteConfig {
        channel_id: Some(channel_id),
        account_id: Some(account_id),
        ..Default::default()
    };
    let row = SiteRow {
        id: existing
            .as_ref()
            .map(|site| site.id.clone())
            .unwrap_or_else(checkin::new_site_id),
        name,
        plugin: plugin_id.to_string(),
        config_json: Some(config.to_json()),
        // 已有站点保留原 cron；新站点用插件的默认 cron
        schedule: existing
            .as_ref()
            .and_then(|site| site.schedule.clone())
            .or_else(|| Some(plugin.default_schedule().to_string())),
        // 02 §7.3：站点插件默认全关
        enabled: existing.as_ref().map(|site| site.enabled).unwrap_or(false),
    };
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &row))
        .map_err(|e| e.to_string())?;
    summarize(state, &row)
}

/// 填内置站点的登录态（**明文只进保险库，不回显、不落日志**）。
///
/// `which`：`cookie`（网页会话 Cookie，必需）/ `token`（开放平台 token，可选）。
#[tauri::command]
pub fn checkin_set_credential(
    state: State<'_, AppState>,
    id: String,
    which: String,
    secret: String,
) -> Result<SiteSummary, String> {
    checkin_set_credential_of(&state, &id, &which, &secret)
}

/// 填登录态（可测版本）。
pub fn checkin_set_credential_of(
    state: &AppState,
    id: &str,
    which: &str,
    secret: &str,
) -> Result<SiteSummary, String> {
    if secret.trim().is_empty() {
        return Err("登录态不能为空".to_string());
    }
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("站点不存在：{id}"))?;
    let mut config = crate::checkin::plugin::parse_config(&row);
    let channel_id = config
        .channel_id
        .clone()
        .ok_or_else(|| "站点没有归属渠道——请重建该站点".to_string())?;
    let kind = match which {
        "cookie" => "cookie",
        "token" => "access_token",
        other => return Err(format!("未知的登录态类型：{other}")),
    };

    let credential_id = {
        let vault = state.vault.read().map_err(|e| e.to_string())?;
        let bytes = SecretBytes::from_slice(secret.as_bytes());
        let id = vault
            .put_credential(&channel_id, kind, &bytes)
            .map_err(|e| e.to_string())?;
        // 只打印 id，明文不落日志
        println!("[checkin] 站点 {id} 的 {which} 登录态已存进保险库（明文不落日志）");
        id
    };

    match which {
        "cookie" => config.credential_id = Some(credential_id),
        _ => config.token_credential_id = Some(credential_id),
    }
    let mut updated = row.clone();
    updated.config_json = Some(config.to_json());
    state
        .store
        .with_conn(|conn| checkin::upsert_site(conn, &updated))
        .map_err(|e| e.to_string())?;
    summarize(state, &updated)
}

/// 拉起浏览器做**登录交接**（MiniMax Code 用：用户在浏览器里登录一次）。
#[tauri::command]
pub async fn checkin_plugin_login(
    state: State<'_, AppState>,
    id: String,
) -> Result<LoginSummary, String> {
    checkin_plugin_login_of(&state, &id).await
}

/// 登录交接（可测版本）。
pub async fn checkin_plugin_login_of(state: &AppState, id: &str) -> Result<LoginSummary, String> {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("站点不存在：{id}"))?;
    let plugin = crate::checkin::plugin::builtin(&row.plugin)
        .ok_or_else(|| format!("这个站点不是内置插件：{}", row.plugin))?;
    let sidecar = crate::checkin::sidecar::Sidecar::default_instance().ok_or_else(|| {
        "辅轨不可用——没找到 sidecar 脚本或 Node 运行时（开发态：在 sidecar/ 目录跑一次 `npm install`）"
            .to_string()
    })?;
    let config = crate::checkin::plugin::parse_config(&row);
    // 引擎只是为了让上下文里有 HTTP 客户端（辅轨型插件不发 HTTP 请求）
    let engine = crate::checkin::replay::ReplayEngine::new().map_err(|e| e.to_string())?;
    let ctx = crate::checkin::plugin::PluginContext {
        site: &row,
        config,
        client: engine.client(),
        vault: &state.vault,
        sidecar: Some(sidecar),
    };
    let outcome = plugin
        .login(&ctx)
        .await
        .ok_or_else(|| "这个插件不支持登录交接".to_string())?;
    Ok(LoginSummary {
        ok: outcome.ok,
        message: outcome.message,
    })
}

/// 辅轨探活（Node 运行时 + 浏览器可用性 + profile 路径）。
#[tauri::command]
pub async fn checkin_sidecar_health() -> Result<crate::checkin::sidecar::SidecarHealth, String> {
    let sidecar = crate::checkin::sidecar::Sidecar::default_instance().ok_or_else(|| {
        "辅轨不可用——没找到 sidecar 脚本或 Node 运行时（开发态：在 sidecar/ 目录跑一次 `npm install`）"
            .to_string()
    })?;
    sidecar.health().await
}

/// 某站点归属账号的积分快照。
#[tauri::command]
pub fn checkin_site_reward(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<crate::store::account::RewardSnapshot>, String> {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("站点不存在：{id}"))?;
    let config = crate::checkin::plugin::parse_config(&row);
    Ok(config
        .account_id
        .as_deref()
        .and_then(|account_id| crate::store::account::read_reward(&state.store, account_id)))
}

/// 登录交接的摘要。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginSummary {
    /// 是否登录成功。
    pub ok: bool,
    /// 人话原因。
    pub message: String,
}

/// 解析归属渠道：给 id 用它；给名字就按名字找/建。
fn resolve_channel(state: &AppState, input: &SaveTemplateInput) -> Result<String, String> {
    if let Some(channel_id) = input.channel_id.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty())
    {
        let exists = state
            .store
            .with_conn(|conn| {
                let found: Option<String> = conn
                    .query_row("SELECT id FROM channel WHERE id = ?1", [channel_id], |row| {
                        row.get(0)
                    })
                    .ok();
                Ok(found.is_some())
            })
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err(format!("归属渠道不存在: {channel_id}"));
        }
        return Ok(channel_id.to_string());
    }
    let name = input
        .channel_name
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "必须指定归属渠道（已有渠道 id 或渠道名）".to_string())?;
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    vault.ensure_channel(name).map_err(|e| e.to_string())
}

/// 站点行 → 展示摘要（含调度信息）。
fn summarize(state: &AppState, row: &SiteRow) -> Result<SiteSummary, String> {
    let template = row
        .config_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<SiteTemplate>(json).ok())
        .map(|template| TemplateSummary {
            source: template.source,
            method: template.request.method,
            url: template.request.url,
            status: template.assert.status.unwrap_or(0),
            variable_count: template.variables.len(),
            variable_names: template
                .variables
                .iter()
                .map(|v| v.name.clone())
                .collect(),
        });

    // 调度信息：下一次触发 / 最近一次尝试 / 系统兜底任务
    let now = chrono::Local::now();
    let next_run_at = row
        .schedule
        .as_deref()
        .map(str::trim)
        .filter(|expr| !expr.is_empty())
        .and_then(|expr| crate::checkin::cron::parse(expr).ok())
        .and_then(|schedule| crate::checkin::cron::next_due_at(&schedule, now))
        .map(|next| next.timestamp());
    let last_attempt_at = crate::checkin::scheduler::last_attempt_at(&state.store, &row.id);
    let last_ok = state
        .store
        .with_conn(|conn| crate::store::checkin::list_results(conn, Some(&row.id), 1))
        .ok()
        .and_then(|rows| rows.first().map(|row| row.ok));
    let task = crate::checkin::system::query(&row.id).ok();

    // 内置插件的信息（HAR 模板没有这些）
    let plugin = crate::checkin::plugin::builtin(&row.plugin);
    let config = crate::checkin::plugin::parse_config(row);
    let credential_ready = config.credential_id.is_some();
    let reward = config
        .account_id
        .as_deref()
        .and_then(|account_id| crate::store::account::read_reward(&state.store, account_id));

    Ok(SiteSummary {
        id: row.id.clone(),
        name: row.name.clone(),
        plugin: row.plugin.clone(),
        enabled: row.enabled,
        schedule: row.schedule.clone(),
        template,
        next_run_at,
        last_attempt_at,
        last_ok,
        system_task: task.as_ref().map(|task| task.name.clone()),
        system_task_registered: task.as_ref().map(|task| task.registered).unwrap_or(false),
        system_task_frequency: task.and_then(|task| task.frequency),
        system_task_error: None,
        credential_ready,
        needs_sidecar: plugin.as_ref().map(|p| p.needs_sidecar()).unwrap_or(false),
        credential_hint: plugin.as_ref().map(|p| p.credential_hint().to_string()),
        reward,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    use crate::gateway::circuit::CircuitBreaker;
    use crate::gateway::circuit_store::StoreCircuitPersistence;
    use crate::gateway::ratelimit::RateLimiter;
    use crate::checkin::har::VarTarget;
    use crate::store::Store;
    use crate::vault::Vault;

    /// 一份最小但真实的 HAR 1.2（字段名与浏览器导出一致）。
    const SAMPLE_HAR: &str = r#"{
      "log": {
        "version": "1.2",
        "creator": { "name": "test", "version": "1" },
        "entries": [
          {
            "startedDateTime": "2026-11-27T09:00:00.000Z",
            "time": 120,
            "request": {
              "method": "GET",
              "url": "https://example.invalid/api/v1/users/login/info",
              "httpVersion": "HTTP/1.1",
              "cookies": [],
              "headers": [
                { "name": "Accept", "value": "application/json" },
                { "name": "Cookie", "value": "acw_tc=abc; csrf_session=SESSION-SECRET; csrf_token=CSRF-SECRET" }
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
                { "name": "Set-Cookie", "value": "csrf_session=NEW-SECRET" }
              ],
              "content": { "size": 12, "mimeType": "application/json", "text": "{\"a\":1}" },
              "contentSize": 12
            },
            "cache": {},
            "timings": { "send": 1, "wait": 100, "receive": 19 }
          }
        ]
      }
    }"#;

    fn app_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
        let vault = Arc::new(RwLock::new(
            Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None)
                .expect("打开保险库"),
        ));
        let circuit = Arc::new(CircuitBreaker::with_persistence(Arc::new(
            StoreCircuitPersistence::new(Arc::clone(&store)),
        )));
        let state = AppState {
            gateway_addr: "127.0.0.1:8787".to_string(),
            gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
            store,
            vault,
            limiter: Arc::new(RateLimiter::new()),
            circuit,
        };
        (dir, state)
    }

    fn write_har(dir: &std::path::Path) -> String {
        let path = dir.join("sample.har");
        std::fs::write(&path, SAMPLE_HAR).expect("写 HAR 失败");
        path.to_string_lossy().to_string()
    }

    fn two_marks() -> Vec<MarkedVariable> {
        vec![
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
        ]
    }

    fn save_input(path: &str, channel_name: &str) -> SaveTemplateInput {
        SaveTemplateInput {
            path: path.to_string(),
            entry_index: 0,
            site_name: "ModelScope".to_string(),
            channel_id: None,
            channel_name: Some(channel_name.to_string()),
            variables: two_marks(),
        }
    }

    /// 查库里某渠道的凭据条数。
    fn credential_count(state: &AppState, channel_id: &str) -> usize {
        let vault = state.vault.read().expect("读保险库");
        vault
            .list_credentials(Some(channel_id))
            .expect("列凭据")
            .len()
    }

    /// 库里某渠道的 id（按名字）。
    fn channel_id_by_name(state: &AppState, name: &str) -> Option<String> {
        let vault = state.vault.read().expect("读保险库");
        vault.ensure_channel(name).ok()
    }

    /// 从保险库解出某个凭据的明文（测试用）。
    fn reveal(state: &AppState, credential_id: &str) -> Option<String> {
        let vault = state.vault.read().expect("读保险库");
        vault
            .get_credential(credential_id)
            .ok()
            .map(|secret| String::from_utf8_lossy(secret.as_bytes()).to_string())
    }

    #[test]
    fn import_masks_sensitive_values_and_drops_response_secrets() {
        let (dir, _state) = app_state();
        let path = write_har(dir.path());
        let imported = checkin_import_har_of(&path).expect("导入失败");
        assert_eq!(imported.entry_count, 1);
        assert_eq!(imported.source, "sample.har");
        let json = serde_json::to_string(&imported).expect("序列化");
        assert!(!json.contains("SESSION-SECRET"), "敏感值不得出 Rust 进程");
        assert!(!json.contains("CSRF-SECRET"));
        assert!(!json.contains("NEW-SECRET"), "响应头的 Set-Cookie 不得进导入视图");
        assert!(json.contains("acw_tc"), "acw_tc 是敏感名，键名要可见");
        assert!(json.contains("application/json"), "非敏感部分照常显示");
    }

    #[test]
    fn saving_two_variables_keeps_the_template_free_of_plaintext() {
        let (dir, state) = app_state();
        let path = write_har(dir.path());
        let summary =
            checkin_save_template_of(&state, &save_input(&path, "ModelScope·签到"))
                .expect("保存模板失败");

        assert_eq!(summary.name, "ModelScope");
        assert_eq!(summary.plugin, "har");
        assert!(!summary.enabled, "导入的站点默认关（02 §7.3）");
        let template = summary.template.expect("应有模板摘要");
        assert_eq!(template.variable_count, 3, "2 个显式 + 1 个自动补齐");
        assert_eq!(
            template.variable_names,
            vec!["cookie", "csrf", "auto_acw_tc"]
        );
        assert_eq!(template.status, 401);

        // 模板里是占位符，没有明文
        let row = state
            .store
            .with_conn(|conn| checkin::get_site(conn, &summary.id))
            .expect("读站点")
            .expect("站点应存在");
        let config = row.config_json.expect("模板 JSON");
        assert!(config.contains("{{cookie}}"));
        assert!(config.contains("{{csrf}}"));
        assert!(!config.contains("SESSION-SECRET"));
        assert!(!config.contains("CSRF-SECRET"));

        // 真实值在保险库里（密文），能解出来
        let channel_id = channel_id_by_name(&state, "ModelScope·签到").expect("应有归属渠道");
        assert_eq!(credential_count(&state, &channel_id), 3);
        let template: SiteTemplate = serde_json::from_str(&config).expect("模板应能解析");
        let ids: Vec<String> = template
            .variables
            .iter()
            .filter_map(|v| v.credential_id.clone())
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(reveal(&state, &ids[0]).as_deref(), Some("SESSION-SECRET"));
        assert_eq!(reveal(&state, &ids[1]).as_deref(), Some("CSRF-SECRET"));
    }

    #[test]
    fn vault_ciphertext_never_contains_the_plain_secret() {
        let (dir, state) = app_state();
        let path = write_har(dir.path());
        let summary =
            checkin_save_template_of(&state, &save_input(&path, "ModelScope·签到"))
                .expect("保存模板失败");
        let channel_id = channel_id_by_name(&state, "ModelScope·签到").expect("应有归属渠道");

        // 直接看 credential 表的密文列：不得出现明文片段
        let rows: Vec<Vec<u8>> = state
            .store
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT ciphertext FROM credential WHERE channel_id = ?1 ORDER BY rowid",
                )?;
                let mapped = stmt.query_map([channel_id], |row| row.get::<_, Vec<u8>>(0))?;
                let mut out = Vec::new();
                for row in mapped {
                    out.push(row?);
                }
                Ok(out)
            })
            .expect("读密文");
        assert_eq!(rows.len(), 3);
        for ciphertext in rows {
            let text = String::from_utf8_lossy(&ciphertext);
            assert!(!text.contains("SESSION-SECRET"));
            assert!(!text.contains("CSRF-SECRET"));
        }
        assert!(!summary.id.is_empty());
    }

    #[test]
    fn re_recording_the_same_site_replaces_the_template_and_credentials() {
        let (dir, state) = app_state();
        let path = write_har(dir.path());
        let first =
            checkin_save_template_of(&state, &save_input(&path, "ModelScope·签到"))
                .expect("首次保存失败");
        let second =
            checkin_save_template_of(&state, &save_input(&path, "ModelScope·签到"))
                .expect("重录保存失败");

        assert_eq!(first.id, second.id, "同名站点应复用同一行");
        let sites = checkin_list_sites_of(&state).expect("列站点失败");
        assert_eq!(sites.len(), 1);

        let channel_id = channel_id_by_name(&state, "ModelScope·签到").expect("应有归属渠道");
        assert_eq!(
            credential_count(&state, &channel_id),
            3,
            "重录后仍是三条（旧凭据要清掉，不能翻倍）"
        );
    }

    #[test]
    fn deleting_a_site_removes_its_credentials() {
        let (dir, state) = app_state();
        let path = write_har(dir.path());
        let summary =
            checkin_save_template_of(&state, &save_input(&path, "ModelScope·签到"))
                .expect("保存模板失败");
        let channel_id = channel_id_by_name(&state, "ModelScope·签到").expect("应有归属渠道");
        assert_eq!(credential_count(&state, &channel_id), 3);

        assert!(checkin_delete_site_of(&state, &summary.id).expect("删除失败"));
        assert!(checkin_list_sites_of(&state).expect("列站点失败").is_empty());
        assert_eq!(credential_count(&state, &channel_id), 0, "凭据要一起清掉");
    }

    #[test]
    fn leak_check_ignores_random_credential_ids() {
        // **回归**：泄漏自检必须只查"可能含密文的请求面"，不能查整段 JSON——
        // 整段 JSON 里有随机生成的 32 位 hex `credential_id`，短密文（如 `abc`）
        // 会偶然出现在里面 → 正常保存被**随机**拒绝（本项目实测踩到过）。
        let (dir, state) = app_state();
        let path = write_har(dir.path());
        let summary =
            checkin_save_template_of(&state, &save_input(&path, "ModelScope·签到"))
                .expect("保存模板失败");

        let row = state
            .store
            .with_conn(|conn| checkin::get_site(conn, &summary.id))
            .expect("读站点")
            .expect("站点应存在");
        let config = row.config_json.expect("模板 JSON");
        let template: SiteTemplate = serde_json::from_str(&config).expect("模板应能解析");
        let surface = har::sensitive_surface(&template);
        for variable in &template.variables {
            if let Some(id) = &variable.credential_id {
                assert!(
                    !surface.contains(id),
                    "自检范围不该包含 credential_id（否则短密文会假阳性）"
                );
            }
        }
        assert!(surface.contains("{{cookie}}"), "请求面要在自检范围内");
    }

    #[test]
    fn a_missing_channel_is_rejected() {        let (dir, state) = app_state();
        let path = write_har(dir.path());
        let mut input = save_input(&path, "任意");
        input.channel_name = None;
        input.channel_id = Some("ch-not-exist".to_string());
        assert!(
            checkin_save_template_of(&state, &input).is_err(),
            "归属渠道不存在应报错"
        );

        let mut input = save_input(&path, "任意");
        input.channel_name = None;
        assert!(
            checkin_save_template_of(&state, &input).is_err(),
            "不指定归属渠道应报错"
        );
    }

    #[test]
    fn an_existing_channel_can_be_reused() {
        let (dir, state) = app_state();
        let path = write_har(dir.path());
        state
            .store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO channel (id, name, kind, provider, base_url, created_at)
                     VALUES ('ch-ms', 'ModelScope 官方', 'api_key', 'custom', 'https://example.invalid', 0)",
                    [],
                )?;
                Ok(())
            })
            .expect("写渠道失败");

        let mut input = save_input(&path, "任意");
        input.channel_id = Some("ch-ms".to_string());
        input.channel_name = None;
        let summary = checkin_save_template_of(&state, &input).expect("保存模板失败");
        assert_eq!(summary.id.is_empty(), false);
        assert_eq!(credential_count(&state, "ch-ms"), 3, "凭据应挂在既有渠道上");
    }
}
