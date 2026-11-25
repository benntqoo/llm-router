//! LLM Router — 本地 AI 网关桌面应用。
//!
//! 架构与红线依据 `docs/planning/02-technical-design.md` 与仓库根 `AGENT.md`。
//! v1 入站仅 OpenAI 协议（`POST /v1/chat/completions` + `GET /v1/models`，见任务 T3）；
//! T0 阶段只起最小骨架：内嵌 axum 服务 + `/healthz`。

pub mod commands;
pub mod gateway;

use tauri::Manager;

use crate::gateway::server;

/// 应用级共享状态。后续任务（渠道/保险库/用量）在此扩展。
#[derive(Clone, Debug)]
pub struct AppState {
    /// 网关实际监听地址（绑定后回填）
    pub gateway_addr: String,
    /// 供客户端填入的 base_url，形如 `http://127.0.0.1:8787/v1`
    pub gateway_base_url: String,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 绑定失败直接让应用启动失败：宁可开不起来，也不要出现"网关没起来但 UI 显示正常"。
            let addr = server::resolve_bind_addr().map_err(std::io::Error::other)?;
            let listener = tauri::async_runtime::block_on(tokio::net::TcpListener::bind(addr))
                .map_err(std::io::Error::other)?;
            let local_addr = listener.local_addr().map_err(std::io::Error::other)?;
            let router = server::router();

            tauri::async_runtime::spawn(async move {
                if let Err(err) = axum::serve(listener, router).await {
                    eprintln!("[gateway] 服务异常退出: {err}");
                }
            });

            println!(
                "[gateway] 监听 http://{local_addr}（OpenAI 兼容入口将在 T3 接入）"
            );

            app.manage(AppState {
                gateway_addr: local_addr.to_string(),
                gateway_base_url: format!("http://{local_addr}/v1"),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::gateway::gateway_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
