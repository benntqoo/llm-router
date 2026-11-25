//! LLM Router — 本地 AI 网关桌面应用。
//!
//! 架构与红线依据 `docs/planning/02-technical-design.md` 与仓库根 `AGENT.md`。
//! v1 入站仅 OpenAI 协议（`POST /v1/chat/completions` + `GET /v1/models`）。

pub mod commands;
pub mod gateway;
pub mod store;
pub mod vault;

use std::sync::{Arc, RwLock};

use tauri::Manager;

use crate::gateway::ratelimit::RateLimiter;
use crate::gateway::server::{self, GatewayState};
use crate::store::Store;
use crate::vault::Vault;

/// 应用级共享状态。后续任务（渠道/保险库/用量）在此扩展。
#[derive(Clone, Debug)]
pub struct AppState {
    /// 网关实际监听地址（绑定后回填）
    pub gateway_addr: String,
    /// 供客户端填入的 base_url，形如 `http://127.0.0.1:8787/v1`
    pub gateway_base_url: String,
    /// 数据层句柄（SQLite + 迁移）
    pub store: Arc<Store>,
    /// 加密保险库（写锁用于主密钥轮换与解锁）
    pub vault: Arc<RwLock<Vault>>,
    /// 限流器（与网关共享同一实例）
    pub limiter: Arc<RateLimiter>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 数据层：空库自动建表（迁移到最新版本）；失败即启动失败。
            let db_path = store::default_db_path();
            let store = Arc::new(Store::open(&db_path).map_err(std::io::Error::other)?);
            println!(
                "[store] 数据库 {}（schema v{}）",
                db_path.display(),
                store.version().map_err(std::io::Error::other)?
            );

            // 加密保险库：首次启动自动创建（OS 原生保护主密钥）
            let vault_path = vault::default_vault_path();
            let vault = Vault::open(Arc::clone(&store), &vault_path, None)
                .map_err(std::io::Error::other)?;
            println!(
                "[vault] 保险库 {}（主密钥 v{}{}）",
                vault_path.display(),
                vault.key_version(),
                if vault.is_locked() { "，待解锁" } else { "" }
            );

            // 绑定策略（AGENT.md §5-4）：非回环地址必须已有有效 client key。
            let addr = server::resolve_bind_addr().map_err(std::io::Error::other)?;
            server::check_bind_policy(addr, server::has_client_key(store.as_ref()))
                .map_err(std::io::Error::other)?;

            // 绑定失败直接让应用启动失败：宁可开不起来，也不要出现"网关没起来但 UI 显示正常"。
            let listener = tauri::async_runtime::block_on(tokio::net::TcpListener::bind(addr))
                .map_err(std::io::Error::other)?;
            let local_addr = listener.local_addr().map_err(std::io::Error::other)?;

            let limiter = Arc::new(RateLimiter::new());
            let gateway_state = GatewayState {
                store: Arc::clone(&store),
                limiter: Arc::clone(&limiter),
            };
            let router = server::router(gateway_state.clone());

            tauri::async_runtime::spawn(async move {
                if let Err(err) = axum::serve(listener, router).await {
                    eprintln!("[gateway] 服务异常退出: {err}");
                }
            });

            println!("[gateway] 监听 http://{local_addr}/v1（OpenAI 兼容入站）");

            app.manage(gateway_state);
            app.manage(AppState {
                gateway_addr: local_addr.to_string(),
                gateway_base_url: format!("http://{local_addr}/v1"),
                store: Arc::clone(&store),
                vault: Arc::new(RwLock::new(vault)),
                limiter,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::gateway::gateway_status,
            commands::vault::vault_status,
            commands::vault::vault_put_secret,
            commands::vault::vault_reveal_secret,
            commands::vault::vault_list_secrets,
            commands::vault::vault_delete_secret,
            commands::vault::vault_rotate_master_key,
            commands::vault::vault_unlock,
            commands::vault::vault_enable_password_mode,
            commands::vault::vault_disable_password_mode,
            commands::vault::vault_export_plaintext,
            commands::vault::vault_export_encrypted_backup,
            commands::client_key::client_key_create,
            commands::client_key::client_key_list,
            commands::client_key::client_key_update_limits,
            commands::client_key::client_key_delete,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
