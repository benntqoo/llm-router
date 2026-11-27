//! **ModelScope 真机探针**（T18 留档 §4.2 ⑩ 的复现工具）。
//!
//! 用法：`cargo run --example checkin_modelscope_probe -- <site_id>`
//! （`site_id` 省略时用魔搭站点 `site_182212017abbf1808375126911d9c85d`）
//!
//! 目的：
//! 1. 走**应用内「立即执行」同一条路径**（`scheduler::run_now`）→ 真实签到 + 落库 +
//!    额度回写（本例里与 UI 点「立即执行」完全同一条链）；
//! 2. 额外打魔粒流水接口，打**结构摘要**（键名 + 类型；值一律打码），
//!    并用插件的 `daily_magicubes` 算一遍当日每日签到魔粒，核对字段形状。
//!
//! **注意**：它会打**真实站点**（一次签到 + 若干次对账）——**别在同一天反复跑**
//! （站点会判频控）。真机当天已领过时，`run_now` 的差值口径会判「今天已领过」。
//!
//! **密文红线**：令牌只从保险库取到内存，**绝不打印**；输出里的字符串一律打码。

use std::sync::{Arc, RwLock};

use llm_router_lib::checkin::plugin::parse_config;
use llm_router_lib::checkin::replay::ReplayEngine;
use llm_router_lib::checkin::scheduler;
use llm_router_lib::checkin::sites::modelscope::{
    daily_magicubes, session_cookie, TRANSACTIONS_PATH,
};
use llm_router_lib::store::checkin;
use llm_router_lib::store::Store;
use llm_router_lib::vault::Vault;
use serde_json::Value;

/// 可以原样打印的字符串键（**枚举值**，不含个人信息）。
const SAFE_STRING_KEYS: &[&str] = &[
    "type",
    "rule_key",
    "ruleKey",
    "status",
    "currency",
    "unit",
    "biz_type",
    "bizType",
    "action",
    "direction",
];

/// 结构摘要（**值全打码**）。
fn shape(value: &Value, depth: usize) -> String {
    if depth > 6 {
        return "<deep>".to_string();
    }
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => format!("bool({b})"),
        Value::Number(n) => {
            let v = n.as_i64().unwrap_or(-1);
            if (0..100_000).contains(&v) {
                format!("num({v})")
            } else {
                "<num>".to_string()
            }
        }
        Value::String(s) => {
            if s.len() <= 32 && !s.contains('=') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                format!("str({s})")
            } else {
                "<str>".to_string()
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                "[] (empty)".to_string()
            } else {
                let inner = shape(&items[0], depth + 1);
                format!("array(len={}) [0]={inner}", items.len())
            }
        }
        Value::Object(map) => {
            let mut parts = Vec::new();
            for (key, item) in map {
                let rendered = match item {
                    Value::String(s) if SAFE_STRING_KEYS.contains(&key.as_str()) => {
                        format!("str({s})")
                    }
                    other => shape(other, depth + 1),
                };
                parts.push(format!("{key}:{rendered}"));
            }
            format!("{{{}}}", parts.join(", "))
        }
    }
}

