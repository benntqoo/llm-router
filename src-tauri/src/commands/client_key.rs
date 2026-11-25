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

/// 删除 client key。
#[tauri::command]
pub fn client_key_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let affected = state
        .store
        .with_conn(|conn| Ok(conn.execute("DELETE FROM client_key WHERE id = ?1", [id])?))
        .map_err(|e| e.to_string())?;
    Ok(affected > 0)
}
