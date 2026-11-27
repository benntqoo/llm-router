//! 调度器（T17；02 §7.2.1）。
//!
//! **一条判定，三处复用**：应用内每分钟 tick、系统计划任务唤起、CLI 一次性运行，
//! 都走 [`due_sites`] + [`run_site`]——所以**关应用期间漏掉的周期会被自动补上**，
//! 且**同一应触发周期最多跑一次**（`last_attempt >= due_at` → 跳过；去重状态落库，跨重启成立）。

use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, Local, Timelike};

use crate::checkin::cron;
use crate::checkin::har::SiteTemplate;
use crate::checkin::replay::{
    ReplayClass, ReplayEngine, ReplayOutcome, ResolveError, SecretResolver,
};
use crate::store::checkin::{self, SiteRow, PLUGIN_HAR};
use crate::store::Store;
use crate::vault::Vault;

/// tick 周期（秒）——cron 最小粒度是 1 分钟。
pub const TICK_SECS: u64 = 60;

/// tick 相对整分钟的对齐偏移（秒）——避开"整点那一秒"的边界竞争。
pub const TICK_OFFSET_SECS: u64 = 3;

/// 从保险库取凭据的解析器（T16 的 `SecretResolver` 接到真实保险库上）。
pub struct VaultResolver {
    vault: Arc<RwLock<Vault>>,
}

impl VaultResolver {
    /// 构造解析器。
    pub fn new(vault: Arc<RwLock<Vault>>) -> Self {
        VaultResolver { vault }
    }
}

impl SecretResolver for VaultResolver {
    fn resolve(&self, credential_id: &str) -> Result<String, ResolveError> {
        let vault = self
            .vault
            .read()
            .map_err(|err| ResolveError::Unavailable(err.to_string()))?;
        match vault.get_credential(credential_id) {
            Ok(secret) => Ok(String::from_utf8_lossy(secret.as_bytes()).to_string()),
            Err(_) => Err(ResolveError::NotFound),
        }
    }
}

/// 一次调度结果。
#[derive(Debug, Clone)]
pub struct SiteRun {
    pub site_id: String,
    pub site_name: String,
    /// 跑了 → `Some(outcome)`；本周期已跑过 / 不该跑 → `None`。
    pub outcome: Option<ReplayOutcome>,
    /// 没跑的原因（给 UI / 日志看）。
    pub skip_reason: Option<String>,
}

/// 站点当前该不该跑（**去重判定**，02 §7.2.1 §2）。
#[derive(Debug, Clone)]
pub enum Due {
    /// 该跑：最近一次应触发时刻是 `due_at`。
    Due { due_at: i64 },
    /// 本周期已跑过。
    AlreadyRan { due_at: i64 },
    /// 不该跑：没启用 / 没有 cron / cron 非法。
    NotScheduled { reason: String },
}

/// 单个站点的调度判定。
pub fn due_of(
    store: &Store,
    site: &SiteRow,
    now: DateTime<Local>,
) -> Due {
    if !site.enabled {
        return Due::NotScheduled {
            reason: "站点未启用（02 §7.3：站点插件默认全关）".to_string(),
        };
    }
    let expr = site.schedule.as_deref().unwrap_or("").trim();
    if expr.is_empty() {
        return Due::NotScheduled {
            reason: "没有 cron 表达式".to_string(),
        };
    }
    let schedule = match cron::parse(expr) {
        Ok(schedule) => schedule,
        Err(reason) => {
            return Due::NotScheduled {
                reason: format!("cron 表达式不合法：{reason}"),
            }
        }
    };
    let Some(due_at) = cron::last_due_at(&schedule, now) else {
        return Due::NotScheduled {
            reason: "窗口内没有应触发时刻".to_string(),
        };
    };
    let due_at_secs = due_at.timestamp();
    let last_attempt = last_attempt_at(store, &site.id);
    if let Some(last) = last_attempt {
        if last >= due_at_secs {
            return Due::AlreadyRan { due_at: due_at_secs };
        }
    }
    Due::Due { due_at: due_at_secs }
}

/// 该站点最近一次**尝试**（不论成败）的时间戳。
pub fn last_attempt_at(store: &Store, site_id: &str) -> Option<i64> {
    let rows = store
        .with_conn(|conn| checkin::list_results(conn, Some(site_id), 1))
        .ok()?;
    rows.first().map(|row| row.ts)
}

/// 该站点最近一次**成功**的时间戳。
pub fn last_success_at(store: &Store, site_id: &str) -> Option<i64> {
    let rows = store
        .with_conn(|conn| checkin::list_results(conn, Some(site_id), 200))
        .ok()?;
    rows.into_iter().find(|row| row.ok).map(|row| row.ts)
}

