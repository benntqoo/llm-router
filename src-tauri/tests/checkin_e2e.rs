//! T15 HAR 导入与模板化 —— 端到端验收测试（真实浏览器抓包的脱敏夹具）。
//!
//! 验收判据（03 §2 T15）：**导入一个真实 HAR 并标记 2 个变量成功**。
//!
//! 本用例用的夹具 `tests/fixtures/modelscope-checkin.har` 是**真实浏览器抓包**
//! （Chromium DevTools 协议导出，2026-11-27 抓 ModelScope 首页）脱敏而来：
//! 保留 HAR 1.2 的真实结构与接口路径，**响应体/响应头按 02 §7.1.1 丢弃**，
//! 敏感取值换成 `REDACTED-*` 占位——所以它可以进 git（AGENT.md §8）。
//!
//! 覆盖两种**真实形态**（实测发现，不是假设）：
//! - entry 0 = `GET /api/v1/users/login/info`（真正的签到接口，匿名态 401）：
//!   它的 Cookie 只有 8 项（`acw_tc` / `cna` / …），**没有 csrf**——
//!   csrf 是服务端在响应里下发、后续请求才带；
//! - entry 1 起 = 带 `csrf_session` + `csrf_token` 的请求（02 §7.1 举的
//!   `{{cookie}}` / `{{csrf}}` 形态）。
//!
//! 关键判据：**零明文**——库 dump 里搜不到变量的真实取值（只有保险库里的密文）。

use std::sync::{Arc, RwLock};

use llm_router_lib::checkin::har::{MarkedVariable, VarTarget};
use llm_router_lib::commands::checkin::{
    checkin_import_har_of, checkin_list_sites_of, checkin_save_template_of, SaveTemplateInput,
};
use llm_router_lib::gateway::circuit::CircuitBreaker;
use llm_router_lib::gateway::circuit_store::StoreCircuitPersistence;
use llm_router_lib::gateway::ratelimit::RateLimiter;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use llm_router_lib::AppState;

/// 真实抓包脱敏后的夹具。
const FIXTURE: &str = "tests/fixtures/modelscope-checkin.har";

/// 夹具里被标记变量的**原始取值**（脱敏后的假值；用于证明零明文）。
const ACW_VALUE: &str = "REDACTED-acw_tc";
const SESSION_VALUE: &str = "REDACTED-csrf_session";
const CSRF_VALUE: &str = "REDACTED-csrf_token";

fn app_state() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
    let vault = Arc::new(RwLock::new(
        Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
    ));
    let circuit = Arc::new(CircuitBreaker::with_persistence(Arc::new(
        StoreCircuitPersistence::new(Arc::clone(&store)),
    )));
    let state = AppState {
        gateway_addr: "127.0.0.1:8787".to_string(),
        gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
        store,
        vault,
        limiter: Arc::new(RateLimiter::new()),
        circuit,
    };
    (dir, state)
}

/// 标记 Cookie 里的某个子项。
fn cookie_part(name: &str, part: &str) -> MarkedVariable {
    MarkedVariable {
        name: name.to_string(),
        target: VarTarget::HeaderPart {
            field: "Cookie".to_string(),
            part: part.to_string(),
        },
    }
}

/// 签到接口上的两个变量：一个敏感（`acw_tc`）、一个非敏感（`cna` 设备标识）。
fn checkin_api_marks() -> Vec<MarkedVariable> {
    vec![cookie_part("acw_tc", "acw_tc"), cookie_part("cna", "cna")]
}

/// 02 §7.1 举的那两个变量：`{{cookie}}` 与 `{{csrf}}`。
fn csrf_marks() -> Vec<MarkedVariable> {
    vec![
        cookie_part("cookie", "csrf_session"),
        cookie_part("csrf", "csrf_token"),
    ]
}

fn save_input(entry_index: usize, variables: Vec<MarkedVariable>) -> SaveTemplateInput {
    SaveTemplateInput {
        path: FIXTURE.to_string(),
        entry_index,
        site_name: "ModelScope".to_string(),
        channel_id: None,
        channel_name: Some("ModelScope·签到".to_string()),
        variables,
    }
}

#[test]
fn real_har_shape_is_imported_with_masked_values() {
    let imported = checkin_import_har_of(FIXTURE).expect("导入真实夹具失败");
    assert_eq!(imported.source, "modelscope-checkin.har");
    assert_eq!(imported.entry_count, 12, "夹具里是 12 条接口请求");
    assert!(imported.entries[0].url.contains("/api/v1/users/login/info"));
    assert_eq!(imported.entries[0].method, "GET");
    assert_eq!(imported.entries[0].status, 401, "匿名态签到接口返回 401");

    let payload = serde_json::to_string(&imported).expect("序列化");
    assert!(
        !payload.contains(SESSION_VALUE),
        "敏感值不得出现在导入视图里"
    );
    assert!(!payload.contains(CSRF_VALUE));
    assert!(!payload.contains(ACW_VALUE));
    assert!(payload.contains("csrf_session"), "键名要可见，用户才知道标什么");
}

