//! 把 02 §6.1 的预设目录导出成 JSON（供 `scripts/` 下的实测脚本使用）。
//!
//! 目的：**预设清单只有一个权威载体**（`providers::presets`），
//! Python 侧不再抄一份渠道名单（抄了必然漂移）。
//!
//! 用法：
//!     cd src-tauri && cargo run --quiet --example presets
//!
//! 输出：一行 JSON 数组，元素形如
//!     {"provider":"deepseek","name":"DeepSeek 官方","base_url":"https://api.deepseek.com", ...}

use llm_router_lib::providers::presets;

fn main() {
    let views = presets::all_views();
    println!(
        "{}",
        serde_json::to_string_pretty(&views).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    );
}
