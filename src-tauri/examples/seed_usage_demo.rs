//! 真机验收工具：种一个**临时库**（渠道 + 凭证 + client key）并起一个 **mock 上游**。
//!
//! 用途：验收「用量统计」面板（T14；02 §8.4）时，需要一份**有真实流水**的库。
//! 应用 UI 不好自动化，但**种库**可以——这里把渠道、模型、凭证、client key 一次性备好，
//! 再用 `curl` 往网关打几次请求，流水就落在同一个库里，应用打开即可看到图表。
//!
//! 用法：
//!
//! ```powershell
//! $env:LLMR_STORE_PATH  = "$env:TEMP\llmr-t14-demo\store.db"
//! $env:LLMR_VAULT_PATH = "$env:TEMP\llmr-t14-demo\vault.bin"
//! cargo run --offline --example seed_usage_demo
//! ```
//!
//! 打印出 **client key 明文**与 mock 上游地址；按提示用 `curl` 打网关即可。
//! 进程会一直挂着提供 mock 上游，按 Ctrl+C 结束。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use llm_router_lib::gateway::auth;
use llm_router_lib::store::Store;
use llm_router_lib::vault::crypto::SecretBytes;
use llm_router_lib::vault::Vault;

/// mock 上游的 chat 路径（渠道的 `path_chat`）。
const MOCK_CHAT_PATH: &str = "/v1/chat/completions";

/// mock 上游：非流式响应（带完整 usage）。
const NON_STREAM_BODY: &str = r#"{"id":"chatcmpl-demo","choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":31,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":5}}}"#;

/// mock 上游：流式响应（增量帧 + 末帧 usage）。
fn stream_chunks() -> Vec<(Duration, String)> {
    vec![
        (
            Duration::from_millis(120),
            "data: {\"id\":\"chunk-0\",\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n".to_string(),
        ),
        (
            Duration::from_millis(60),
            "data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n".to_string(),
        ),
        (
            Duration::from_millis(40),
            "data: {\"id\":\"chunk-2\",\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":9}}\n\n"
                .to_string(),
        ),
        (Duration::from_millis(10), "data: [DONE]\n\n".to_string()),
    ]
}

/// mock 上游的 chat 处理器：按请求体的 `stream` 决定流式还是非流式。
async fn chat(body: String) -> Response {
    if body.contains("\"stream\":true") {
        let stream = futures_util::stream::unfold(stream_chunks().into_iter(), |mut iter| async move {
            let (delay, text) = iter.next()?;
            tokio::time::sleep(delay).await;
            Some((Ok::<_, std::io::Error>(text), iter))
        });
        Response::builder()
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(stream))
            .expect("构造 SSE 响应")
    } else {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(NON_STREAM_BODY))
            .expect("构造 JSON 响应")
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. 路径（与应用的 env 约定一致）──
    let store_path = std::env::var("LLMR_STORE_PATH")
        .unwrap_or_else(|_| std::env::temp_dir().join("llmr-usage-demo").join("store.db").display().to_string());
    let vault_path = std::env::var("LLMR_VAULT_PATH")
        .unwrap_or_else(|_| std::env::temp_dir().join("llmr-usage-demo").join("vault.bin").display().to_string());

    // ── 2. mock 上游（先绑定，拿到端口再种库）──
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    let mock_base = format!("http://{addr}");
    let app = Router::new().route(MOCK_CHAT_PATH, post(chat));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // ── 3. 开库 + 开保险库 ──
    let store = Arc::new(Store::open(&store_path)?);
    let vault = Vault::open(Arc::clone(&store), &vault_path, None)?;

    // ── 4. 种渠道 + 模型 + 凭证 ──
    let meta = format!(
        r#"{{"path_chat":"{MOCK_CHAT_PATH}","extra_headers":{{"HTTP-Referer":"https://github.com/benntqoo/llm-router","X-Title":"LLM Router"}}}}"#
    );
    store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at, meta_json)
             VALUES ('c-demo', '演示渠道', 'api_key', 'custom', ?1, 1, 100, ?2, ?3)",
            rusqlite::params![mock_base, llm_router_lib::vault::now_secs(), meta],
        )?;
        conn.execute(
            "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
             VALUES ('m-demo', 'c-demo', 'demo-model-1', 'demo-chat', 1)",
            [],
        )?;
        Ok(())
    })?;
    vault.put_credential(
        "c-demo",
        "api_key",
        &SecretBytes::from_slice(b"sk-demo-local-key"),
    )?;

    // ── 5. client key（只存哈希 + 前缀，明文打印一次）──
    let client_key = auth::generate_client_key();
    let hash = auth::hash_client_key(&client_key);
    let prefix = auth::key_prefix(&client_key);
    store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
             VALUES ('ck-demo', 'demo-client', ?1, ?2, ?3)",
            rusqlite::params![hash, prefix, llm_router_lib::vault::now_secs()],
        )?;
        Ok(())
    })?;

    // ── 6. 打印用法 ──
    println!("[demo] mock 上游：{mock_base}{MOCK_CHAT_PATH}");
    println!("[demo] 库：{store_path}");
    println!("[demo] 保险库：{vault_path}");
    println!("[demo] client key（明文）：{client_key}");
    println!();
    println!("[demo] 打网关（非流式）：");
    println!(
        "  curl -s http://127.0.0.1:18787/v1/chat/completions -H \"Authorization: Bearer {client_key}\" -H \"Content-Type: application/json\" -d '{{\"model\":\"demo-chat\",\"messages\":[{{\"role\":\"user\",\"content\":\"hi\"}}]}}'"
    );
    println!("[demo] 打网关（流式，看 TTFT）：");
    println!(
        "  curl -s -N http://127.0.0.1:18787/v1/chat/completions -H \"Authorization: Bearer {client_key}\" -H \"Content-Type: application/json\" -d '{{\"model\":\"demo-chat\",\"messages\":[{{\"role\":\"user\",\"content\":\"hi\"}}],\"stream\":true}}'"
    );
    println!();
    println!("[demo] mock 上游运行中，Ctrl+C 结束。");

    // 常驻提供服务（tokio 未开 signal 特性，用 pending 挂住）
    std::future::pending::<()>().await;
    Ok(())
}
