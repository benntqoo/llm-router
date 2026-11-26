//! 真机验收工具：把 **T15 的真实产物**接到 **T16 的重放引擎**上跑一遍（T16；02 §7.1.3）。
//!
//! 一条命令走完整条链：
//! **真实抓包夹具 → 导入并标记变量 → 模板落库（真实值进保险库）→ 取回模板 →
//! 注入变量 → 重放 HTTP → 断言 → 错误分类 → 落 `checkin_result`**。
//!
//! 跑三个场景（任务卡验收的 3 条）：**成功 / 断言失败 / 网络失败**，
//! 外加一条**缺凭据**（证明缺凭据时一个请求都不发出去）。
//!
//! 用法：
//!
//! ```powershell
//! $env:LLMR_STORE_PATH  = "$env:TEMP\llmr-t16-demo\store.db"
//! $env:LLMR_VAULT_PATH = "$env:TEMP\llmr-t16-demo\vault.bin"
//! cargo run --offline --example checkin_replay_demo
//! ```
//!
//! 库与保险库都指向临时目录，**不污染用户真实库**。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;
use llm_router_lib::checkin::har::{JsonPathAssert, MarkedVariable, SiteTemplate, VarTarget};
use llm_router_lib::checkin::replay::{ReplayEngine, ResolveError, SecretResolver};
use llm_router_lib::commands::checkin::{checkin_save_template_of, SaveTemplateInput};
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::store::checkin;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;

/// 真实抓包脱敏后的夹具（T15 的产物）。
/// **按 crate 目录解析**——`cargo run --example` 的工作目录不一定是 crate 目录。
fn fixture_path() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/modelscope-checkin.har")
        .display()
        .to_string()
}

/// 夹具里被标记变量的原始取值（脱敏后的假值；用来核对"注入的值确实是保险库里的值"）。
const ACW_VALUE: &str = "REDACTED-acw_tc";
const CNA_VALUE: &str = "REDACTED-cna";

/// 从保险库取凭据的解析器（T16 的 `SecretResolver` 接到真实保险库上）。
struct VaultResolver {
    vault: Arc<RwLock<Vault>>,
}

