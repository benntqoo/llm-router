//! T3 网关 —— 端到端验收测试与联调辅助。
//!
//! 验收判据（03 §2 T3）：无 key 返回 401 统一结构；`/v1/models` 返回空别名列表。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::store::Store;
use tower::ServiceExt;

fn temp_state() -> (tempfile::TempDir, GatewayState) {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let state = GatewayState {
        store,
        limiter: Arc::new(RateLimiter::new()),
    };
    (dir, state)
}

fn create_client_key(state: &GatewayState, label: &str) -> String {
    let key = auth::generate_client_key();
    let hash = auth::hash_client_key(&key);
    let prefix = auth::key_prefix(&key);
    let now = llm_router_lib::vault::now_secs();
    state
        .store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["ck_e2e", label, hash, prefix, now],
            )?;
            Ok(())
        })
        .expect("写入 client key");
    key
}

#[tokio::test]
async fn acceptance_without_key_returns_401_unified_structure() {
    let (_dir, state) = temp_state();
    let response = server::router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("host", "127.0.0.1:8787")
                .body(Body::from(r#"{"model":"deepseek-chat"}"#))
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
    assert_eq!(value["error"]["type"], "authentication_error");
    assert!(value["error"]["message"].is_string());
    assert!(value["error"]["retried"].is_array());
}

#[tokio::test]
async fn acceptance_models_returns_empty_alias_list() {
    let (_dir, state) = temp_state();
    let key = create_client_key(&state, "e2e");
    let response = server::router(state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/models")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::empty())
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
    assert_eq!(value["object"], "list");
    assert!(value["data"].as_array().expect("data 应为数组").is_empty());
}

#[tokio::test]
async fn acceptance_valid_key_passes_auth_and_reaches_routing_stage() {
    let (_dir, state) = temp_state();
    let key = create_client_key(&state, "e2e");
    let response = server::router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::from(r#"{"model":"deepseek-chat","messages":[]}"#))
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");

    // 鉴权通过；出站适配在 T4/T5 落地 → 503 + 统一错误结构
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("应为 JSON");
    assert_eq!(value["error"]["type"], "api_error");
}

#[tokio::test]
async fn acceptance_client_key_hash_is_stored_not_plaintext() {
    let (_dir, state) = temp_state();
    let key = create_client_key(&state, "e2e");
    let stored: Vec<String> = state
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT key_hash FROM client_key")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .expect("读取哈希");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], auth::hash_client_key(&key));
    assert_ne!(stored[0], key, "不得存明文");
    assert!(!key.contains(&stored[0]));
}

/// 联调辅助：在指定目录种一个 client key 并打印明文。
///
/// 用法：
/// ```text
/// LLMR_SEED_DIR=<目录> cargo test --test gateway_e2e -- --ignored seed_client_key --nocapture
/// ```
#[test]
#[ignore]
fn seed_client_key() {
    let dir = std::env::var("LLMR_SEED_DIR").expect("需要 LLMR_SEED_DIR 环境变量");
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("创建目录");
    let store = Arc::new(Store::open(dir.join("store.db")).expect("打开库"));
    let state = GatewayState {
        store,
        limiter: Arc::new(RateLimiter::new()),
    };
    let key = create_client_key(&state, "seeder");
    println!("CLIENT_KEY={key}");
}
