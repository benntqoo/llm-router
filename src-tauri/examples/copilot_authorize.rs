//! 真机验收工具：命令行完成一次 GitHub Copilot 设备授权（T20；02 §6.2）。
//!
//! **为什么需要它**：判据⑤要求「手动一次真实授权」——用户必须**自己在浏览器**输入短码，
//! 自动化代替不了（也不该代替）。但本机 UI 点不动（`SetCursorPos` 被屏蔽），
//! 于是把「发起 → 轮询 → 入库」搬到命令行：**用户只做浏览器里那一步**。
//!
//! **安全边界（AGENT.md §5.3）**：
//! - 终端**只打印短码与验证网址**——`device_code` 不出后端，也**不进终端**
//!   （它能换出 token）；
//! - 两个 token **只进保险库**，终端**不回显**，只报「已入库 + 到期时间」。
//!
//! 用法：
//!
//! ```powershell
//! cargo run --example copilot_authorize
//! ```
//!
//! 默认写**真机库**（`%APPDATA%\llm-router`）；可用 `LLMR_STORE_PATH` / `LLMR_VAULT_PATH` 覆盖。

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use llm_router_lib::commands::copilot::{
    copilot_authorize_poll_of, copilot_authorize_start_of, copilot_status_of, CHANNEL_NAME,
};
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::providers::oauth::copilot::{self, Endpoints};
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;

/// 出错时的统一出口（**不回显任何凭据**）。
fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    // ── 打开真机库与保险库 ──
    let store = match Store::open(llm_router_lib::store::default_db_path()) {
        Ok(store) => Arc::new(store),
        Err(err) => fail(format!("打开数据库失败：{err}")),
    };
    let vault = match Vault::open(
        Arc::clone(&store),
        llm_router_lib::vault::default_vault_path(),
        None,
    ) {
        Ok(vault) => Arc::new(RwLock::new(vault)),
        Err(err) => fail(format!("打开保险库失败：{err}")),
    };
    let state = AppState {
        gateway_addr: "127.0.0.1:8787".to_string(),
        gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
        store,
        vault,
        limiter: Arc::new(RateLimiter::new()),
        circuit: Arc::new(CircuitBreaker::new()),
    };

    // ── ① 发起设备授权 ──
    let endpoints = Endpoints::default();
    let view = match copilot_authorize_start_of(&state, &endpoints).await {
        Ok(view) => view,
        Err(err) => fail(format!("发起设备授权失败：{err}")),
    };

    println!("短码：{}", view.user_code);
    println!("验证页：{}", view.verification_uri);
    println!(
        "（短码 {} 分钟内有效；请在浏览器里输入短码并确认授权）",
        view.expires_in / 60
    );

    // ── ③ 轮询到终态 ──
    let deadline = Instant::now() + Duration::from_secs(view.expires_in);
    let mut interval = view.interval.max(1);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        match copilot_authorize_poll_of(&state, &view.flow_id, &endpoints).await {
            Ok(poll) => match poll.state.as_str() {
                "pending" => {}
                "slow_down" => interval = poll.interval.max(1),
                "authorized" => {
                    println!(
                        "授权成功。Copilot token 到期时刻：{}",
                        poll.expires_at.unwrap_or_default()
                    );
                    break;
                }
                other => fail(format!("授权未完成（{other}）——请重新运行本工具。")),
            },
            Err(err) => {
                // ④ 失败（例如账号没有 Copilot 订阅）：GitHub token **已经入库**，
                // 可以直接再换一次 Copilot token，不必重走浏览器。
                println!("轮询出错：{err}");
                retry_copilot_token(&state, &endpoints).await;
                break;
            }
        }
        if Instant::now() >= deadline {
            fail("设备码已过期——请重新运行本工具。".to_string());
        }
    }

    // ── 收尾：只报状态，不回显凭据 ──
    match copilot_status_of(&state) {
        Ok(status) => {
            println!("渠道：{CHANNEL_NAME}");
            println!("已授权：{}", status.authorized);
            println!(
                "渠道启用：{}（**默认关闭**——启用与 ToS 确认属 T24）",
                status.channel_enabled
            );
            println!("出站域名：{}", status.api_base.as_deref().unwrap_or("—"));
        }
        Err(err) => fail(format!("查状态失败：{err}")),
    }
    println!("两条凭据已进保险库（终端不回显）。应用打开即可看到状态。");
}

/// ④ 失败后的补救：用**已入库的 GitHub token** 再换一次 Copilot token。
async fn retry_copilot_token(state: &AppState, endpoints: &Endpoints) {
    let channel_id = match llm_router_lib::store::channels::list_channels(&state.store) {
        Ok(rows) => match rows
            .into_iter()
            .find(|row| row.provider == llm_router_lib::store::channels::OAUTH_PROVIDER)
        {
            Some(row) => row.id,
            None => fail("找不到 Copilot 渠道行。".to_string()),
        },
        Err(err) => fail(format!("列渠道失败：{err}")),
    };
    let client = match copilot::http_client() {
        Ok(client) => client,
        Err(err) => fail(format!("构造 HTTP 客户端失败：{err}")),
    };
    let vault = match state.vault.read() {
        Ok(vault) => vault,
        Err(err) => fail(format!("读保险库失败：{err}")),
    };
    match copilot::ensure_token(&client, endpoints, &vault, &channel_id, copilot::cache()).await {
        Ok(token) => println!(
            "已用入库的 GitHub token 换到 Copilot token（到期时刻 {}）",
            token.expires_at
        ),
        Err(err) => fail(format!("换取 Copilot token 失败：{err}")),
    }
}
