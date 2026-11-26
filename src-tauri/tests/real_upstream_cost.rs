//! 真实上游实测（OpenRouter 免费模型）—— 验证 **真实 usage → `cost_micros`** 全链。
//!
//! 用途：T13 的验收证据之一。现有 e2e（`tests/usage_e2e.rs`）打的是 **mock 上游**，
//! 本文件打的是**真上游**，验的是「真实上游报的 token 数能被正确采集并换成成本」。
//!
//! **默认不跑**（`#[ignore]`）：需要真实 API Key，且会打真实网络。
//!
//! ```powershell
//! $env:OPENROUTER_API_KEY = "<你的 OpenRouter API key（sk-or-v1- 开头）>"
//! cargo test --offline --test real_upstream_cost -- --ignored --nocapture
//! ```
//!
//! **key 必须是 OpenRouter 的 API key**（形如 `sk-or-v1-<64 hex>`）；本项目自己的入站
//! client key（`llmr_` 开头）是**另一回事**，拿它去请求 OpenRouter 会被判成
//! `Missing Authentication header`。
//!
//! - `OPENROUTER_MODEL` 可选，覆盖默认的免费模型（默认按候选列表依次尝试）。
//! - **零花费**：只用 OpenRouter 的 `:free` 模型；`cost_micros` 由**我们自己的价目表**
//!   算出（02 §8.3），与上游收多少钱无关。
//! - **key 不落盘**：从环境变量读进内存后直接进保险库，不打印、不写文件。

use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use llm_router_lib::gateway::auth;
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::estimator::TiktokenEstimator;
use llm_router_lib::gateway::fallback::Forwarder;
use llm_router_lib::gateway::outbound::{OutboundClient, StreamUsageDowngrades};
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::gateway::router::AliasRouter;
use llm_router_lib::gateway::server::{self, GatewayState};
use llm_router_lib::store::price::{upsert, PriceEntry};
use llm_router_lib::store::usage::StoreUsageSink;
use llm_router_lib::store::Store;
use llm_router_lib::vault::crypto::SecretBytes;
use llm_router_lib::vault::Vault;
use tower::ServiceExt;

/// OpenRouter 端点（自定义 OpenAI 兼容渠道）。
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
/// OpenRouter 的 chat 路径（拼在 base_url 之后）。
const OPENROUTER_CHAT_PATH: &str = "/chat/completions";

/// 候选免费模型（OpenRouter `:free`；2026-11-26 实测在售 11 个，逐个尝试直到成功）。
const FREE_MODEL_CANDIDATES: &[&str] = &[
    "liquid/lfm-3-1.7b:free",
    "google/gemma-4-26b-a4b-it:free",
    "inclusionai/ling-3.2-flash:free",
    "baidu/ernie-5.5-flash-preview:free",
];

/// 测试价（1e-6 元 / 1M tokens）：输入 1 元、输出 2 元、缓存命中 0.1 元。
/// 这组价是**我们自己配的测试价**——`cost_micros` 按它算，与上游收多少钱无关。
const TEST_INPUT_PER_1M: i64 = 1_000_000;
const TEST_OUTPUT_PER_1M: i64 = 2_000_000;
const TEST_CACHE_READ_PER_1M: i64 = 100_000;

/// 独立复算的期望成本（**不调用生产代码的换算函数**，避免自证）。
///
/// 与 02 §8.3 的公式一致：可计费输入 = prompt - cache_read；缓存写按输入价；总额四舍五入一次。
fn expected_cost_micros(
    prompt: i64,
    completion: i64,
    cache_read: i64,
    cache_write: i64,
) -> i64 {
    let billable_input = (prompt - cache_read).max(0);
    let total: i128 = billable_input as i128 * TEST_INPUT_PER_1M as i128
        + completion as i128 * TEST_OUTPUT_PER_1M as i128
        + cache_read as i128 * TEST_CACHE_READ_PER_1M as i128
        + cache_write as i128 * TEST_INPUT_PER_1M as i128;
    ((total + 500_000) / 1_000_000) as i64
}

/// 网关夹具。
struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<Store>,
    state: GatewayState,
    vault: Arc<RwLock<Vault>>,
    key: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault = Arc::new(RwLock::new(
        Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
    ));
    let forwarder = Arc::new(Forwarder::new(
        OutboundClient::new().expect("出站客户端"),
        Arc::new(CircuitBreaker::with_persistence(Arc::new(
            StoreCircuitPersistence::new(Arc::clone(&store)),
        ))),
        Arc::new(StreamUsageDowngrades::default()),
        Arc::clone(&vault),
        Arc::clone(&store),
        Arc::new(StoreUsageSink::new(Arc::clone(&store))),
        Arc::new(TiktokenEstimator),
    ));
    let state = GatewayState {
        router: Arc::new(AliasRouter::new(Arc::clone(&store))),
        forwarder: Arc::clone(&forwarder),
        store: Arc::clone(&store),
        limiter: Arc::new(RateLimiter::new()),
    };

    let key = auth::generate_client_key();
    let hash = auth::hash_client_key(&key);
    let prefix = auth::key_prefix(&key);
    let now = llm_router_lib::vault::now_secs();
    store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                 VALUES ('ck1', 'real-upstream', ?1, ?2, ?3)",
                rusqlite::params![hash, prefix, now],
            )?;
            Ok(())
        })
        .expect("写入 client key");

    Fixture {
        _dir: dir,
        store,
        state,
        vault,
        key,
    }
}