#[test]
fn marking_two_variables_on_the_checkin_api_produces_a_placeholder_template() {
    let (_dir, state) = app_state();
    let summary = checkin_save_template_of(&state, &save_input(0, checkin_api_marks()))
        .expect("保存模板失败");

    assert_eq!(summary.name, "ModelScope");
    assert!(!summary.enabled, "导入的站点默认关（02 §7.3）");
    let template = summary.template.expect("应有模板摘要");
    assert_eq!(template.method, "GET");
    assert!(template.url.contains("/api/v1/users/login/info"));
    assert_eq!(template.status, 401);
    assert_eq!(
        template.variable_count, 8,
        "标记了 2 个 + 自动补齐 6 个 cookie 子项（cookie 一律按密文处理）"
    );
    assert!(template.variable_names.contains(&"acw_tc".to_string()));
    assert!(template.variable_names.contains(&"cna".to_string()));

    // 模板里是占位符；敏感变量的原值不进模板
    let row = state
        .store
        .with_conn(|conn| llm_router_lib::store::checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let config = row.config_json.expect("模板 JSON");
    assert!(config.contains("{{acw_tc}}"));
    assert!(config.contains("{{cna}}"));
    assert!(!config.contains(ACW_VALUE), "敏感原值不得进模板");
    assert!(!config.contains("REDACTED-cna"), "cookie 子项一律按密文处理");
    assert!(
        config.contains("application/json"),
        "非敏感的结构类请求头可留在模板里"
    );

    // cookie 子项全部进保险库
    let vault = state.vault.read().expect("读保险库");
    let metas = vault.list_credentials(None).expect("列凭据");
    assert_eq!(metas.len(), 8, "cookie 子项全部进保险库");
    assert!(metas.iter().all(|meta| meta.kind == "cookie"));
    let secret = vault.get_credential(&metas[0].id).expect("解密");
    assert_eq!(
        String::from_utf8_lossy(secret.as_bytes()),
        ACW_VALUE,
        "保险库里能解出原值"
    );

    let sites = checkin_list_sites_of(&state).expect("列站点失败");
    assert_eq!(sites.len(), 1);
}

#[test]
fn the_database_dump_contains_no_plain_variable_value() {
    let (dir, state) = app_state();
    let summary = checkin_save_template_of(&state, &save_input(1, csrf_marks()))
        .expect("保存模板失败");

    // 模板里是占位符（02 §7.1 的 {{cookie}} / {{csrf}} 形态）
    let row = state
        .store
        .with_conn(|conn| llm_router_lib::store::checkin::get_site(conn, &summary.id))
        .expect("读站点")
        .expect("站点应存在");
    let config = row.config_json.expect("模板 JSON");
    assert!(config.contains("{{cookie}}"));
    assert!(config.contains("{{csrf}}"));
    assert!(!config.contains(SESSION_VALUE));
    assert!(!config.contains(CSRF_VALUE));
    // 未标记的敏感字段被自动补齐（名字带 auto_ 前缀）
    assert!(
        config.contains("{{auto_acw_tc}}"),
        "未标记的敏感字段要自动补成变量，否则会明文留在模板里"
    );
    // **整个库**（含凭据表）里搜不到变量的真实取值：只有保险库里的密文
    let dump = dump_database(&state);
    assert!(!dump.contains(SESSION_VALUE), "库里不得出现变量原值（明文）");
    assert!(!dump.contains(CSRF_VALUE));
    assert!(!dump.contains(ACW_VALUE));

    // 但保险库能解出来（密文是真的密文，不是丢了）
    let vault = state.vault.read().expect("读保险库");
    let metas = vault.list_credentials(None).expect("列凭据");
    assert_eq!(metas.len(), 10, "2 个显式 + 自动补齐的 8 个 cookie 子项");
    let mut values = Vec::new();
    for meta in &metas {
        let secret = vault.get_credential(&meta.id).expect("解密");
        values.push(String::from_utf8_lossy(secret.as_bytes()).to_string());
    }
    assert!(values.iter().any(|v| v == SESSION_VALUE));
    assert!(values.iter().any(|v| v == CSRF_VALUE));
    assert!(values.iter().any(|v| v == ACW_VALUE));

    // 保险库文件本身也不该有明文
    let vault_raw = std::fs::read(dir.path().join("vault.bin")).expect("读保险库文件");
    let vault_text = String::from_utf8_lossy(&vault_raw);
    assert!(!vault_text.contains(SESSION_VALUE));
    assert!(!vault_text.contains(CSRF_VALUE));
}

/// 把库里所有表的文本内容拼成一段（用于搜明文）。
fn dump_database(state: &AppState) -> String {
    let conn = state.store.conn();
    let guard = conn.lock().expect("取锁");
    let mut out = String::new();
    let mut stmt = guard
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .expect("准备查询");
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("执行查询")
        .map(|r| r.expect("读取行失败"))
        .collect();
    drop(stmt);
    for table in tables {
        let sql = format!("SELECT * FROM {table}");
        let mut stmt = guard.prepare(&sql).expect("准备查询");
        let mut rows = stmt.query([]).expect("执行查询");
        while let Some(row) = rows.next().expect("读取行失败") {
            for index in 0..row.as_ref().column_count() {
                if let Ok(value) = row.get_ref(index) {
                    let bytes: Vec<u8> = match value {
                        rusqlite::types::ValueRef::Text(bytes) => bytes.to_vec(),
                        rusqlite::types::ValueRef::Blob(bytes) => bytes.to_vec(),
                        rusqlite::types::ValueRef::Integer(value) => {
                            value.to_string().into_bytes()
                        }
                        rusqlite::types::ValueRef::Real(value) => value.to_string().into_bytes(),
                        _ => Vec::new(),
                    };
                    out.push_str(&String::from_utf8_lossy(&bytes));
                    out.push('\n');
                }
            }
        }
    }
    out
}
