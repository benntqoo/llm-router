//! T4 出站适配器 —— 端到端验收测试。
//!
//! 验收判据（03 §2 T4）：
//! 1. 单测覆盖每个改写规则（R-1/R-3/R-5/R-7 必测）；
//! 2. MiniMax 出站 URL 按 02 §4.2 口径拼接。
//!
//! 本文件走**真实 `channel` 表 → 预设合并 → 出站报文**的完整链路，
//! 验证数据库里的一行渠道配置能产出正确的上游 HTTP 请求。

use axum::http::{HeaderMap, HeaderValue, Method};
use llm_router_lib::providers::adapter::{CallContext, OpenAIRequest};
use llm_router_lib::providers::{adapter_for, config_from_row, ChannelRow};
use llm_router_lib::store::Store;
use llm_router_lib::vault::crypto::SecretBytes;

/// 写入一条渠道行并回读（模拟 T8/T9 的渠道管理 + DB 往返）。
fn insert_channel(store: &Store, id: &str, name: &str, provider: &str, meta_json: Option<&str>) {
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at, meta_json)
                 VALUES (?1, ?2, 'api_key', ?3, '', 1, 100, ?4, ?5)",
                rusqlite::params![id, name, provider, now, meta_json],
            )?;
            Ok(())
        })
        .expect("写入渠道");
}

/// 回读渠道行。
fn load_channel(store: &Store, id: &str) -> ChannelRow {
    store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, kind, provider, base_url, meta_json FROM channel WHERE id = ?1",
            )?;
            let row = stmt.query_row([id], |r| {
                Ok(ChannelRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    provider: r.get(3)?,
                    base_url: r.get(4)?,
                    meta_json: r.get(5)?,
                })
            })?;
            Ok(row)
        })
        .expect("回读渠道")
}

/// 写入模型行（alias → upstream_id），返回上游模型名。
fn insert_model(store: &Store, channel_id: &str, alias: &str, upstream_id: &str) {
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                rusqlite::params![format!("m_{alias}"), channel_id, upstream_id, alias],
            )?;
            Ok(())
        })
        .expect("写入模型");
}

fn inbound_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("connection", HeaderValue::from_static("keep-alive"));
    headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
    headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
    headers.insert("content-length", HeaderValue::from_static("123"));
    headers.insert("host", HeaderValue::from_static("127.0.0.1:8787"));
    headers.insert("authorization", HeaderValue::from_static("Bearer inbound-client-key"));
    headers.insert("x-api-key", HeaderValue::from_static("llmr_inbound"));
    headers.insert("accept", HeaderValue::from_static("application/json"));
    headers
}

/// 构造出站调用（走适配器注册表）。
async fn complete(
    store: &Store,
    channel_id: &str,
    upstream_model: &str,
    body: serde_json::Value,
    stream: bool,
) -> llm_router_lib::providers::adapter::OutgoingCall {
    let row = load_channel(store, channel_id);
    let config = config_from_row(&row).expect("构造渠道配置");
    let adapter = adapter_for(&row.provider).expect("应找到适配器");
    let ctx = CallContext {
        channel: config,
        upstream_model: upstream_model.to_string(),
        credential: Some(SecretBytes::from_slice(b"sk-upstream-secret")),
    };
    adapter
        .complete(
            &ctx,
            OpenAIRequest {
                body,
                stream,
                inbound_headers: Some(inbound_headers()),
            },
        )
        .await
        .expect("构造出站调用")
}

#[tokio::test]
async fn acceptance_minimax_cn_url_follows_section_4_2() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("store.db")).expect("打开库");
    insert_channel(
        &store,
        "ch-mm-cn",
        "MiniMax 国内站",
        "minimax_cn",
        Some(r#"{"group_id":"17895xxxxxxx","supports_groupid_query":true}"#),
    );
    insert_model(&store, "ch-mm-cn", "mm-alias", "MiniMax-Text-01");

    let call = complete(
        &store,
        "ch-mm-cn",
        "MiniMax-Text-01",
        serde_json::json!({"model": "mm-alias", "messages": [{"role": "user", "content": "hi"}]}),
        false,
    )
    .await;

    // R-3 + R-5：base_url + 路径模板 + ?GroupId=
    assert_eq!(
        call.url,
        "https://api.minimaxi.com/v1/text/chatcompletion_v2?GroupId=17895xxxxxxx"
    );
    assert_eq!(call.method, Method::POST);

    // R-1：alias 换成上游真实模型名
    let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
    assert_eq!(body["model"], "MiniMax-Text-01");

    // R-2：凭证进 Authorization，入站鉴权头被替换
    assert_eq!(
        call.headers.get("authorization").expect("Authorization"),
        "Bearer sk-upstream-secret"
    );
    assert!(call.headers.get("x-api-key").is_none());

    // R-4：MiniMax 不注入 stream_options
    assert!(body.get("stream_options").is_none());

    // R-7：hop-by-hop 头被剥离，普通头透传
    for name in ["connection", "keep-alive", "transfer-encoding", "content-length", "host"] {
        assert!(call.headers.get(name).is_none(), "{name} 应被剥离");
    }
    assert_eq!(
        call.headers.get("accept").expect("Accept 应透传"),
        "application/json"
    );
    assert_eq!(
        call.headers.get("content-type").expect("Content-Type"),
        "application/json"
    );
}

