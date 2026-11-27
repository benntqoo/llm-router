//! 真机验收工具：Copilot 出站适配（T21；02 §6.2.2）。
//!
//! 做五件事（**全程真实上游**）：
//! ① 报告 Copilot 渠道状态（出站域名 / 是否启用 / 已有模型数）；
//! ② 确认真实的 Copilot token（**不回显**）并打印到期时刻；
//! ③ **真实拉一次模型目录**（`GET {api}/models`）→ 打印收录结果并落库；
//! ④ 打印**真实构造出来的出站请求**（URL + R-6 头；`Authorization` 打码）；
//! ⑤ **经真网关发一次流式对话**（渠道**临时打开** → 测完**恢复原状**）。
//!
//! **安全边界（AGENT.md §5.3）**：终端**不回显任何 token**——只打印模型目录、
//! 出站头（Authorization 打码）与对话内容。
//!
//! **为什么临时打开渠道**：启用开关与 ToS 确认弹窗属 **T24**（本卡不提供产品入口），
//! 而「经网关流式对话成功」这条判据要求渠道是通的 → 这里**直接改库**打开、测完**改回**。
//!
//! 用法：
//!
//! ```powershell
//! cargo run --example copilot_outbound
//! ```
//!
//! 默认写**真机库**（`%APPDATA%\llm-router`）；可用 `LLMR_STORE_PATH` / `LLMR_VAULT_PATH` 覆盖。

use std::sync::{Arc, RwLock};

use axum::http::HeaderMap;
use http_body_util::BodyExt;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::fallback::Forwarder;
use llm_router_lib::gateway::inbound::openai::parse_chat_request;
use llm_router_lib::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
use llm_router_lib::gateway::router::AliasRouter;
use llm_router_lib::gateway::usage::CharRatioEstimator;
use llm_router_lib::providers::adapter::{CallContext, OpenAIRequest, ProviderAdapter};
use llm_router_lib::providers::copilot::{self, CopilotSession};
use llm_router_lib::providers::oauth::copilot as oauth_copilot;
use llm_router_lib::providers::{config_from_row, AdapterDeps, ChannelRow};
use llm_router_lib::store::channels::{sync_models_from_catalog, CatalogModel, OAUTH_PROVIDER};
use llm_router_lib::store::usage::StoreUsageSink;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;

/// 出错时的统一出口（**不回显任何凭据**）。
fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

/// 回读渠道行（拿到 `meta_json` 原文）。
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
        .unwrap_or_else(|err| fail(format!("回读渠道失败：{err}")))
}

/// 渠道是否启用。
fn channel_enabled(store: &Store, id: &str) -> bool {
    store
        .with_conn(|conn| {
            let value: i64 = conn.query_row(
                "SELECT enabled FROM channel WHERE id = ?1",
                [id],
                |row| row.get(0),
            )?;
            Ok(value != 0)
        })
        .unwrap_or_else(|err| fail(format!("查渠道启用状态失败：{err}")))
}

/// 设置渠道启用状态。
fn set_enabled(store: &Store, id: &str, enabled: bool) {
    store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE channel SET enabled = ?2 WHERE id = ?1",
                rusqlite::params![id, if enabled { 1 } else { 0 }],
            )?;
            Ok(())
        })
        .unwrap_or_else(|err| fail(format!("设置渠道启用状态失败：{err}")));
}

/// 渠道启用的**恢复守卫**。
///
/// **为什么需要它**：真机验收会把渠道**临时打开**（启用开关与 ToS 弹窗属 T24），
/// 测完必须**改回原状**——否则用户的渠道就停在"已启用"（违反 AGENT.md §5.5
/// 「反代默认关闭」）。用 `Drop` 兜底：**任何**退出路径（含 panic）都会还原。
///
/// （注意：`std::process::exit` **不跑析构** → 所以本文件里 `exit` 只出现在
/// `run()` 返回**之后**，守卫那时已经 Drop 过了。）
struct EnabledGuard<'a> {
    /// 数据层。
    store: &'a Store,
    /// 渠道 id。
    channel_id: &'a str,
    /// 原状态（`true` = 本来就是开的 → 不用还原）。
    original: bool,
    /// 本次是否真的动过。
    touched: bool,
}

impl Drop for EnabledGuard<'_> {
    fn drop(&mut self) {
        if self.touched && !self.original {
            let _ = set_enabled(self.store, self.channel_id, false);
        }
    }
}

#[tokio::main]
async fn main() {
    let code = run().await;
    std::process::exit(code);
}