impl SecretResolver for VaultResolver {
    fn resolve(&self, credential_id: &str) -> Result<String, ResolveError> {
        let vault = self
            .vault
            .read()
            .map_err(|err| ResolveError::Unavailable(err.to_string()))?;
        match vault.get_credential(credential_id) {
            Ok(secret) => Ok(String::from_utf8_lossy(secret.as_bytes()).to_string()),
            Err(_) => Err(ResolveError::NotFound),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store_path = std::env::var("LLMR_STORE_PATH").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("llmr-t16-demo")
            .join("store.db")
            .display()
            .to_string()
    });
    let vault_path = std::env::var("LLMR_VAULT_PATH").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("llmr-t16-demo")
            .join("vault.bin")
            .display()
            .to_string()
    });
    if let Some(parent) = std::path::Path::new(&store_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    // ── 1. mock 上游（先绑端口）──
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let app = Router::new().fallback(any(move |request: axum::http::Request<axum::body::Body>| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let cookie = request
                .headers()
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            println!("      上游收到 Cookie: {cookie}");
            if request.uri().path().ends_with("/fail401") {
                return (StatusCode::UNAUTHORIZED, "{\"message\":\"unauthorized\"}").into_response();
            }
            if request.uri().path().ends_with("/wrongjson") {
                return (StatusCode::OK, "{\"data\":{\"ok\":false,\"credits\":0}}").into_response();
            }
            (
                StatusCode::OK,
                "{\"data\":{\"ok\":true,\"credits\":400},\"message\":\"签到成功\"}",
            )
                .into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let mock_base = format!("http://{addr}");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // ── 2. 开库 + 开保险库 ──
    let store = Arc::new(Store::open(&store_path)?);
    let vault = Arc::new(RwLock::new(Vault::open(
        Arc::clone(&store),
        &vault_path,
        None,
    )?));
    let circuit = Arc::new(CircuitBreaker::with_persistence(Arc::new(
        StoreCircuitPersistence::new(Arc::clone(&store)),
    )));
    let state = AppState {
        gateway_addr: "127.0.0.1:8787".to_string(),
        gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
        store: Arc::clone(&store),
        vault: Arc::clone(&vault),
        limiter: Arc::new(RateLimiter::new()),
        circuit,
    };

    println!("库: {store_path}");
    println!("保险库: {vault_path}");
    println!("mock 上游: {mock_base}\n");

    // ── 3. T15：导入真实抓包 → 标记 2 个变量 → 存模板 ──
    let summary = checkin_save_template_of(
        &state,
        &SaveTemplateInput {
            path: fixture_path(),
            entry_index: 0,
            site_name: "ModelScope".to_string(),
            channel_id: None,
            channel_name: Some("ModelScope·签到".to_string()),
            variables: vec![
                MarkedVariable {
                    name: "acw_tc".to_string(),
                    target: VarTarget::HeaderPart {
                        field: "Cookie".to_string(),
                        part: "acw_tc".to_string(),
                    },
                },
                MarkedVariable {
                    name: "cna".to_string(),
                    target: VarTarget::HeaderPart {
                        field: "Cookie".to_string(),
                        part: "cna".to_string(),
                    },
                },
            ],
        },
    )?;
    println!(
        "[T15] 模板已存：站点={} 变量={} 个 {:?}（默认关={}）",
        summary.name,
        summary.template.as_ref().map(|t| t.variable_count).unwrap_or(0),
        summary.template.as_ref().map(|t| t.variable_names.clone()).unwrap_or_default(),
        summary.enabled
    );

    // ── 4. T16：取回模板 → 指向 mock → 重放 ──
    let row = state
        .store
        .with_conn(|conn| checkin::get_site(conn, &summary.id))?
        .expect("站点应存在");
    let template: SiteTemplate =
        serde_json::from_str(&row.config_json.expect("模板 JSON")).expect("模板应能解析");

    let resolver = VaultResolver {
        vault: Arc::clone(&vault),
    };
    let engine = ReplayEngine::new()?;

    let mut ok_template = template.clone();
    ok_template.request.url = format!("{mock_base}/api/v1/users/login/info");
    ok_template.assert.status = Some(200);
    ok_template.assert.json_paths = vec![JsonPathAssert {
        path: "data.ok".to_string(),
        equals: Some(serde_json::json!(true)),
    }];

    let mut wrong_json = template.clone();
    wrong_json.request.url = format!("{mock_base}/api/v1/wrongjson");
    wrong_json.assert.status = Some(200);
    wrong_json.assert.json_paths = ok_template.assert.json_paths.clone();

    let mut unauthorized = template.clone();
    unauthorized.request.url = format!("{mock_base}/api/v1/fail401");
    unauthorized.assert.status = Some(200);

    // 找一个没人监听的端口（起一下再关掉）
    let dead_port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        drop(listener);
        addr.port()
    };
    let mut network = template.clone();
    network.request.url = format!("http://127.0.0.1:{dead_port}/api/v1/users/login/info");
    network.assert.status = Some(200);

    println!();
    let scenarios: Vec<(&str, &SiteTemplate)> = vec![
        ("① 成功", &ok_template),
        ("② 断言失败（JSON 路径不符）", &wrong_json),
        ("③ 断言失败（上游 401）", &unauthorized),
        ("④ 网络失败（连不上）", &network),
    ];
    for (label, scenario) in scenarios {
        let outcome = engine.replay(scenario, &resolver).await;
        println!(
            "  {label}: ok={} 分类={} 可重试={} 状态={:?} 耗时={}ms",
            outcome.ok,
            outcome.class.as_str(),
            outcome.retryable,
            outcome.status,
            outcome.elapsed_ms
        );
        println!("      原因: {}", outcome.message);
        state.store.with_conn(|conn| {
            checkin::insert_result(
                conn,
                &summary.id,
                llm_router_lib::vault::now_secs(),
                outcome.ok,
                None,
                Some(&outcome.message),
            )?;
            Ok(())
        })?;
    }

    // ── 5. 缺凭据：一个请求都不该发出去 ──
    let credential_id = template.variables[0]
        .credential_id
        .clone()
        .expect("应有凭据 id");
    {
        let guard = vault.read().expect("读保险库");
        guard.delete_credential(&credential_id)?;
    }
    let before = hits.load(Ordering::SeqCst);
    let outcome = engine.replay(&ok_template, &resolver).await;
    let after = hits.load(Ordering::SeqCst);
    println!();
    println!(
        "  ⑤ 缺凭据: ok={} 分类={} 原因={}",
        outcome.ok,
        outcome.class.as_str(),
        outcome.message
    );
    println!("      上游请求数 {before} → {after}（缺凭据时**不该发出任何请求**）");
    state.store.with_conn(|conn| {
        checkin::insert_result(
            conn,
            &summary.id,
            llm_router_lib::vault::now_secs(),
            outcome.ok,
            None,
            Some(&outcome.message),
        )?;
        Ok(())
    })?;

    // ── 6. 结果落库 + 零明文核对 ──
    println!();
    let rows = state
        .store
        .with_conn(|conn| checkin::list_results(conn, Some(&summary.id), 20))?;
    println!("checkin_result 共 {} 行：", rows.len());
    for row in &rows {
        println!(
            "  ts={} ok={} message={}",
            row.ts,
            row.ok,
            row.message.as_deref().unwrap_or("-")
        );
    }

    let mut plaintext_hits = 0;
    for path in [&store_path, &vault_path] {
        let raw = std::fs::read(path)?;
        for secret in [ACW_VALUE, CNA_VALUE] {
            if raw
                .windows(secret.len())
                .any(|window| window == secret.as_bytes())
            {
                println!("  !! 明文命中: {secret} in {path}");
                plaintext_hits += 1;
            }
        }
    }
    println!(
        "零明文核对：store.db + vault.bin 明文命中 {plaintext_hits} 条（响应体与变量真值都不落库）"
    );
    println!("上游总请求数：{}", hits.load(Ordering::SeqCst));

    Ok(())
}