/// 列出**该跑**的站点（去重判定后）。
pub fn due_sites(store: &Store, now: DateTime<Local>) -> Vec<SiteRow> {
    let sites = match store.with_conn(|conn| checkin::list_sites(conn)) {
        Ok(sites) => sites,
        Err(_) => return Vec::new(),
    };
    sites
        .into_iter()
        .filter(|site| matches!(due_of(store, site, now), Due::Due { .. }))
        .collect()
}

/// 跑一个站点（**按 `plugin` 分派** → 执行 → 落 `checkin_result` → 积分回写）。
///
/// 分派口径（02 §7.4 §1）：
/// - `har` → **T16 重放引擎**（用户自建模板）；
/// - `modelscope` / `minimax_code` → **内置插件**（`checkin::sites::*`）；
/// - 其它 → `template_invalid`（**不静默跳过**：站点配置错了要让人看见）。
pub async fn run_site(
    store: &Store,
    vault: &Arc<RwLock<Vault>>,
    engine: &ReplayEngine,
    site: &SiteRow,
) -> ReplayOutcome {
    let outcome = execute(vault, engine, site).await;
    record(store, &site.id, &outcome);
    write_back_reward(store, site, &outcome);
    outcome
}

/// 按插件类型执行一次签到（**不分派到别处**）。
async fn execute(
    vault: &Arc<RwLock<Vault>>,
    engine: &ReplayEngine,
    site: &SiteRow,
) -> ReplayOutcome {
    // 1) 用户自建 HAR 模板 → T16 重放引擎
    if site.plugin == PLUGIN_HAR {
        let resolver = VaultResolver::new(Arc::clone(vault));
        return match site.config_json.as_deref() {
            Some(json) => match crate::checkin::replay::parse_template(json) {
                Ok(template) => engine.replay(&template, &resolver).await,
                Err(failure) => ReplayOutcome {
                    ok: false,
                    class: failure.class,
                    message: failure.message,
                    status: None,
                    elapsed_ms: 0,
                    retryable: false,
                    body_preview: None,
                    reward: None,
                },
            },
            None => ReplayOutcome {
                ok: false,
                class: ReplayClass::TemplateInvalid,
                message: "站点没有模板（config_json 为空）".to_string(),
                status: None,
                elapsed_ms: 0,
                retryable: false,
                body_preview: None,
                reward: None,
            },
        };
    }

    // 2) 内置站点插件 → `checkin::sites::*`
    match crate::checkin::plugin::builtin(&site.plugin) {
        Some(plugin) => {
            let config = crate::checkin::plugin::parse_config(site);
            // 辅轨**只在需要时才探测**（找不到就 `None`——插件自己给出"辅轨不可用"的失败）
            let sidecar = if plugin.needs_sidecar() {
                crate::checkin::sidecar::Sidecar::default_instance()
            } else {
                None
            };
            let ctx = crate::checkin::plugin::PluginContext {
                site,
                config,
                client: engine.client(),
                vault,
                sidecar,
            };
            plugin.run(&ctx).await
        }
        None => ReplayOutcome {
            ok: false,
            class: ReplayClass::TemplateInvalid,
            message: format!("未知的站点插件：{}", site.plugin),
            status: None,
            elapsed_ms: 0,
            retryable: false,
            body_preview: None,
            reward: None,
        },
    }
}

/// 把结果写进 `checkin_result`（`reward_json` 带本次拿到的积分）。
pub fn record(store: &Store, site_id: &str, outcome: &ReplayOutcome) {
    let reward_json = outcome.reward.as_ref().map(|reward| reward.to_json());
    let _ = store.with_conn(|conn| {
        checkin::insert_result(
            conn,
            site_id,
            now_secs(),
            outcome.ok,
            reward_json.as_deref(),
            Some(&outcome.message),
        )
    });
}

/// 积分回写 `account.quota_json`（02 §7.4 §4）。
///
/// **写回失败不回滚签到**——结果已落 `checkin_result`，回写失败只记日志。
fn write_back_reward(store: &Store, site: &SiteRow, outcome: &ReplayOutcome) {
    if !outcome.ok {
        return;
    }
    let Some(reward) = outcome.reward.as_ref() else {
        return;
    };
    let config = crate::checkin::plugin::parse_config(site);
    let Some(account_id) = config.account_id.as_deref() else {
        return;
    };
    if let Err(err) = crate::store::account::write_reward(
        store,
        account_id,
        reward.amount,
        &reward.unit,
        now_secs(),
    ) {
        println!("[checkin] {}：积分回写失败（不影响签到结果）：{err}", site.name);
    }
}

