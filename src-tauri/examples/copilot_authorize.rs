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

/// 连续网络失败多少次就放弃（每轮约 5~15 s，15 分钟的设备码寿命内够扛一阵子）。
const MAX_NET_FAILURES: u32 = 40;

/// 签发设备码最多试几次（本机到 GitHub 的出口会轮换，撞到不通的节点是常态）。
const MAX_START_ATTEMPTS: u32 = 12;

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
    // 本机到 github.com 的出口会轮换，撞到不通的节点很常见 → **签发失败就再来一次**
    // （签发很便宜：失败的那枚短码自然作废，不会留下任何东西）。
    let endpoints = Endpoints::default();
    let mut view = None;
    for attempt in 1..=MAX_START_ATTEMPTS {
        match copilot_authorize_start_of(&state, &endpoints).await {
            Ok(started) => {
                view = Some(started);
                break;
            }
            Err(err) => {
                eprintln!("发起设备授权失败（第 {attempt}/{MAX_START_ATTEMPTS} 次）：{err}");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
    let Some(view) = view else {
        fail(format!(
            "连续 {MAX_START_ATTEMPTS} 次都连不上 GitHub——请检查网络后重试。"
        ));
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
    // 连续网络失败的次数上限（本机到 github.com 的出口会轮换，撞到不通的节点是常态）
    let mut net_failures = 0u32;
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if Instant::now() >= deadline {
            fail("设备码已过期——请重新运行本工具。".to_string());
        }
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
                // **网络层失败 ≠ 授权失败** —— 两者靠「GitHub token 有没有落库」区分：
                // 落了 → 是 ④ 换取 Copilot token 失败（token 已在手，换一次即可）；
                // 没落 → 纯网络失败，轮询是幂等的、设备码还有寿命，**继续等**。
                let gh_landed = copilot_status_of(&state)
                    .map(|status| status.authorized)
                    .unwrap_or(false);
                if gh_landed {
                    println!("已拿到 GitHub token，但换取 Copilot token 失败：{err}");
                    retry_copilot_token(&state, &endpoints).await;
                    break;
                }
                net_failures += 1;
                println!("轮询暂时不通（第 {net_failures} 次，继续等）：{err}");
                if net_failures >= MAX_NET_FAILURES {
                    fail(format!(
                        "连续 {MAX_NET_FAILURES} 次连不上 GitHub——请检查网络后重新运行本工具。\n\
                         （若浏览器里已经点过「授权」，设备码过期前重跑本工具即可续上）"
                    ));
                }
                continue;
            }
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
    let vault = match state.vault.read() {
        Ok(vault) => vault,
        Err(err) => fail(format!("读保险库失败：{err}")),
    };
    match copilot::ensure_token(endpoints, &vault, &channel_id, copilot::cache()).await {
        Ok(token) => println!(
            "已用入库的 GitHub token 换到 Copilot token（到期时刻 {}）",
            token.expires_at
        ),
        Err(err) => fail(format!("换取 Copilot token 失败：{err}")),
    }
}
