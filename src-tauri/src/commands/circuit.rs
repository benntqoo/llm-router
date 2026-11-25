//! 熔断状态查询 IPC（02 §2.3）。
//!
//! UI 展示"当前正在冷却的渠道"：状态、原因、剩余秒数。
//! 数据来自**运行中的熔断器**（与网关共享同一实例），落库副本在 `circuit_state` 表。

use serde::Serialize;
use tauri::State;

use crate::gateway::circuit::{
    circuit_key, CircuitState, MIN_OPEN_DURATION_SECS, OPEN_DURATION_SECS,
};
use crate::store::settings::{get_int_setting, set_int_setting, KEY_CIRCUIT_OPEN_SECS};
use crate::AppState;

/// 单条熔断状态。
#[derive(Serialize, Clone, Debug)]
pub struct CircuitStatusRow {
    /// 渠道 id。
    pub channel_id: String,
    /// 渠道名（渠道被删时回退为 id）。
    pub channel_name: String,
    /// 上游模型名。
    pub upstream_model: String,
    /// 状态：`open`（熔断中）/ `half_open`（冷却已过，等一次探测）。
    pub state: String,
    /// 打开原因：`failures`（连续失败）/ `rate_limit`（上游限流）。
    pub reason: Option<String>,
    /// 连续失败次数。
    pub failures: u32,
    /// 429 命中次数。
    pub rate_limit_hits: u32,
    /// 冷却截止（Unix 秒）。
    pub open_until: Option<i64>,
    /// 剩余冷却秒数。
    pub remaining_secs: Option<i64>,
}

/// 列出当前处于熔断/冷却的 `(channel, upstream_model)`。
#[tauri::command]
pub fn circuit_status(state: State<'_, AppState>) -> Result<Vec<CircuitStatusRow>, String> {
    circuit_status_of(&state)
}

/// 熔断状态查询（可测版本：不依赖 Tauri 的 `State` 包装）。
pub fn circuit_status_of(state: &AppState) -> Result<Vec<CircuitStatusRow>, String> {
    let now = crate::vault::now_secs();
    let snapshots = state.circuit.active_snapshots();
    let mut rows = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let channel_name = state
            .store
            .with_conn(|conn| {
                let name: Option<String> = conn
                    .query_row(
                        "SELECT name FROM channel WHERE id = ?1",
                        [&snapshot.channel_id],
                        |row| row.get(0),
                    )
                    .ok();
                Ok(name.unwrap_or_else(|| snapshot.channel_id.clone()))
            })
            .map_err(|e| e.to_string())?;
        rows.push(CircuitStatusRow {
            channel_id: snapshot.channel_id.clone(),
            channel_name,
            upstream_model: snapshot.upstream_model.clone(),
            state: snapshot.state.as_str().to_string(),
            reason: snapshot.reason.map(|reason| reason.as_str().to_string()),
            failures: snapshot.failures,
            rate_limit_hits: snapshot.rate_limit_hits,
            open_until: snapshot.open_until,
            remaining_secs: snapshot.open_until.map(|until| (until - now).max(0)),
        });
    }
    Ok(rows)
}

/// 熔断设置。
#[derive(Serialize, Clone, Debug)]
pub struct CircuitSettings {
    /// 连续失败触发的熔断时长（秒；默认 60，最小 1）。
    pub open_duration_secs: i64,
}

/// 读熔断设置。
#[tauri::command]
pub fn circuit_settings_get(state: State<'_, AppState>) -> Result<CircuitSettings, String> {
    circuit_settings_get_of(&state)
}

/// 熔断设置（可测版本）。
pub fn circuit_settings_get_of(state: &AppState) -> Result<CircuitSettings, String> {
    let open_duration_secs = get_int_setting(&state.store, KEY_CIRCUIT_OPEN_SECS)
        .unwrap_or(OPEN_DURATION_SECS)
        .max(MIN_OPEN_DURATION_SECS);
    Ok(CircuitSettings { open_duration_secs })
}

/// 写熔断设置（秒；最小 1 秒）。
#[tauri::command]
pub fn circuit_settings_set(
    state: State<'_, AppState>,
    open_duration_secs: i64,
) -> Result<CircuitSettings, String> {
    circuit_settings_set_of(&state, open_duration_secs)
}

/// 写熔断设置（可测版本）：落库 + 立即生效。
pub fn circuit_settings_set_of(
    state: &AppState,
    open_duration_secs: i64,
) -> Result<CircuitSettings, String> {
    // 先钳到下限再落库，保证库里的值与运行时一致
    let clamped = state.circuit.set_open_duration_secs(open_duration_secs);
    set_int_setting(&state.store, KEY_CIRCUIT_OPEN_SECS, clamped).map_err(|e| e.to_string())?;
    Ok(CircuitSettings {
        open_duration_secs: clamped,
    })
}

/// 手动解除熔断（清内存状态 + 删落库行）。
#[tauri::command]
pub fn circuit_reset(
    state: State<'_, AppState>,
    channel_id: String,
    upstream_model: String,
) -> Result<bool, String> {
    circuit_reset_of(&state, &channel_id, &upstream_model)
}