/// 当前 Unix 秒。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 跑一遍该跑的站点（**补签**也走这里）。
///
/// `only_site` 为 `Some(id)` 时只处理该站点（系统计划任务按站点注册 → 精确唤起）。
pub async fn run_due_sites(
    store: &Arc<Store>,
    vault: &Arc<RwLock<Vault>>,
    engine: &ReplayEngine,
    only_site: Option<&str>,
) -> Vec<SiteRun> {
    let now = Local::now();
    let sites = match store.with_conn(|conn| checkin::list_sites(conn)) {
        Ok(sites) => sites,
        Err(_) => return Vec::new(),
    };
    let mut runs = Vec::new();
    for site in sites {
        if let Some(only) = only_site {
            if site.id != only {
                continue;
            }
        }
        let due = due_of(store, &site, now);
        let reason = match &due {
            Due::Due { .. } => None,
            Due::AlreadyRan { due_at } => Some(format!("本周期已跑过（应触发时刻 {due_at}）")),
            Due::NotScheduled { reason } => Some(reason.clone()),
        };
        match due {
            Due::Due { .. } => {
                let outcome = run_site(store, vault, engine, &site).await;
                runs.push(SiteRun {
                    site_id: site.id.clone(),
                    site_name: site.name.clone(),
                    outcome: Some(outcome),
                    skip_reason: None,
                });
            }
            _ => runs.push(SiteRun {
                site_id: site.id.clone(),
                site_name: site.name.clone(),
                outcome: None,
                skip_reason: reason,
            }),
        }
    }
    runs
}

/// 应用内的每分钟 tick（02 §7.2.1 §1）。
///
/// **启动时先跑一遍**——这就是「错过补签」的入口：应用一开，就把关着的那段时间里
/// 漏掉的应触发周期补上（与系统计划任务唤起时同一套判定）。
pub async fn run_ticker(
    store: Arc<Store>,
    vault: Arc<RwLock<Vault>>,
    engine: ReplayEngine,
) {
    tick_once(&store, &vault, &engine).await;
    loop {
        // 对齐到整分钟 + 偏移，避免和秒边界竞争
        let now = Local::now();
        let secs_to_next_minute = 60 - now.second() as u64;
        let wait = Duration::from_secs(secs_to_next_minute + TICK_OFFSET_SECS);
        tokio::time::sleep(wait).await;
        tick_once(&store, &vault, &engine).await;
    }
}

/// 一轮 tick：跑该跑的站点并打印结果。
pub async fn tick_once(
    store: &Arc<Store>,
    vault: &Arc<RwLock<Vault>>,
    engine: &ReplayEngine,
) {
    let runs = run_due_sites(store, vault, engine, None).await;
    for run in runs {
        match run.outcome {
            Some(outcome) => println!(
                "[checkin] {}: {}（分类 {}）",
                run.site_name,
                outcome.message,
                outcome.class.as_str()
            ),
            None => {
                if let Some(reason) = run.skip_reason {
                    println!("[checkin] {}: 跳过（{reason}）", run.site_name);
                }
            }
        }
    }
}

/// 立即执行一次（**忽略去重**；UI 的「立即执行一次」按钮与手动检查用）。
pub async fn run_now(
    store: &Arc<Store>,
    vault: &Arc<RwLock<Vault>>,
    engine: &ReplayEngine,
    site_id: &str,
) -> Result<ReplayOutcome, String> {
    let site = store
        .with_conn(|conn| checkin::get_site(conn, site_id))
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("站点不存在：{site_id}"))?;
    Ok(run_site(store, vault, engine, &site).await)
}