#[tokio::main]
async fn main() {
    let site_id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "site_182212017abbf1808375126911d9c85d".to_string());

    let store = Arc::new(
        Store::open(&llm_router_lib::store::default_db_path()).expect("打开库失败"),
    );
    let vault = Arc::new(RwLock::new(
        Vault::open(
            Arc::clone(&store),
            &llm_router_lib::vault::default_vault_path(),
            None,
        )
        .expect("打开保险库失败"),
    ));
    let engine = ReplayEngine::new().expect("构造重放引擎失败");

    // ---- 1) 走 UI「立即执行」同一条路径 ----
    let outcome = scheduler::run_now(&store, &vault, &engine, &site_id)
        .await
        .expect("run_now 失败");
    println!("=== 1) run_now 结果 ===");
    println!(
        "ok={} class={} status={:?} elapsed_ms={} retryable={}",
        outcome.ok,
        outcome.class.as_str(),
        outcome.status,
        outcome.elapsed_ms,
        outcome.retryable
    );
    println!("message={}", outcome.message);
    match outcome.reward.as_ref() {
        Some(reward) => println!("reward amount={} unit={}", reward.amount, reward.unit),
        None => println!("reward = None"),
    }

    // ---- 2) 魔粒流水：结构摘要 + 当日到账核算 ----
    println!();
    println!("=== 2) 魔粒流水结构 ===");
    let row = store
        .with_conn(|conn| checkin::get_site(conn, &site_id))
        .ok()
        .flatten()
        .expect("取站点失败");
    let config = parse_config(&row);
    let credential_id = config.credential_id.clone().expect("站点没配凭据");
    let secret = {
        let guard = vault.read().expect("保险库锁失败");
        let bytes = guard.get_credential(&credential_id).expect("取凭据失败");
        String::from_utf8_lossy(bytes.as_bytes()).to_string()
    };
    println!(
        "凭据形态：{}（含 '=' → 原始 Cookie 头；否则 → 访问令牌）",
        if secret.contains('=') { "Cookie头" } else { "访问令牌" }
    );
    let cookie = session_cookie(&secret);
    let base = config
        .base_url
        .clone()
        .unwrap_or_else(|| "https://www.modelscope.cn".to_string());
    let url = format!("{}{}", base.trim_end_matches('/'), TRANSACTIONS_PATH);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("建 client 失败");

    // 2a) Bearer（令牌形态）
    let mut req = client.get(&url).header(reqwest::header::ACCEPT, "application/json");
    if !secret.contains('=') {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {secret}"));
    } else {
        req = req.header(reqwest::header::COOKIE, cookie.clone());
    }
    let resp = req.send().await;
    match resp {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.bytes().await.unwrap_or_default();
            println!("[Bearer] status={status} len={}", body.len());
            match serde_json::from_slice::<Value>(&body) {
                Ok(json) => {
                    println!("[Bearer] shape={}", shape(&json, 0));
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    println!(
                        "[Bearer] daily_magicubes({today}) = {:?}",
                        daily_magicubes(&json, &today).map(|(total, _)| total)
                    );
                    if let Some(data) = json.get("data") {
                        println!("[Bearer] data 顶层键 = {:?}", data.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()));
                    }
                    // 逐条流水（非敏感字段：type / rule_key / 日期 / 金额 / 条数）
                    if let Some(records) = json
                        .get("data")
                        .and_then(|d| d.get("records"))
                        .and_then(Value::as_array)
                    {
                        for (index, record) in records.iter().enumerate() {
                            let get = |key: &str| record.get(key).and_then(Value::as_str).unwrap_or("-");
                            let num = |key: &str| record.get(key).and_then(Value::as_i64).unwrap_or(-1);
                            println!(
                                "  record[{index}] type={} rule_key={} gmt_created={} total_amount={} count={} expire_days={} days_remaining={} category_key={} 其它键={:?}",
                                get("type"),
                                get("rule_key"),
                                get("gmt_created"),
                                num("total_amount"),
                                num("count"),
                                num("expire_days"),
                                num("days_remaining"),
                                get("category_key"),
                                record.as_object().map(|m| m.keys().filter(|k| !matches!(k.as_str(), "type" | "rule_key" | "gmt_created" | "total_amount" | "count" | "expire_days" | "days_remaining" | "category_key")).cloned().collect::<Vec<_>>()),
                            );
                        }
                    }
                }
                Err(err) => println!("[Bearer] 不是 JSON：{err}；前 200 字节 = {:?}", String::from_utf8_lossy(&body[..body.len().min(200)])),
            }
        }
        Err(err) => println!("[Bearer] 请求失败：{err}"),
    }

    // 2b) Cookie 会话形态（同一接口，比对两种认证的差别）
    let resp = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await;
    match resp {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.bytes().await.unwrap_or_default();
            println!("[Cookie] status={status} len={}", body.len());
            match serde_json::from_slice::<Value>(&body) {
                Ok(json) => println!("[Cookie] shape={}", shape(&json, 0)),
                Err(err) => println!("[Cookie] 不是 JSON：{err}；前 200 字节 = {:?}", String::from_utf8_lossy(&body[..body.len().min(200)])),
            }
        }
        Err(err) => println!("[Cookie] 请求失败：{err}"),
    }

    // 2c) 签到接口自身的响应体（**结构摘要**）——看有没有「今天已签到 / 今日魔粒」信号
    println!();
    println!("=== 3) 签到接口响应体结构 ===");
    let signin_url = format!(
        "{}{}",
        base.trim_end_matches('/'),
        llm_router_lib::checkin::sites::modelscope::SIGNIN_PATH
    );
    let resp = client
        .get(&signin_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::COOKIE, session_cookie(&secret))
        .send()
        .await;
    match resp {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.bytes().await.unwrap_or_default();
            println!("[signin] status={status} len={}", body.len());
            match serde_json::from_slice::<Value>(&body) {
                Ok(json) => {
                    println!("[signin] 顶层键 = {:?}", json.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()));
                    println!("[signin] shape={}", shape(&json, 0));
                    if let Some(data) = json.get("Data") {
                        println!("[signin] Data 键 = {:?}", data.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()));
                        println!("[signin] Data shape={}", shape(data, 1));
                    }
                }
                Err(err) => println!("[signin] 不是 JSON：{err}"),
            }
        }
        Err(err) => println!("[signin] 请求失败：{err}"),
    }
}
