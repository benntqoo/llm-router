//! T17 真机验收工具（02 §7.2.1）——**站点准备 / 状态检查 / 清理**。
//!
//! 配合 `checkin_mock_upstream`（另一个 example）跑完整的验收链：
//!
//! ```text
//! setup   <har> <site_name>        导入 HAR → 建站点模板（T15 链路），打印站点 id
//! seed    <site_id> <cron>         直接写 cron + enabled=1（**不碰系统兜底任务**）
//! status  <site_id>                看站点状态 / 系统兜底任务 / 最近执行结果
//! cleanup <site_id>                删站点 + 注销系统兜底任务
//! ```
//!
//! **库与保险库路径**取 `LLMR_STORE_PATH` / `LLMR_VAULT_PATH`；不给就用应用默认路径
//! （真机验收要用默认路径——**系统计划任务是独立进程，拿不到当前进程的环境变量**）。
//!
//! `seed` 刻意**不注册系统兜底任务**：验收工具自己的 exe 不是应用 exe，
//! 注册出来的计划任务动作会指向错误的可执行文件（补签测试只需要"到点该跑"，
//! 不需要任务真的触发）。

use std::sync::{Arc, RwLock};

use llm_router_lib::checkin::har::MarkedVariable;
use llm_router_lib::commands::checkin::{
    checkin_delete_site_of, checkin_list_sites_of, checkin_results_of,
    checkin_save_template_of, SaveTemplateInput,
};
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::store::checkin;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;

