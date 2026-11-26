//! client key 管理 IPC（02 §3.1）。
//!
//! 明文只在创建时返回一次；之后界面只能看到前缀与元信息。

use serde::Serialize;
use tauri::State;

use crate::gateway::auth;
use crate::AppState;

/// 创建 client key 后的返回（**明文仅此一次**）。
#[derive(Serialize, Clone, Debug)]
pub struct CreatedClientKey {
    pub id: String,
    /// 明文 key：只在本次返回，之后不可再取。
    pub key: String,
    /// 前 8 位，供 UI 辨认。
    pub key_prefix: String,
}

/// client key 元信息（无明文）。
#[derive(Serialize, Clone, Debug)]
pub struct ClientKeyMeta {
    pub id: String,
    pub label: String,
    pub key_prefix: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub rpm_limit: Option<i64>,
    pub daily_tokens: Option<i64>,
}

/// 创建 client key。
#[tauri::command]
pub fn client_key_create(
    state: State<'_, AppState>,
    label: String,
) -> Result<CreatedClientKey, String> {
    let key = auth::generate_client_key();
    let hash = auth::hash_client_key(&key);
    let prefix = auth::key_prefix(&key);
    let id = crate::vault::new_id("ck");
    let now = crate::vault::now_secs();

    state
        .store
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, label, hash, prefix, now],
            )?;
            Ok(())
        })
        .map_err(|e| e.to_string())?;

    // 明文只在这里出现一次，不写日志
    println!("[client_key] 已创建 {id}（明文不落日志）");
    Ok(CreatedClientKey { id, key, key_prefix: prefix })
}

/// 列出 client key（仅元信息）。
#[tauri::command]
pub fn client_key_list(state: State<'_, AppState>) -> Result<Vec<ClientKeyMeta>, String> {
    client_key_list_of(&state)
}

/// 列出 client key（可测版本）。
pub fn client_key_list_of(state: &AppState) -> Result<Vec<ClientKeyMeta>, String> {
    state
        .store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, label, key_prefix, created_at, expires_at, rpm_limit, daily_tokens
                 FROM client_key ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ClientKeyMeta {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    key_prefix: row.get(2)?,
                    created_at: row.get(3)?,
                    expires_at: row.get(4)?,
                    rpm_limit: row.get(5)?,
                    daily_tokens: row.get(6)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .map_err(|e| e.to_string())
}

/// 更新限额（`None` 表示不限）。
#[tauri::command]
pub fn client_key_update_limits(
    state: State<'_, AppState>,
    id: String,
    rpm_limit: Option<i64>,
    daily_tokens: Option<i64>,
    expires_at: Option<i64>,
) -> Result<bool, String> {
    let affected = state
        .store
        .with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE client_key SET rpm_limit = ?1, daily_tokens = ?2, expires_at = ?3 WHERE id = ?4",
                rusqlite::params![rpm_limit, daily_tokens, expires_at, id],
            )?)
        })
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}

/// 更新 client key 的标签与限额（T9；限额 `None` = 不限）。
#[tauri::command]
pub fn client_key_update_meta(
    state: State<'_, AppState>,
    id: String,
    label: String,
    rpm_limit: Option<i64>,
    daily_tokens: Option<i64>,
    expires_at: Option<i64>,
) -> Result<bool, String> {
    client_key_update_meta_of(&state, &id, &label, rpm_limit, daily_tokens, expires_at)
}

/// 更新标签与限额（可测版本）。
pub fn client_key_update_meta_of(
    state: &AppState,
    id: &str,
    label: &str,
    rpm_limit: Option<i64>,
    daily_tokens: Option<i64>,
    expires_at: Option<i64>,
) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("client key id 不能为空".to_string());
    }
    let label = if label.trim().is_empty() {
        "未命名"
    } else {
        label.trim()
    };
    // 限额不能是负数（负数会让限流判断恒真/恒假）
    if rpm_limit.is_some_and(|v| v < 0) {
        return Err("rpm_limit 不能为负数".to_string());
    }
    if daily_tokens.is_some_and(|v| v < 0) {
        return Err("daily_tokens 不能为负数".to_string());
    }
    let affected = state
        .store
        .with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE client_key SET label = ?2, rpm_limit = ?3, daily_tokens = ?4, expires_at = ?5
                 WHERE id = ?1",
                rusqlite::params![id, label, rpm_limit, daily_tokens, expires_at],
            )?)
        })
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}

/// 删除 client key。
#[tauri::command]
pub fn client_key_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let affected = state
        .store
        .with_conn(|conn| Ok(conn.execute("DELETE FROM client_key WHERE id = ?1", [id])?))
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    use crate::store::Store;
    use crate::vault::Vault;

    fn state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Arc::new(Store::open(dir.path().join("store.db")).expect("打开库"));
        let vault = Arc::new(RwLock::new(
            Vault::open(Arc::clone(&store), dir.path().join("vault.bin"), None).expect("打开保险库"),
        ));
        let state = AppState {
            gateway_addr: "127.0.0.1:8787".to_string(),
            gateway_base_url: "http://127.0.0.1:8787/v1".to_string(),
            store,
            vault,
            limiter: Arc::new(crate::gateway::ratelimit::RateLimiter::new()),
            circuit: Arc::new(crate::gateway::circuit::CircuitBreaker::new()),
        };
        (dir, state)
    }

    fn create(state: &AppState, label: &str) -> String {
        let key = auth::generate_client_key();
        let hash = auth::hash_client_key(&key);
        let prefix = auth::key_prefix(&key);
        let id = crate::vault::new_id("ck");
        let now = crate::vault::now_secs();
        state
            .store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![id, label, hash, prefix, now],
                )?;
                Ok(())
            })
            .expect("插入 client key");
        id
    }

    #[test]
    fn update_meta_changes_label_and_limits() {
        let (_dir, state) = state();
        let id = create(&state, "pi");

        assert!(
            client_key_update_meta_of(&state, &id, "pi-主力", Some(60), Some(100_000), None)
                .expect("更新失败")
        );

        let list = client_key_list_of(&state).expect("列出失败");
        let key = list.iter().find(|k| k.id == id).expect("应有该 key");
        assert_eq!(key.label, "pi-主力");
        assert_eq!(key.rpm_limit, Some(60));
        assert_eq!(key.daily_tokens, Some(100_000));
    }

    #[test]
    fn update_meta_rejects_negative_limits_and_unknown_id() {
        let (_dir, state) = state();
        let id = create(&state, "pi");
        assert!(client_key_update_meta_of(&state, &id, "x", Some(-1), None, None).is_err());
        assert!(client_key_update_meta_of(&state, &id, "x", None, Some(-1), None).is_err());
        assert!(!client_key_update_meta_of(&state, "ck-not-exist", "x", None, None, None).unwrap());
    }

    #[test]
    fn update_meta_falls_back_to_default_label() {
        let (_dir, state) = state();
        let id = create(&state, "pi");
        client_key_update_meta_of(&state, &id, "   ", None, None, None).expect("更新失败");
        let list = client_key_list_of(&state).expect("列出失败");
        assert_eq!(list[0].label, "未命名");
    }
}
