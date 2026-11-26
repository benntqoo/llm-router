// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // T17：`--checkin-once` 一次性模式 —— **不起 UI、不起网关**，跑完补签判定就退出。
    // 系统计划任务 / macOS launchd 唤起应用时走的就是这条路径（02 §7.2.1 §4）。
    let args: Vec<String> = std::env::args().collect();
    if let Some(site) = llm_router_lib::checkin::cli::parse_args(&args) {
        let code = llm_router_lib::checkin::cli::run_once(site.as_deref());
        std::process::exit(code);
    }
    llm_router_lib::run()
}
