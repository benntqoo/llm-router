//! 用量聚合 IPC（T14；02 §8.4）。
//!
//! 面向 UI 的两个入口：
//! - `usage_aggregate` —— 四维聚合（日 / 渠道 / 模型 / 客户端）；
//! - `usage_recent` —— 最近请求明细（M2 判据要的 tokens / TTFT / tokens·s⁻¹ / 成本）。

use tauri::State;

use crate::store::analytics::{self, AggregateQuery, AggregateResult, Dim, RecentRequest};
use crate::AppState;

/// 默认时间窗（近 30 天）。
const DEFAULT_WINDOW_SECS: i64 = 30 * 24 * 3600;
/// 默认返回桶数上限。
const DEFAULT_BUCKET_LIMIT: i64 = 50;
/// 默认返回明细条数。
const DEFAULT_RECENT_LIMIT: i64 = 50;

/// 四维聚合（02 §8.4）。
///
/// `since_ts` 为 `None` 时用默认窗口（近 30 天）；`until_ts` 为 `None` 时不限上界。
#[tauri::command]
pub fn usage_aggregate(
    state: State<'_, AppState>,
    dim: String,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
    limit: Option<i64>,
) -> Result<AggregateResult, String> {
    usage_aggregate_of(&state, &dim, since_ts, until_ts, limit)
}

/// 四维聚合（可测版本）。
pub fn usage_aggregate_of(
    state: &AppState,
    dim: &str,
    since_ts: Option<i64>,
    until_ts: Option<i64>,
    limit: Option<i64>,
) -> Result<AggregateResult, String> {
    let Some(dim) = Dim::parse(dim) else {
        return Err(format!(
            "未知聚合维度：{dim}（应为 day / channel / model / client）"
        ));
    };
    let now = crate::vault::now_secs();
    let query = AggregateQuery {
        dim,
        since_ts: Some(since_ts.unwrap_or(now - DEFAULT_WINDOW_SECS)),
        until_ts,
        limit: Some(limit.unwrap_or(DEFAULT_BUCKET_LIMIT)),
    };
    analytics::aggregate(&state.store, &query).map_err(|e| e.to_string())
}

/// 最近请求明细（按时间倒序）。
#[tauri::command]
pub fn usage_recent(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<RecentRequest>, String> {
    usage_recent_of(&state, limit)
}

/// 最近请求明细（可测版本）。
pub fn usage_recent_of(
    state: &AppState,
    limit: Option<i64>,
) -> Result<Vec<RecentRequest>, String> {
    analytics::recent_requests(&state.store, limit.unwrap_or(DEFAULT_RECENT_LIMIT))
        .map_err(|e| e.to_string())
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

    fn record(ts: i64) -> UsageRecord {
        UsageRecord {
            ts,
            client_key_id: Some("ck1".to_string()),
            channel_id: "c1".to_string(),
            account_id: "acct_default_c1".to_string(),
            model_alias: "alias".to_string(),
            upstream_model: "deepseek-flash".to_string(),
            protocol: "openai".to_string(),
            prompt_tokens: 100,
            completion_tokens: 200,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            estimated: false,
            ttft_ms: Some(80),
            duration_ms: 1000,
            status: UsageStatus::Success,
            error_kind: None,
        }
    }

    #[test]
    fn unknown_dimension_is_rejected() {
        let (_dir, state) = app_state();
        let err = usage_aggregate_of(&state, "nope", None, None, None).expect_err("应报错");
        assert!(err.contains("未知聚合维度"), "报错应说清维度取值：{err}");
    }

    #[test]
    fn aggregate_defaults_to_a_thirty_day_window() {
        let (_dir, state) = app_state();
        insert_usage(&state.store, &record(crate::vault::now_secs())).expect("写用量");
        let result = usage_aggregate_of(&state, "channel", None, None, None).expect("聚合");
        assert_eq!(result.buckets.len(), 1);
        assert_eq!(result.buckets[0].label, "DeepSeek 官方");
        assert_eq!(result.buckets[0].requests, 1);
        assert_eq!(result.dim, "channel");
    }

    #[test]
    fn recent_returns_rows_newest_first() {
        let (_dir, state) = app_state();
        insert_usage(&state.store, &record(100)).expect("写用量");
        insert_usage(&state.store, &record(200)).expect("写用量");
        let rows = usage_recent_of(&state, Some(10)).expect("读明细");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts, 200);
        assert_eq!(rows[0].total_tokens, 300);
    }
}
