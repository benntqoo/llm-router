//! 价目表 IPC（T13；02 §8.3）。
//!
//! 价目表是**用户可编辑**的：预填的国产牌价只是 2026-11-26 的快照，
//! 厂商调价不会自动跟随——用户改一行，本地历史成本随之重算。

use tauri::State;

use crate::store::price::{
    self, compute_cost_micros, lookup, upsert, BillableTokens, PriceEntry,
};
use crate::AppState;

/// 列出全部价目。
#[tauri::command]
pub fn price_list(state: State<'_, AppState>) -> Result<Vec<PriceEntry>, String> {
    price_list_of(&state)
}

/// 列出全部价目（可测版本）。
pub fn price_list_of(state: &AppState) -> Result<Vec<PriceEntry>, String> {
    state
        .store
        .with_conn(|conn| price::list(conn))
        .map_err(|e| e.to_string())
}

/// 写一条价目（同 `(provider, model_pattern)` 则覆盖），并重算该 provider 的历史成本。
///
/// 返回**重算的历史行数**——改价后历史 `cost_micros` 会同步更新，不会停留在旧牌价上。
#[tauri::command]
pub fn price_upsert(
    state: State<'_, AppState>,
    provider: String,
    model_pattern: String,
    input_per_1m: i64,
    output_per_1m: i64,
    cache_read_per_1m: i64,
) -> Result<usize, String> {
    price_upsert_of(
        &state,
        &provider,
        &model_pattern,
        input_per_1m,
        output_per_1m,
        cache_read_per_1m,
    )
}

/// 写一条价目（可测版本）。
pub fn price_upsert_of(
    state: &AppState,
    provider: &str,
    model_pattern: &str,
    input_per_1m: i64,
    output_per_1m: i64,
    cache_read_per_1m: i64,
) -> Result<usize, String> {
    if provider.trim().is_empty() {
        return Err("provider 不能为空".to_string());
    }
    if model_pattern.trim().is_empty() {
        return Err("model_pattern 不能为空".to_string());
    }
    if input_per_1m < 0 || output_per_1m < 0 || cache_read_per_1m < 0 {
        return Err("单价不能为负".to_string());
    }
    let entry = PriceEntry {
        provider: provider.to_string(),
        model_pattern: model_pattern.to_string(),
        input_per_1m,
        output_per_1m,
        cache_read_per_1m,
    };
    state
        .store
        .with_conn(|conn| upsert(conn, &entry))
        .map_err(|e| e.to_string())?;
    price::recompute_provider_costs(&state.store, provider).map_err(|e| e.to_string())
}

/// 删一条价目，并重算该 provider 的历史成本（重算后未命中价目的行 `cost_micros` 记为 NULL）。
#[tauri::command]
pub fn price_delete(
    state: State<'_, AppState>,
    provider: String,
    model_pattern: String,
) -> Result<bool, String> {
    price_delete_of(&state, &provider, &model_pattern)
}

/// 删一条价目（可测版本）。
pub fn price_delete_of(
    state: &AppState,
    provider: &str,
    model_pattern: &str,
) -> Result<bool, String> {
    let removed = state
        .store
        .with_conn(|conn| price::delete(conn, provider, model_pattern))
        .map_err(|e| e.to_string())?;
    price::recompute_provider_costs(&state.store, provider).map_err(|e| e.to_string())?;
    Ok(removed)
}

/// 重算某个 provider 的历史成本（不改动价目）。
#[tauri::command]
pub fn price_recompute(state: State<'_, AppState>, provider: String) -> Result<usize, String> {
    price_recompute_of(&state, &provider)
}

/// 重算某个 provider 的历史成本（可测版本）。
pub fn price_recompute_of(state: &AppState, provider: &str) -> Result<usize, String> {
    price::recompute_provider_costs(&state.store, provider).map_err(|e| e.to_string())
}

/// 试算一次成本换算（不落库；供 UI 预览用）。
#[tauri::command]
pub fn price_preview(
    state: State<'_, AppState>,
    provider: String,
    model: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
) -> Result<Option<i64>, String> {
    price_preview_of(
        &state,
        &provider,
        &model,
        prompt_tokens,
        completion_tokens,
        cache_read_tokens,
        cache_write_tokens,
    )
}

