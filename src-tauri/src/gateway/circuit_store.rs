//! 熔断状态的落库实现（02 §2.3）。
//!
//! - **写穿**：熔断状态一变就写 `circuit_state` 表（非 closed 才留行，回到 closed 即删）；
//! - **启动恢复**：应用启动时读回已落库的熔断，重启不再绕过限流；
//! - **失败不影响请求**：落库出错只记日志，不让请求失败（熔断状态丢失只是重新计数）。

use std::sync::Arc;

use crate::gateway::circuit::{CircuitPersistence, CircuitSnapshot, CircuitStateKind, OpenReason};
use crate::store::Store;

/// 基于 `Store` 的熔断状态持久化。
pub struct StoreCircuitPersistence {
    store: Arc<Store>,
}

impl std::fmt::Debug for StoreCircuitPersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreCircuitPersistence").finish_non_exhaustive()
    }
}

impl StoreCircuitPersistence {
    /// 构造（`store` 为共享句柄）。
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl CircuitPersistence for StoreCircuitPersistence {
    fn save(&self, snapshot: &CircuitSnapshot) {
        let now = crate::vault::now_secs();
        let result = self.store.with_conn(|conn| {
            conn.execute(
                "INSERT INTO circuit_state
                     (channel_id, upstream_model, state, reason, failures, rate_limit_hits, open_until, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(channel_id, upstream_model) DO UPDATE SET
                     state = excluded.state,
                     reason = excluded.reason,
                     failures = excluded.failures,
                     rate_limit_hits = excluded.rate_limit_hits,
                     open_until = excluded.open_until,
                     updated_at = excluded.updated_at",
                rusqlite::params![
                    snapshot.channel_id,
                    snapshot.upstream_model,
                    snapshot.state.as_str(),
                    snapshot.reason.map(|r| r.as_str()),
                    snapshot.failures as i64,
                    snapshot.rate_limit_hits as i64,
                    snapshot.open_until,
                    now,
                ],
            )?;
            Ok(())
        });
        if let Err(err) = result {
            // 落库失败不能影响请求；熔断状态丢失只是重新计数
            eprintln!(
                "[circuit] 熔断状态落库失败（{} / {}）：{err}",
                snapshot.channel_id, snapshot.upstream_model
            );
        }
    }

    fn remove(&self, channel_id: &str, upstream_model: &str) {
        let result = self.store.with_conn(|conn| {
            conn.execute(
                "DELETE FROM circuit_state WHERE channel_id = ?1 AND upstream_model = ?2",
                rusqlite::params![channel_id, upstream_model],
            )?;
            Ok(())
        });
        if let Err(err) = result {
            eprintln!("[circuit] 熔断状态删除失败（{channel_id} / {upstream_model}）：{err}");
        }
    }

    fn load_all(&self) -> Vec<CircuitSnapshot> {
        let result = self.store.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT channel_id, upstream_model, state, reason, failures, rate_limit_hits, open_until
                 FROM circuit_state",
            )?;
            let rows = stmt.query_map([], |row| {
                let state: String = row.get(2)?;
                let reason: Option<String> = row.get(3)?;
                Ok(CircuitSnapshot {
                    channel_id: row.get(0)?,
                    upstream_model: row.get(1)?,
                    state: CircuitStateKind::from_str(&state).unwrap_or(CircuitStateKind::Open),
                    reason: reason.as_deref().and_then(OpenReason::from_str),
                    failures: row.get::<_, i64>(4)?.max(0) as u32,
                    rate_limit_hits: row.get::<_, i64>(5)?.max(0) as u32,
                    open_until: row.get(6)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        });
        match result {
            Ok(rows) => rows,
            Err(err) => {
                eprintln!("[circuit] 读取熔断状态失败：{err}");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个带渠道的临时库（`circuit_state` 有外键，渠道必须先存在）。
    fn store_with_channel() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
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
        (dir, store)
    }

    fn snapshot(state: CircuitStateKind, reason: Option<OpenReason>) -> CircuitSnapshot {
        CircuitSnapshot {
            channel_id: "c1".to_string(),
            upstream_model: "deepseek-chat".to_string(),
            state,
            reason,
            failures: 3,
            rate_limit_hits: 0,
            open_until: Some(1_234),
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let (_dir, store) = store_with_channel();
        let persistence = StoreCircuitPersistence::new(Arc::clone(&store));

        persistence.save(&snapshot(
            CircuitStateKind::Open,
            Some(OpenReason::Failures),
        ));

        let rows = persistence.load_all();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].channel_id, "c1");
        assert_eq!(rows[0].upstream_model, "deepseek-chat");
        assert_eq!(rows[0].state, CircuitStateKind::Open);
        assert_eq!(rows[0].reason, Some(OpenReason::Failures));
        assert_eq!(rows[0].failures, 3);
        assert_eq!(rows[0].open_until, Some(1_234));
    }

    #[test]
    fn save_is_upsert_not_insert() {
        let (_dir, store) = store_with_channel();
        let persistence = StoreCircuitPersistence::new(Arc::clone(&store));

        persistence.save(&snapshot(
            CircuitStateKind::Open,
            Some(OpenReason::Failures),
        ));
        let mut second = snapshot(CircuitStateKind::HalfOpen, Some(OpenReason::RateLimit));
        second.rate_limit_hits = 2;
        persistence.save(&second);

        let rows = persistence.load_all();
        assert_eq!(rows.len(), 1, "同一 key 应覆盖而不是新增");
        assert_eq!(rows[0].state, CircuitStateKind::HalfOpen);
        assert_eq!(rows[0].reason, Some(OpenReason::RateLimit));
        assert_eq!(rows[0].rate_limit_hits, 2);
    }

    #[test]
    fn remove_deletes_the_row() {
        let (_dir, store) = store_with_channel();
        let persistence = StoreCircuitPersistence::new(Arc::clone(&store));

        persistence.save(&snapshot(
            CircuitStateKind::Open,
            Some(OpenReason::Failures),
        ));
        assert_eq!(persistence.load_all().len(), 1);

        persistence.remove("c1", "deepseek-chat");
        assert!(persistence.load_all().is_empty());
    }

    #[test]
    fn deleted_channel_clears_its_circuit_rows() {
        let (_dir, store) = store_with_channel();
        let persistence = StoreCircuitPersistence::new(Arc::clone(&store));
        persistence.save(&snapshot(
            CircuitStateKind::Open,
            Some(OpenReason::Failures),
        ));
        assert_eq!(persistence.load_all().len(), 1);

        store
            .with_conn(|conn| {
                conn.execute("DELETE FROM channel WHERE id = 'c1'", [])?;
                Ok(())
            })
            .expect("删除渠道");
        assert!(
            persistence.load_all().is_empty(),
            "渠道删除后其熔断记录应级联清除"
        );
    }

    #[test]
    fn load_all_on_empty_table_returns_empty() {
        let (_dir, store) = store_with_channel();
        let persistence = StoreCircuitPersistence::new(store);
        assert!(persistence.load_all().is_empty());
    }
}