#[tokio::test]
async fn acceptance_minimax_intl_url_has_no_group_id() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("store.db")).expect("打开库");
    // 国际站：即使 meta 里写了 group_id，也不拼（预设 supports_groupid_query=false）
    insert_channel(
        &store,
        "ch-mm-intl",
        "MiniMax 国际站",
        "minimax_intl",
        Some(r#"{"group_id":null}"#),
    );
    insert_model(&store, "ch-mm-intl", "mm-intl-alias", "MiniMax-Text-01");

    let call = complete(
        &store,
        "ch-mm-intl",
        "MiniMax-Text-01",
        serde_json::json!({"model": "mm-intl-alias", "messages": []}),
        false,
    )
    .await;

    assert_eq!(
        call.url,
        "https://api.minimax.io/v1/text/chatcompletion_v2"
    );
    assert!(!call.url.contains("GroupId"), "国际站 URL 不得带 GroupId");
}

#[tokio::test]
async fn acceptance_glm_non_standard_path() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("store.db")).expect("打开库");
    insert_channel(&store, "ch-glm", "GLM 官方", "glm", None);
    insert_model(&store, "ch-glm", "glm-alias", "glm-4-plus");

    let call = complete(
        &store,
        "ch-glm",
        "glm-4-plus",
        serde_json::json!({"model": "glm-alias", "messages": []}),
        false,
    )
    .await;

    assert_eq!(call.url, "https://open.bigmodel.cn/api/paas/v4/chat/completions");
}

#[tokio::test]
async fn acceptance_dashscope_compatible_mode_path() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("store.db")).expect("打开库");
    insert_channel(&store, "ch-dq", "阿里百炼", "dashscope", None);
    insert_model(&store, "ch-dq", "dq-alias", "qwen-max");

    let call = complete(
        &store,
        "ch-dq",
        "qwen-max",
        serde_json::json!({"model": "dq-alias", "messages": []}),
        true,
    )
    .await;

    assert_eq!(
        call.url,
        "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
    );
    // R-4：百炼声明支持 → 流式注入 include_usage
    let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
    assert_eq!(body["stream_options"]["include_usage"], serde_json::json!(true));
}

#[tokio::test]
async fn acceptance_group_id_never_leaks_into_body_or_headers() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("store.db")).expect("打开库");
    insert_channel(
        &store,
        "ch-mm-cn2",
        "MiniMax 国内站 2",
        "minimax_cn",
        Some(r#"{"group_id":"17895xxxxxxx","supports_groupid_query":true}"#),
    );

    let call = complete(
        &store,
        "ch-mm-cn2",
        "MiniMax-Text-01",
        serde_json::json!({"model": "mm-alias", "messages": []}),
        false,
    )
    .await;

    let body_text = String::from_utf8(call.body.clone()).expect("body 为 UTF-8");
    assert!(!body_text.contains("GroupId"), "GroupId 不得进 body");
    assert!(!body_text.contains("17895xxxxxxx"), "GroupId 不得进 body");
    for (name, value) in call.headers.iter() {
        let text = value.to_str().unwrap_or_default();
        assert!(
            !text.contains("17895xxxxxxx"),
            "GroupId 不得进 header {name}"
        );
    }
    // 只允许出现在 URL query
    assert!(call.url.contains("?GroupId=17895xxxxxxx"));
}

#[tokio::test]
async fn acceptance_outgoing_request_line_matches_target_url() {
    // 把出站调用渲染成 HTTP 请求行，确认路径与 query 与 §4.2 口径一致
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Store::open(dir.path().join("store.db")).expect("打开库");
    insert_channel(
        &store,
        "ch-mm-cn3",
        "MiniMax 国内站 3",
        "minimax_cn",
        Some(r#"{"group_id":"17895xxxxxxx"}"#),
    );

    let call = complete(
        &store,
        "ch-mm-cn3",
        "MiniMax-Text-01",
        serde_json::json!({"model": "mm-alias", "messages": []}),
        false,
    )
    .await;

    let (origin, path) = call
        .url
        .split_once("https://")
        .map(|(_, rest)| rest.split_once('/').unwrap())
        .expect("解析 URL");
    assert_eq!(origin, "api.minimaxi.com");
    assert_eq!(path, "v1/text/chatcompletion_v2?GroupId=17895xxxxxxx");

    // 请求体不得为空且必须是 JSON
    let body: serde_json::Value = serde_json::from_slice(&call.body).expect("解析 body");
    assert!(body["model"].is_string());
}