/// 真机验收主体（返回进程退出码）。
async fn run() -> i32 {
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
    let session = CopilotSession::production(Arc::clone(&vault));

    // ── ① 渠道状态 ──
    let channels = llm_router_lib::store::channels::list_channels(&store)
        .unwrap_or_else(|err| fail(format!("列渠道失败：{err}")));
    let Some(channel) = channels
        .into_iter()
        .find(|row| row.provider == OAUTH_PROVIDER)
    else {
        fail("真机库里没有 Copilot 渠道——先跑 `cargo run --example copilot_authorize` 做一次设备授权。");
    };
    let channel_id = channel.id.clone();
    println!("① Copilot 渠道");
    println!("   id：{channel_id}");
    println!("   出站域名：{}", channel.base_url);
    println!("   已启用：{}", channel.enabled);
    println!("   已有模型数：{}", channel.models.len());

    // ── ② 确认真实 token（**不回显**）──
    let token = oauth_copilot::ensure_token(
        &session.endpoints,
        &session.vault,
        &channel_id,
        &session.cache,
    )
    .await
    .unwrap_or_else(|err| fail(format!("取 Copilot token 失败：{err}")));
    println!("② Copilot token 已就绪（**不回显**）");
    println!("   到期时刻（Unix 秒）：{}", token.expires_at);
    println!("   本次用的出站域名：{}", token.api_base);

    // ── ③ 真实模型目录 ──
    println!("③ 模型目录（真实上游）");
    let raw = copilot::fetch_models_raw(&session, &channel_id)
        .await
        .unwrap_or_else(|err| fail(format!("拉模型目录失败：{err}")));
    // **原始字段**（排障用：核对上游到底给了哪些字段——决定收录口径）
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
        if let Some(data) = value.get("data").and_then(|item| item.as_array()) {
            println!("   原始条目共 {} 个；逐条关键字段：", data.len());
            for item in data.iter().take(60) {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let kind = item
                    .get("capabilities")
                    .and_then(|c| c.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let endpoints = item
                    .get("supported_endpoints")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .unwrap_or_else(|| "-".to_string());
                let policy = item
                    .get("policy")
                    .and_then(|p| p.get("state"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                println!("   - {id}｜type={kind}｜endpoints={endpoints}｜policy={policy}");
            }
        }
    }
    let models = copilot::parse_models(&raw)
        .unwrap_or_else(|err| fail(format!("解析模型目录失败：{err}")));
    println!("   收录 {} 个模型（只收 chat 端点 + 策略启用的）", models.len());
    for model in models.iter().take(12) {
        println!(
            "   - {}（{}）上下文 {:?} / 工具调用 {:?}",
            model.id, model.name, model.context_len, model.supports_tool_calls
        );
    }
    let entries: Vec<CatalogModel> = models
        .iter()
        .map(|model| CatalogModel {
            alias: model.id.clone(),
            upstream_id: model.id.clone(),
            context_len: model.context_len,
        })
        .collect();
    let report = sync_models_from_catalog(&store, &channel_id, &entries)
        .unwrap_or_else(|err| fail(format!("模型目录落库失败：{err}")));
    println!(
        "   落库：新增 {} / 更新 {} / 禁用 {} / 跳过 {:?}",
        report.added.len(),
        report.updated.len(),
        report.disabled.len(),
        report.skipped
    );

    // ── ④ 真实出站请求（Authorization 打码）──
    // 演示模型：优先 `gpt-4o` 家族（真机实测能正常对话），否则取目录第一个
    let first_model = models
        .iter()
        .find(|model| model.id.starts_with("gpt-4o"))
        .or_else(|| models.first())
        .unwrap_or_else(|| fail("模型目录为空——无法继续。"));
    let row = load_channel(&store, &channel_id);
    let config = config_from_row(&row).unwrap_or_else(|err| fail(format!("构造渠道配置失败：{err}")));
    let adapter = copilot::CopilotAdapter::new(session.clone());
    let call = adapter
        .complete(
            &CallContext {
                channel: config,
                upstream_model: first_model.id.clone(),
                credential: None,
            },
            OpenAIRequest {
                body: serde_json::json!({
                    "model": first_model.id,
                    "messages": [{"role": "user", "content": "hi"}],
                    "stream": true
                }),
                stream: true,
                inbound_headers: None,
            },
        )
        .await
        .unwrap_or_else(|err| fail(format!("构造出站请求失败：{err}")));
    println!("④ 出站请求（真实构造）");
    println!("   URL：{}", call.url);
    for (name, value) in call.headers.iter() {
        let text = value.to_str().unwrap_or_default();
        if name.as_str() == "authorization" {
            println!("   {name}: Bearer ***（已打码）");
        } else {
            println!("   {name}: {text}");
        }
    }

    // ── ⑤ 经真网关发一次流式对话 ──
    let alias = first_model.id.clone();
    let originally_enabled = channel_enabled(&store, &channel_id);
    // 守卫：任何退出路径都会把渠道**改回原状**
    let mut guard = EnabledGuard {
        store: &store,
        channel_id: &channel_id,
        original: originally_enabled,
        touched: false,
    };
    if !originally_enabled {
        println!("⑤ 临时打开渠道做真机对话（测完恢复原状；启用开关属 T24）");
        set_enabled(&store, &channel_id, true);
        guard.touched = true;
    }
    let result = run_chat(&store, &session, &alias, true).await;
    match result {
        Ok((frames, content, _raw)) => {
            println!("⑤ 经网关流式对话：**成功**");
            println!("   别名：{alias}");
            println!("   SSE 帧数：{frames}");
            println!("   上游回话：{}", truncate(content.trim(), 300));
            // 落库证据：`usage_log` 里应有一行这条请求
            if let Some(row) = last_usage(&store, &channel_id) {
                println!("   usage_log：{row}");
            }
        }
        Err(err) => {
            println!("⑤ 经网关流式对话：**失败**");
            println!("   {err}");
            // **不**在这里 `process::exit`——守卫还没 Drop，渠道会停在开启态。
            // 返回退出码即可：`run()` 返回时守卫先 Drop（还原渠道），再由 `main` 退出。
            return 1;
        }
    }

    // ── ⑤' 非流式一次（实测矩阵第 8 列的「非流式 chat」格）──
    match run_chat(&store, &session, &alias, false).await {
        Ok((_frames, content, raw)) => {
            println!("⑤' 经网关非流式对话：**成功**");
            println!("   上游回话：{}", truncate(content.trim(), 200));
            println!("   回包片段：{}", truncate(raw.trim(), 200));
        }
        Err(err) => {
            println!("⑤' 经网关非流式对话：**失败**");
            println!("   {err}");
            return 1;
        }
    }
    0
}

/// 渠道最近一条 `usage_log`（**用量落库证据**）。
fn last_usage(store: &Store, channel_id: &str) -> Option<String> {
    store
        .with_conn(|conn| {
            let row = conn.query_row(
                "SELECT status, model_alias, upstream_model, prompt_tokens, completion_tokens,
                        estimated, duration_ms, error_kind
                 FROM usage_log WHERE channel_id = ?1 ORDER BY id DESC LIMIT 1",
                [channel_id],
                |row| {
                    Ok(format!(
                        "status={} alias={} upstream={} tokens={}/{} estimated={} 耗时={}ms error={:?}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            );
            Ok(row.ok())
        })
        .unwrap_or(None)
}

/// 经真网关发一次流式对话，返回拼接后的文本。
async fn run_chat(
    store: &Arc<Store>,
    session: &CopilotSession,
    alias: &str,
    stream: bool,
) -> Result<(usize, String, String), String> {
    let router = AliasRouter::new(Arc::clone(store));
    let plan = router
        .resolve("t21-verify", alias)
        .await
        .map_err(|err| format!("路由解析失败：{err:?}"))?;
    let vault = Arc::clone(&session.vault);
    let forwarder = Forwarder::new(
        OutboundClient::new().map_err(|err| format!("构造出站客户端失败：{err}"))?,
        Arc::new(CircuitBreaker::new()),
        Arc::new(StreamUsageDowngrades::default()),
        vault,
        Arc::clone(store),
        Arc::new(StoreUsageSink::new(Arc::clone(store))),
        Arc::new(CharRatioEstimator),
    )
    .with_adapter_deps(AdapterDeps {
        copilot: session.clone(),
    });
    let request = parse_chat_request(serde_json::json!({
        "model": alias,
        "messages": [{"role": "user", "content": "用一句话介绍你自己"}],
        "stream": stream
    }))
    .map_err(|err| format!("构造请求失败：{err:?}"))?;
    let response = forwarder
        .forward(&plan, &request, &HeaderMap::new(), None)
        .await
        .map_err(|err| format!("网关转发失败：{err:?}"))?;
    if !response.status().is_success() {
        return Err(format!("网关返回 HTTP {}", response.status()));
    }
    let mut body = response.into_body();
    let mut raw = String::new();
    let mut content = String::new();
    let mut frames = 0usize;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|err| format!("读流失败：{err}"))?;
        let bytes = frame.into_data().unwrap_or_default();
        let chunk = String::from_utf8_lossy(&bytes).to_string();
        frames += 1;
        raw.push_str(&chunk);
        // 把 SSE 帧里的 `delta.content` 拼起来（**证据**：上游真的回话了）
        for line in chunk.lines() {
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload.trim() == "[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
                if let Some(text) = value["choices"][0]["delta"]["content"].as_str() {
                    content.push_str(text);
                }
            }
        }
    }
    // 非流式：整包 JSON → `choices[0].message.content`
    if content.is_empty() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
            if let Some(text) = value["choices"][0]["message"]["content"].as_str() {
                content.push_str(text);
            }
        }
    }
    Ok((frames, content, raw))
}

/// 截断长文本。
fn truncate(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }
    let mut out: String = chars[..limit].iter().collect();
    out.push('…');
    out
}
