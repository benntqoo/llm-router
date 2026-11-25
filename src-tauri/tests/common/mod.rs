//! T6 测试共用夹具。
//!
//! 说明：端到端用例里"上游不可用"的场景**不用**黑洞地址（如 127.0.0.1:9）——
//! 本机防火墙会把这类连接黑洞化，导致每次等待 30s 连接超时；
//! 改用本机 mock 立即返回 500，行为确定且快。

#![allow(dead_code)]

use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;

/// 启动本机 mock 上游（后台任务），返回 base_url（形如 `http://127.0.0.1:PORT`）。
pub async fn start_mock(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定 mock 端口");
    let addr = listener.local_addr().expect("读取 mock 地址");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// 恒返回 500 的 mock 上游。
pub fn mock_always_500() -> Router {
    Router::new().fallback(any(|| async {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "upstream boom",
        )
            .into_response()
    }))
}