/// 种入一个 OpenRouter 渠道（provider = `custom`）+ 模型 + 凭证 + 测试价。
fn seed_openrouter(f: &Fixture, model: &str, api_key: &str) {
    let now = llm_router_lib::vault::now_secs();
    let meta = format!(
        r#"{{"path_chat":"{OPENROUTER_CHAT_PATH}","extra_headers":{{"HTTP-Referer":"https://github.com/benntqoo/llm-router","X-Title":"LLM Router"}}}}"#
    );
    f.store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at, meta_json)
                 VALUES ('c-or', 'OpenRouter', 'api_key', 'custom', ?1, 1, 100, ?2, ?3)",
                rusqlite::params![OPENROUTER_BASE_URL, now, meta],
            )?;
            conn.execute(
                "INSERT INTO model (id, channel_id, upstream_id, alias, enabled)
                 VALUES ('m-or', 'c-or', ?1, 'or-free', 1)",
                rusqlite::params![model],
            )?;
            // 测试价：provider = `custom`（自定义渠道的 provider 只能是 custom）
            upsert(
                conn,
                &PriceEntry {
                    provider: "custom".to_string(),
                    model_pattern: model.to_string(),
                    input_per_1m: TEST_INPUT_PER_1M,
                    output_per_1m: TEST_OUTPUT_PER_1M,
                    cache_read_per_1m: TEST_CACHE_READ_PER_1M,
                },
            )?;
            Ok(())
        })
        .expect("写入渠道/模型/价目");

    f.vault
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .put_credential(
            "c-or",
            "api_key",
            &SecretBytes::from_slice(api_key.as_bytes()),
        )
        .expect("写入凭证");
}