/// 开库 + 开保险库 + 拼 `AppState`。
fn open_state(store_path: &str, vault_path: &str) -> Result<AppState, Box<dyn std::error::Error>> {
    if let Some(parent) = std::path::Path::new(store_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let store = Arc::new(Store::open(store_path)?);
    let vault = Arc::new(RwLock::new(Vault::open(
        Arc::clone(&store),
        vault_path,
        None,
    )?));
    let circuit = Arc::new(CircuitBreaker::with_persistence(Arc::new(
        StoreCircuitPersistence::new(Arc::clone(&store)),
    )));
    Ok(AppState {
        gateway_addr: "127.0.0.1:8787".to_string(),
        gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
        store,
        vault,
        limiter: Arc::new(RateLimiter::new()),
        circuit,
    })
}

/// 打印一个站点的状态。
fn print_status(state: &AppState, site_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, site_id))?
        .ok_or_else(|| format!("站点不存在：{site_id}"))?;
    println!("站点：{}（{}）", row.name, row.id);
    println!("  enabled  = {}", row.enabled);
    println!("  schedule = {:?}", row.schedule);
    let now = chrono::Local::now();
    if let Some(expr) = row.schedule.as_deref() {
        if let Ok(schedule) = llm_router_lib::checkin::cron::parse(expr) {
            println!(
                "  下次应触发 = {:?}",
                llm_router_lib::checkin::cron::next_due_at(&schedule, now)
            );
            println!(
                "  最近应触发 = {:?}",
                llm_router_lib::checkin::cron::last_due_at(&schedule, now)
            );
        }
    }
    let task = llm_router_lib::checkin::system::query(&row.id);
    match &task {
        Ok(task) => println!(
            "  系统兜底   = {}（已注册：{}）",
            task.name, task.registered
        ),
        Err(err) => println!("  系统兜底   = 查询失败：{err}"),
    }
    println!("  系统兜底任务清单 = {:?}", llm_router_lib::checkin::system::list()?);
    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(&row.id), 20))?;
    println!("  最近执行结果 {} 条：", rows.len());
    for item in &rows {
        println!(
            "    ts={} ok={} message={}",
            item.ts,
            item.ok,
            item.message.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法：checkin_schedule_demo <setup|seed|status|cleanup> ...");
        return Err("缺少子命令".into());
    }
    let mode = args[1].as_str();
    let store_path = std::env::var("LLMR_STORE_PATH")
        .unwrap_or_else(|_| llm_router_lib::store::default_db_path().display().to_string());
    let vault_path = std::env::var("LLMR_VAULT_PATH")
        .unwrap_or_else(|_| llm_router_lib::vault::default_vault_path().display().to_string());
    println!("库：{store_path}");
    println!("保险库：{vault_path}");
    let state = open_state(&store_path, &vault_path)?;

    match mode {
        "setup" => {
            let har = args
                .get(2)
                .ok_or("用法：setup <har 路径> <站点名>")?
                .clone();
            let site_name = args.get(3).cloned().unwrap_or_else(|| "T17-验收".to_string());
            let summary = checkin_save_template_of(
                &state,
                &SaveTemplateInput {
                    path: har,
                    entry_index: 0,
                    site_name,
                    channel_id: None,
                    channel_name: Some("T17-验收·签到".to_string()),
                    variables: vec![MarkedVariable {
                        name: "csrf_session".to_string(),
                        target: llm_router_lib::checkin::har::VarTarget::HeaderPart {
                            field: "Cookie".to_string(),
                            part: "csrf_session".to_string(),
                        },
                    }],
                },
            )?;
            println!("SITE_ID={}", summary.id);
            println!("SITE_NAME={}", summary.name);
            println!(
                "变量={:?} 录制状态码={} 默认关={}",
                summary
                    .template
                    .as_ref()
                    .map(|t| t.variable_names.clone())
                    .unwrap_or_default(),
                summary.template.as_ref().map(|t| t.status).unwrap_or(0),
                summary.enabled
            );
        }
        "register" => {
            // 直接调 `system::register`（真机复现注册失败用；动作里的 exe 是**本工具**的）
            let site_id = args.get(2).ok_or("用法：register <site_id> <cron>")?.clone();
            let cron = args.get(3).ok_or("用法：register <site_id> <cron>")?.clone();
            println!("action = {:?}", llm_router_lib::checkin::system::action_for(&site_id));
            match llm_router_lib::checkin::system::register(&site_id, &cron) {
                Ok(task) => println!("注册成功：{:?}", task),
                Err(err) => {
                    println!("注册失败：{err}");
                    println!("原始字节：{:?}", err.as_bytes());
                }
            }
        }
        "seed" => {            let site_id = args.get(2).ok_or("用法：seed <site_id> <cron>")?.clone();
            let cron = args.get(3).ok_or("用法：seed <site_id> <cron>")?.clone();
            let mut row = state
                .store
                .with_conn(|conn| checkin::get_site(conn, &site_id))?
                .ok_or("站点不存在")?;
            row.schedule = Some(cron.clone());
            row.enabled = true;
            state
                .store
                .with_conn(|conn| checkin::upsert_site(conn, &row))?;
            println!("已写 cron={cron} enabled=1（**未注册系统兜底任务**）");
        }
        "status" => {
            let site_id = args.get(2).ok_or("用法：status <site_id>")?;
            print_status(&state, site_id)?;
        }
        "cleanup" => {
            let site_id = args.get(2).ok_or("用法：cleanup <site_id>")?.clone();
            let removed = checkin_delete_site_of(&state, &site_id)?;
            let task = llm_router_lib::checkin::system::unregister(&site_id)?;
            println!("站点已删：{removed}；系统兜底任务已注销：{task}");
        }
        "tidy" => {
            // 清掉验收痕迹：签到站点 / 结果 / 验收专用渠道（credential 靠 ON DELETE CASCADE 连带清）
            let mut sites = 0usize;
            let mut results = 0usize;
            let mut channels = 0usize;
            state.store.with_conn(|conn| {
                sites = conn.execute("DELETE FROM checkin_site", [])?;
                results = conn.execute("DELETE FROM checkin_result", [])?;
                channels = conn.execute(
                    "DELETE FROM channel WHERE name LIKE 'T17-%'",
                    [],
                )?;
                Ok(())
            })?;
            println!("已清：站点 {sites} 条、结果 {results} 条、渠道 {channels} 条");
        }
        "list" => {
            for site in checkin_list_sites_of(&state)? {
                println!(
                    "{}  {}  enabled={}  schedule={:?}",
                    site.id, site.name, site.enabled, site.schedule
                );
            }
            let rows = checkin_results_of(&state, None, 20)?;
            println!("全部结果 {} 条：", rows.len());
            for row in rows {
                println!(
                    "  ts={} site={} ok={} message={}",
                    row.ts,
                    row.site_id,
                    row.ok,
                    row.message.as_deref().unwrap_or("-")
                );
            }
        }
        other => return Err(format!("未知子命令：{other}").into()),
    }
    Ok(())
}
