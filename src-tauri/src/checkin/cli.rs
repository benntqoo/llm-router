//! `--checkin-once` 一次性运行入口（T17；02 §7.2.1 §4）。
//!
//! **系统计划任务 / launchd 唤起应用时走的就是这条路径**：
//! 不起 UI、不起网关，跑完补签判定就退出。
//!
//! ```text
//! llm-router.exe --checkin-once             # 全部启用站点（按去重判定 → 自动补签）
//! llm-router.exe --checkin-once --site <id> # 只处理某个站点
//! ```

use std::sync::{Arc, RwLock};

use crate::checkin::replay::ReplayEngine;
use crate::checkin::scheduler;
use crate::store::Store;
use crate::vault::Vault;

/// 退出码：全部成功（或无事可做）。
pub const EXIT_OK: i32 = 0;

/// 退出码：有站点跑失败了。
pub const EXIT_FAILED: i32 = 1;

/// 退出码：环境或参数有问题（库打不开 / 保险库锁着）。
pub const EXIT_ENV: i32 = 2;

/// 解析命令行参数。
///
/// 返回 `None` = **不是**一次性运行模式；`Some(site)` = 一次性运行，`site` 为 `None` 表示全部站点。
pub fn parse_args(args: &[String]) -> Option<Option<String>> {
    if !args.iter().any(|arg| arg == "--checkin-once") {
        return None;
    }
    let mut site = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--site" {
            site = iter.next().cloned();
            break;
        }
        if let Some(value) = arg.strip_prefix("--site=") {
            site = Some(value.to_string());
            break;
        }
    }
    Some(site)
}

/// 跑一次（补签判定 + 指定站点），返回进程退出码。
pub fn run_once(site_id: Option<&str>) -> i32 {
    let store_path = crate::store::default_db_path();
    let vault_path = crate::vault::default_vault_path();
    println!("[checkin] 一次性模式：库 {}", store_path.display());

    let store = match Store::open(&store_path) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            eprintln!("[checkin] 打开库失败：{err}");
            return EXIT_ENV;
        }
    };
    let vault = match Vault::open(Arc::clone(&store), &vault_path, None) {
        Ok(vault) => Arc::new(RwLock::new(vault)),
        Err(err) => {
            eprintln!("[checkin] 打开保险库失败：{err}");
            return EXIT_ENV;
        }
    };
    if vault.read().map(|guard| guard.is_locked()).unwrap_or(false) {
        eprintln!("[checkin] 保险库处于锁定状态，退出");
        return EXIT_ENV;
    }
    let engine = match ReplayEngine::new() {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("[checkin] 构造重放引擎失败：{err}");
            return EXIT_ENV;
        }
    };

    // 同步阻塞等引擎跑完（这个进程没有 tokio 主循环）
    let runs = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("建运行时失败：{err}"))
        .and_then(|runtime| {
            Ok(runtime.block_on(scheduler::run_due_sites(
                &store,
                &vault,
                &engine,
                site_id,
            )))
        });
    let runs = match runs {
        Ok(runs) => runs,
        Err(err) => {
            eprintln!("[checkin] {err}");
            return EXIT_ENV;
        }
    };

    let mut failed = 0;
    let mut ran = 0;
    for run in runs {
        match run.outcome {
            Some(outcome) => {
                ran += 1;
                if outcome.ok {
                    println!("[checkin] {}：{}", run.site_name, outcome.message);
                } else {
                    failed += 1;
                    println!(
                        "[checkin] {}：{}（分类 {}）",
                        run.site_name,
                        outcome.message,
                        outcome.class.as_str()
                    );
                }
            }
            None => {
                if let Some(reason) = run.skip_reason {
                    println!("[checkin] {}：跳过（{reason}）", run.site_name);
                }
            }
        }
    }
    println!("[checkin] 本次跑了 {ran} 个站点，失败 {failed} 个");
    if failed > 0 {
        EXIT_FAILED
    } else {
        EXIT_OK
    }
}