/// 发起一次 chat 请求；返回 (HTTP 状态码, 响应体文本)。
async fn chat(f: &Fixture, prompt: &str) -> (u16, String) {
    let body = serde_json::json!({
        "model": "or-free",
        "messages": [{"role": "user", "content": prompt}],
        "stream": false
    });
    let response = server::router(f.state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("host", "127.0.0.1:8787")
                .header("authorization", format!("Bearer {}", f.key))
                .body(Body::from(body.to_string()))
                .expect("构造请求"),
        )
        .await
        .expect("请求应成功");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体");
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// `usage_log` 的一行（只取本测试需要的字段）。
#[derive(Debug, Clone)]
struct Row {
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    estimated: i64,
    status: Option<String>,
    cost_micros: Option<i64>,
}

fn query_rows(store: &Store) -> Vec<Row> {
    store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens,
                        estimated, status, cost_micros
                 FROM usage_log ORDER BY id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(Row {
                    prompt_tokens: row.get(0)?,
                    completion_tokens: row.get(1)?,
                    cache_read_tokens: row.get(2)?,
                    cache_write_tokens: row.get(3)?,
                    estimated: row.get(4)?,
                    status: row.get(5)?,
                    cost_micros: row.get(6)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .expect("读取 usage_log")
}

async fn wait_rows(store: &Store, expected: usize) -> Vec<Row> {
    for _ in 0..1000 {
        let rows = query_rows(store);
        if rows.len() >= expected {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等待 {expected} 行 usage_log 超时，实际 {} 行", query_rows(store).len());
}

/// 候选模型列表：`OPENROUTER_MODEL` 优先，否则按免费模型列表依次尝试。
fn candidate_models() -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(model) = std::env::var("OPENROUTER_MODEL") {
        if !model.trim().is_empty() {
            candidates.push(model.trim().to_string());
        }
    }
    candidates.extend(FREE_MODEL_CANDIDATES.iter().map(|s| s.to_string()));
    candidates
}

/// 逐个尝试候选模型，返回**第一个拿到 HTTP 200** 的夹具与模型 id。
///
/// 每个候选用独立夹具（独立 store / 保险库），所以失败候选留下的错误行不会污染成功候选的 `usage_log`。
async fn first_working_fixture(api_key: &str, probe_prompt: &str) -> (Fixture, String) {
    let mut errors: Vec<String> = Vec::new();
    for model in candidate_models() {
        let f = fixture();
        seed_openrouter(&f, &model, api_key);
        let (status, body) = chat(&f, probe_prompt).await;
        if status == 200 {
            return (f, model);
        }
        errors.push(format!("{model}: HTTP {status} → {}", truncate(&body, 200)));
        println!("[real-upstream] {model} 失败（HTTP {status}），换下一个候选");
    }
    panic!(
        "所有候选免费模型都失败：\n{}",
        errors.join("\n")
    );
}

/// 真实上游实测：**真实 token 数 → `cost_micros`**。
#[tokio::test]
#[ignore = "需要真实 OpenRouter API Key 与外网；默认不跑"]
async fn real_upstream_cost_micros_matches_independent_recompute() {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("需要环境变量 OPENROUTER_API_KEY（真实上游实测用）");
    assert!(!api_key.trim().is_empty(), "OPENROUTER_API_KEY 不能为空");

    // 逐个尝试候选免费模型（免费模型限流较重）→ 拿到第一个成功的夹具。
    let (f, model) = first_working_fixture(&api_key, "用一句话介绍你自己。").await;

    let rows = wait_rows(&f.store, 1).await;
    let row = &rows[0];

    println!("[real-upstream] model = {model}");
    println!(
        "[real-upstream] usage: prompt={} completion={} cache_read={} cache_write={} estimated={} status={:?}",
        row.prompt_tokens,
        row.completion_tokens,
        row.cache_read_tokens,
        row.cache_write_tokens,
        row.estimated,
        row.status
    );
    println!("[real-upstream] cost_micros = {:?}", row.cost_micros);

    assert_eq!(row.status.as_deref(), Some("success"), "请求应成功");
    assert!(
        row.prompt_tokens > 0,
        "{model}: 真实上游应报出输入 token（usage 解析没生效？）"
    );
    assert!(
        row.completion_tokens > 0,
        "{model}: 真实上游应报出输出 token（usage 解析没生效？）"
    );
    assert_eq!(row.estimated, 0, "真实上游给了 usage → 不是估算值");

    // 独立复算（不调用生产代码的换算函数）
    let expected = expected_cost_micros(
        row.prompt_tokens,
        row.completion_tokens,
        row.cache_read_tokens,
        row.cache_write_tokens,
    );
    assert_eq!(
        row.cost_micros,
        Some(expected),
        "{model}: 成本应等于独立复算值（prompt={} completion={} cache_read={}）",
        row.prompt_tokens,
        row.completion_tokens,
        row.cache_read_tokens
    );
    println!("[real-upstream] 独立复算 = {expected} micros ✅");
}

/// 真实上游实测（第二轮，同一提示词）：观察 OpenRouter 是否报出**缓存命中** token。
///
/// 缓存命中是 `cost_micros` 三价里的第三档——上游不报就不验，**不强行要求**。
#[tokio::test]
#[ignore = "需要真实 OpenRouter API Key 与外网；默认不跑"]
async fn real_upstream_reports_cache_read_tokens_if_any() {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("需要环境变量 OPENROUTER_API_KEY（真实上游实测用）");
    assert!(!api_key.trim().is_empty(), "OPENROUTER_API_KEY 不能为空");

    // 同一段较长的提示词连发两次：若上游做前缀缓存，第二次可能报出 cached_tokens。
    let prompt = "请用三句话解释什么是向量数据库，并给出一个具体的使用场景。".repeat(8);

    // 先探一个能用的模型（探测本身也算一轮请求，故最终 3 行 usage_log）。
    let (f, model) = first_working_fixture(&api_key, "ping").await;

    for round in 1..=2 {
        let (status, body) = chat(&f, &prompt).await;
        assert_eq!(
            status, 200,
            "{model}: 第 {round} 轮应成功 → HTTP {status} {}",
            truncate(&body, 200)
        );
    }

    let rows = wait_rows(&f.store, 3).await;
    for (i, row) in rows.iter().enumerate() {
        println!(
            "[real-upstream] 第 {} 轮: prompt={} completion={} cache_read={} cost_micros={:?}",
            i + 1,
            row.prompt_tokens,
            row.completion_tokens,
            row.cache_read_tokens,
            row.cost_micros
        );
        // 失败轮（401/429）也会落一行，但 token 为 0 —— 必须显式判成功，
        // 否则「上游全失败 + 成本恒为 0」会变成假绿。
        assert_eq!(
            row.status.as_deref(),
            Some("success"),
            "第 {} 轮应成功（失败轮 token 为 0，会让断言失去意义）",
            i + 1
        );
        assert!(
            row.prompt_tokens > 0 && row.completion_tokens > 0,
            "第 {} 轮应报出真实 token（prompt={} completion={}）",
            i + 1,
            row.prompt_tokens,
            row.completion_tokens
        );
        let expected = expected_cost_micros(
            row.prompt_tokens,
            row.completion_tokens,
            row.cache_read_tokens,
            row.cache_write_tokens,
        );
        assert_eq!(row.cost_micros, Some(expected), "第 {} 轮成本应等于独立复算值", i + 1);
    }
    println!(
        "[real-upstream] 缓存命中 token：{:?}（上游不报就是 0，不强求）",
        rows.iter().map(|r| r.cache_read_tokens).collect::<Vec<_>>()
    );
}

/// 截断长文本（避免把整段响应体打进日志）。
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