/// 站点模板（供 IPC / CLI 读）。
pub fn template_of(store: &Store, site_id: &str) -> Option<SiteTemplate> {
    let row = store.with_conn(|conn| checkin::get_site(conn, site_id)).ok()??;
    let json = row.config_json?;
    serde_json::from_str::<SiteTemplate>(&json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::checkin::{SiteRow, PLUGIN_HAR};
    use chrono::TimeZone;

    fn store() -> Store {
        Store::open_in_memory().expect("打开内存库失败")
    }

    fn site(name: &str, schedule: Option<&str>, enabled: bool) -> SiteRow {
        SiteRow {
            id: format!("site_{name}"),
            name: name.to_string(),
            plugin: PLUGIN_HAR.to_string(),
            config_json: Some("{\"version\":1}".to_string()),
            schedule: schedule.map(|expr| expr.to_string()),
            enabled,
        }
    }

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("时间应合法")
    }

    #[test]
    fn disabled_site_is_not_scheduled() {
        let store = store();
        let row = site("off", Some("0 9 * * *"), false);
        match due_of(&store, &row, at(2026, 11, 27, 10, 0)) {
            Due::NotScheduled { reason } => assert!(reason.contains("未启用")),
            _ => panic!("停用的站点不该被调度"),
        }
    }

    #[test]
    fn site_without_cron_is_not_scheduled() {
        let store = store();
        let row = site("nocron", None, true);
        match due_of(&store, &row, at(2026, 11, 27, 10, 0)) {
            Due::NotScheduled { reason } => assert!(reason.contains("cron")),
            _ => panic!("没有 cron 的站点不该被调度"),
        }
    }

    #[test]
    fn invalid_cron_is_not_scheduled_but_reports_the_reason() {
        let store = store();
        let row = site("bad", Some("not a cron"), true);
        match due_of(&store, &row, at(2026, 11, 27, 10, 0)) {
            Due::NotScheduled { reason } => assert!(reason.contains("不合法")),
            _ => panic!("非法 cron 不该被调度"),
        }
    }

    #[test]
    fn site_is_due_after_its_scheduled_time() {
        let store = store();
        let row = site("due", Some("0 9 * * *"), true);
        let now = at(2026, 11, 27, 10, 0);
        match due_of(&store, &row, now) {
            Due::Due { due_at } => {
                assert_eq!(due_at, at(2026, 11, 27, 9, 0).timestamp());
            }
            _ => panic!("过了 09:00 就该跑"),
        }
    }

    #[test]
    fn site_is_due_again_after_missing_the_window() {
        // **错过补签**：昨天就该跑，但应用关着 → 今天打开时仍然"该跑"
        let store = store();
        let row = site("missed", Some("0 9 * * *"), true);
        let now = at(2026, 11, 27, 23, 0);
        match due_of(&store, &row, now) {
            Due::Due { due_at } => {
                assert_eq!(due_at, at(2026, 11, 27, 9, 0).timestamp());
            }
            _ => panic!("漏掉的周期应被补上"),
        }
    }

    #[test]
    fn already_ran_in_this_cycle_is_skipped() {
        let store = store();
        let row = site("ran", Some("0 9 * * *"), true);
        store
            .with_conn(|conn| {
                checkin::insert_result(
                    conn,
                    &row.id,
                    at(2026, 11, 27, 9, 5).timestamp(),
                    true,
                    None,
                    Some("签到成功"),
                )
            })
            .expect("写结果失败");
        let now = at(2026, 11, 27, 10, 0);
        match due_of(&store, &row, now) {
            Due::AlreadyRan { due_at } => {
                assert_eq!(due_at, at(2026, 11, 27, 9, 0).timestamp());
            }
            _ => panic!("本周期已跑过就不该再跑"),
        }
    }

    #[test]
    fn failed_attempt_also_counts_as_tried_in_this_cycle() {
        // 失败也落库 → 本周期不再重复打上游（避免频繁重试骚扰站点）
        let store = store();
        let row = site("failed", Some("0 9 * * *"), true);
        store
            .with_conn(|conn| {
                checkin::insert_result(
                    conn,
                    &row.id,
                    at(2026, 11, 27, 9, 5).timestamp(),
                    false,
                    None,
                    Some("网络失败"),
                )
            })
            .expect("写结果失败");
        let now = at(2026, 11, 27, 10, 0);
        assert!(
            matches!(due_of(&store, &row, now), Due::AlreadyRan { .. }),
            "失败也算试过了"
        );
    }

    #[test]
    fn last_attempt_and_success_are_readable() {
        let store = store();
        let row = site("hist", Some("0 9 * * *"), true);
        assert_eq!(last_attempt_at(&store, &row.id), None);
        assert_eq!(last_success_at(&store, &row.id), None);
        store
            .with_conn(|conn| {
                checkin::insert_result(conn, &row.id, 100, false, None, Some("失败"))?;
                checkin::insert_result(conn, &row.id, 200, true, None, Some("成功"))?;
                Ok(())
            })
            .expect("写结果失败");
        assert_eq!(last_attempt_at(&store, &row.id), Some(200));
        assert_eq!(last_success_at(&store, &row.id), Some(200));
    }

    #[test]
    fn due_sites_only_lists_enabled_sites_with_a_cron() {
        let store = store();
        let rows = vec![
            site("on", Some("0 9 * * *"), true),
            site("off", Some("0 9 * * *"), false),
            site("nocron", None, true),
        ];
        for row in &rows {
            store
                .with_conn(|conn| checkin::upsert_site(conn, row))
                .expect("写入失败");
        }
        let now = at(2026, 11, 27, 10, 0);
        let due = due_sites(&store, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "on");
    }
}