/// 试算一次成本换算（可测版本）；没命中价目返回 `None`（未知）。
pub fn price_preview_of(
    state: &AppState,
    provider: &str,
    model: &str,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
) -> Result<Option<i64>, String> {
    let hit = state
        .store
        .with_conn(|conn| lookup(conn, provider, model))
        .map_err(|e| e.to_string())?;
    Ok(hit.map(|price| {
        compute_cost_micros(
            BillableTokens {
                prompt_tokens,
                completion_tokens,
                cache_read_tokens,
                cache_write_tokens,
            },
            &price,
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    use crate::gateway::circuit::CircuitBreaker;
    use crate::gateway::circuit_store::StoreCircuitPersistence;
    use crate::gateway::ratelimit::RateLimiter;
    use crate::store::usage::{insert_usage, UsageRecord, UsageStatus};
    use crate::store::Store;
    use crate::vault::Vault;

    /// 构造一个带渠道的 `AppState`（只用到数据层）。
    fn app_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
        let vault = Arc::new(RwLock::new(
            Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None)
                .expect("打开保险库"),
        ));
        let circuit = Arc::new(CircuitBreaker::with_persistence(Arc::new(
            StoreCircuitPersistence::new(Arc::clone(&store)),
        )));
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO channel (id, name, kind, provider, base_url, created_at)
                     VALUES ('c1', 'DeepSeek 官方', 'api_key', 'deepseek', 'https://example.invalid', 0)",
                    [],
                )?;
                Ok(())
            })
            .expect("写入渠道");
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

    fn usage_record(prompt: i64, completion: i64, cache_read: i64) -> UsageRecord {
        UsageRecord {
            ts: 1_000,
            client_key_id: Some("ck1".to_string()),
            channel_id: "c1".to_string(),
            account_id: "acct_default_c1".to_string(),
            model_alias: "alias".to_string(),
            upstream_model: "deepseek-flash".to_string(),
            protocol: "openai".to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            cache_read_tokens: cache_read,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            estimated: false,
            ttft_ms: Some(120),
            duration_ms: 900,
            status: UsageStatus::Success,
            error_kind: None,
        }
    }

    fn stored_cost(state: &AppState) -> Vec<Option<i64>> {
        state
            .store
            .with_conn(|conn| {
                let mut stmt =
                    conn.prepare("SELECT cost_micros FROM usage_log ORDER BY id")?;
                let rows = stmt.query_map([], |row| row.get::<_, Option<i64>>(0))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .expect("读取成本失败")
    }

    #[test]
    fn default_prices_are_seeded_on_first_open() {
        let (_dir, state) = app_state();
        let rows = price_list_of(&state).expect("列价目失败");
        assert!(!rows.is_empty(), "首次启动应播种预填牌价");
        assert!(
            rows.iter().any(|r| r.provider == "deepseek"),
            "预填应含 deepseek"
        );
    }

    #[test]
    fn usage_row_gets_cost_from_the_price_table() {
        let (_dir, state) = app_state();
        insert_usage(&state.store, &usage_record(1_000_000, 1_000_000, 0)).expect("写用量失败");
        let costs = stored_cost(&state);
        assert_eq!(costs.len(), 1);
        // 预填 deepseek-flash：输入 2 元/1M、输出 8 元/1M → 10 元 = 10_000_000 micros
        assert_eq!(costs[0], Some(10_000_000));
    }

    #[test]
    fn unknown_model_leaves_cost_null() {
        let (_dir, state) = app_state();
        let mut record = usage_record(1_000_000, 1_000_000, 0);
        record.upstream_model = "某个没配牌价的模型".to_string();
        insert_usage(&state.store, &record).expect("写用量失败");
        let costs = stored_cost(&state);
        assert_eq!(costs[0], None, "没命中价目 → NULL（未知），不是 0");
    }

    #[test]
    fn editing_a_price_recomputes_history() {
        let (_dir, state) = app_state();
        insert_usage(&state.store, &usage_record(1_000_000, 1_000_000, 0)).expect("写用量失败");
        assert_eq!(stored_cost(&state)[0], Some(10_000_000));

        // 把 deepseek-flash 的输入价改成 5 元/1M → 成本应变成 5 + 8 = 13 元
        let recomputed = price_upsert_of(
            &state,
            "deepseek",
            "deepseek-flash",
            5_000_000,
            8_000_000,
            40_000,
        )
        .expect("改价失败");
        assert_eq!(recomputed, 1, "应重算 1 行历史");
        assert_eq!(stored_cost(&state)[0], Some(13_000_000));
    }

    #[test]
    fn deleting_a_price_clears_history_cost() {
        let (_dir, state) = app_state();
        insert_usage(&state.store, &usage_record(1_000_000, 1_000_000, 0)).expect("写用量失败");
        assert_eq!(stored_cost(&state)[0], Some(10_000_000));

        let removed = price_delete_of(&state, "deepseek", "deepseek-flash").expect("删价失败");
        assert!(removed, "应确实删掉");
        assert_eq!(stored_cost(&state)[0], None, "删价后历史成本变 NULL");
    }

    #[test]
    fn invalid_price_input_is_rejected() {
        let (_dir, state) = app_state();
        assert!(price_upsert_of(&state, "", "m", 1, 1, 1).is_err(), "空 provider 应报错");
        assert!(
            price_upsert_of(&state, "p", "  ", 1, 1, 1).is_err(),
            "空 model_pattern 应报错"
        );
        assert!(
            price_upsert_of(&state, "p", "m", -1, 1, 1).is_err(),
            "负单价应报错"
        );
    }

    #[test]
    fn preview_matches_the_stored_computation() {
        let (_dir, state) = app_state();
        insert_usage(&state.store, &usage_record(1_000_000, 0, 0)).expect("写用量失败");
        let preview = price_preview_of(&state, "deepseek", "deepseek-flash", 1_000_000, 0, 0, 0)
            .expect("试算失败");
        assert_eq!(preview, stored_cost(&state)[0]);
    }
}