/// 手动解除熔断（可测版本）；返回是否确实解除了一个熔断。
pub fn circuit_reset_of(
    state: &AppState,
    channel_id: &str,
    upstream_model: &str,
) -> Result<bool, String> {
    let key = circuit_key(channel_id, upstream_model);
    let existed = state.circuit.state(&key) != CircuitState::Closed;
    state.circuit.reset(&key);
    Ok(existed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    use crate::gateway::circuit::CircuitBreaker;
    use crate::gateway::circuit_store::StoreCircuitPersistence;
    use crate::gateway::ratelimit::RateLimiter;
    use crate::store::Store;
    use crate::vault::Vault;
    /// 构造一个带渠道的 `AppState`（只用到熔断与数据层）。
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

    #[test]
    fn empty_state_lists_nothing() {
        let (_dir, state) = app_state();
        let rows = circuit_status_of(&state).expect("查询失败");
        assert!(rows.is_empty(), "没有熔断时应返回空列表");
    }

    #[test]
    fn open_circuit_is_listed_with_channel_name_and_reason() {
        let (_dir, state) = app_state();
        let key = crate::gateway::circuit::circuit_key("c1", "deepseek-chat");
        for _ in 0..3 {
            state.circuit.on_failure(&key);
        }

        let rows = circuit_status_of(&state).expect("查询失败");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].channel_id, "c1");
        assert_eq!(rows[0].channel_name, "DeepSeek 官方");
        assert_eq!(rows[0].upstream_model, "deepseek-chat");
        assert_eq!(rows[0].state, "open");
        assert_eq!(rows[0].reason.as_deref(), Some("failures"));
        assert_eq!(rows[0].failures, 3);
        assert!(
            rows[0].remaining_secs.unwrap_or(0) > 0,
            "应给出剩余冷却秒数"
        );
    }

    #[test]
    fn rate_limit_circuit_is_listed_with_rate_limit_reason() {
        let (_dir, state) = app_state();
        let key = crate::gateway::circuit::circuit_key("c1", "deepseek-chat");
        state
            .circuit
            .on_rate_limited(&key, Some(std::time::Duration::from_secs(5)));

        let rows = circuit_status_of(&state).expect("查询失败");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reason.as_deref(), Some("rate_limit"));
        assert_eq!(rows[0].rate_limit_hits, 1);
    }

    #[test]
    fn closed_circuit_is_not_listed() {
        let (_dir, state) = app_state();
        let key = crate::gateway::circuit::circuit_key("c1", "deepseek-chat");
        for _ in 0..3 {
            state.circuit.on_failure(&key);
        }
        assert_eq!(circuit_status_of(&state).expect("查询失败").len(), 1);

        state.circuit.on_success(&key);
        assert!(
            circuit_status_of(&state).expect("查询失败").is_empty(),
            "回到 closed 后不应再展示"
        );
    }

    #[test]
    fn settings_default_to_sixty_seconds() {
        let (_dir, state) = app_state();
        let settings = circuit_settings_get_of(&state).expect("读取失败");
        assert_eq!(settings.open_duration_secs, 60);
    }

    #[test]
    fn settings_are_persisted_and_applied_immediately() {
        let (_dir, state) = app_state();
        let saved = circuit_settings_set_of(&state, 180).expect("写入失败");
        assert_eq!(saved.open_duration_secs, 180);
        assert_eq!(state.circuit.open_duration_secs(), 180, "运行时立即生效");

        // 再读一次应来自落库的值
        let reread = circuit_settings_get_of(&state).expect("读取失败");
        assert_eq!(reread.open_duration_secs, 180);
    }

    #[test]
    fn settings_below_one_second_are_clamped() {
        let (_dir, state) = app_state();
        let saved = circuit_settings_set_of(&state, 0).expect("写入失败");
        assert_eq!(saved.open_duration_secs, 1);
        let saved = circuit_settings_set_of(&state, -5).expect("写入失败");
        assert_eq!(saved.open_duration_secs, 1);
    }

    #[test]
    fn manual_reset_clears_circuit_and_its_row() {
        let (_dir, state) = app_state();
        let key = crate::gateway::circuit::circuit_key("c1", "deepseek-chat");
        for _ in 0..3 {
            state.circuit.on_failure(&key);
        }
        assert_eq!(circuit_status_of(&state).expect("查询失败").len(), 1);

        let existed = circuit_reset_of(&state, "c1", "deepseek-chat").expect("解除失败");
        assert!(existed, "确实存在熔断时应返回 true");
        assert!(
            circuit_status_of(&state).expect("查询失败").is_empty(),
            "解除后不应再展示"
        );

        // 落库行也应被清掉（重启不会复活）
        let rows = state
            .store
            .with_conn(|conn| {
                let count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM circuit_state", [], |row| row.get(0))?;
                Ok(count)
            })
            .expect("查询失败");
        assert_eq!(rows, 0, "手动解除应删除落库行");
    }

    #[test]
    fn manual_reset_on_healthy_circuit_reports_nothing_to_clear() {
        let (_dir, state) = app_state();
        let existed = circuit_reset_of(&state, "c1", "deepseek-chat").expect("解除失败");
        assert!(!existed, "没有熔断时应返回 false");
    }
}
