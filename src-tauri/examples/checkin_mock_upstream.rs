//! T17 真机验收用的 **mock 上游**（02 §7.2.1）。
//!
//! 签到请求打到这里：每次收到请求就往日志文件追加一行 JSON
//! （时间 / 方法 / URL / Cookie），并回 **200 + `{"data":{"ok":true}}`**。
//!
//! 为什么要单独一个进程：**验收「关闭应用后计划任务仍触发」时，**
//! 计划任务唤起的 `llm-router.exe --checkin-once` 是**另一个进程**，
//! mock 必须一直活着，才能证明「请求真的发出去了」。
//!
//! 用法：
//!
//! ```powershell
//! $env:MOCK_PORT = "18731"
//! $env:MOCK_LOG  = "$env:TEMP\llmr-t17\mock.log"
//! cargo run --example checkin_mock_upstream
//! ```
//!
//! 端口与日志路径都可由环境变量指定；日志文件**追加**写（不覆盖），便于核对请求数。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::var("MOCK_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(18731);
    let log_path = std::env::var("MOCK_LOG").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("llmr-t17")
            .join("mock.log")
            .display()
            .to_string()
    });
    if let Some(parent) = std::path::Path::new(&log_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let log = log_path.clone();
    let app = Router::new().fallback(any(move |request: axum::http::Request<axum::body::Body>| {
        let counter = Arc::clone(&counter);
        let log = log.clone();
        async move {
            let index = counter.fetch_add(1, Ordering::SeqCst);
            let cookie = request
                .headers()
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let line = format!(
                "{{\"seq\":{},\"ts\":{},\"method\":\"{}\",\"uri\":\"{}\",\"cookie\":\"{}\"}}\n",
                index,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                request.method(),
                request.uri(),
                cookie.replace('\\', "\\\\").replace('"', "\\\"")
            );
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log)
            {
                let _ = file.write_all(line.as_bytes());
            }
            println!("[mock] #{index} {} {}", request.method(), request.uri());
            (
                StatusCode::OK,
                "{\"data\":{\"ok\":true,\"credits\":400},\"message\":\"签到成功\"}",
            )
                .into_response()
        }
    }));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("[mock] 监听 http://127.0.0.1:{port}/（日志 {log_path}）");
    axum::serve(listener, app).await?;
    Ok(())
}
