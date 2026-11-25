//! 保险库 IPC 命令（02 §5.2 的 IPC 边界）。
//!
//! 边界规则：
//! - 除 `vault_reveal_secret` / `vault_export_plaintext` 外，**任何命令都不把明文送回前端**；
//! - 列表类命令只返回元信息（id / kind / 时间 / 版本），不含密文与明文；
//! - 明文导出必须显式二次确认（`confirm = true`），否则拒绝。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::vault::crypto::SecretBytes;
use crate::vault::CredentialMeta;
use crate::AppState;

/// 保险库状态。
#[derive(Serialize, Clone, Debug)]
pub struct VaultStatus {
    /// 保险库文件路径
    pub path: String,
    /// 当前主密钥版本
    pub key_version: i64,
    /// 是否处于锁定态（主密码模式且未解锁）
    pub locked: bool,
    /// 是否为主密码模式
    pub password_mode: bool,
    /// 凭证条数
    pub credential_count: usize,
}

/// 保险库状态。
#[tauri::command]
pub fn vault_status(state: State<'_, AppState>) -> Result<VaultStatus, String> {
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    let credential_count = vault.list_credentials(None).map_err(|e| e.to_string())?.len();
    Ok(VaultStatus {
        path: vault.path().display().to_string(),
        key_version: vault.key_version(),
        locked: vault.is_locked(),
        password_mode: vault.is_password_mode(),
        credential_count,
    })
}

/// 录入一条凭据（加密后落库）。
///
/// `channel_name` 不存在时会创建一条占位渠道（`provider = 'custom'`），
/// 因为 `credential.channel_id` 有外键约束。
#[tauri::command]
pub fn vault_put_secret(
    state: State<'_, AppState>,
    channel_name: String,
    kind: String,
    secret: String,
) -> Result<CredentialMeta, String> {
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    let channel_id = vault.ensure_channel(&channel_name).map_err(|e| e.to_string())?;
    let secret_bytes = SecretBytes::from_slice(secret.as_bytes());
    let id = vault
        .put_credential(&channel_id, &kind, &secret_bytes)
        .map_err(|e| e.to_string())?;
    // 不回显明文
    println!("[vault] 已保存凭据 {id}（明文不落日志）");
    let meta = vault
        .list_credentials(Some(&channel_id))
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| "凭证写入后未能读取元信息".to_string())?;
    Ok(meta)
}

/// 回看凭据明文（**唯一**向前端返回明文的命令）。
#[tauri::command]
pub fn vault_reveal_secret(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    let secret = vault.get_credential(&id).map_err(|e| e.to_string())?;
    println!("[vault] 回看凭据 {id}（明文不落日志）");
    Ok(secret.to_string_lossy())
}

/// 列出凭据元信息（无明文、无密文）。
#[tauri::command]
pub fn vault_list_secrets(
    state: State<'_, AppState>,
    channel_name: Option<String>,
) -> Result<Vec<CredentialMeta>, String> {
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    let channel_id = match channel_name {
        Some(name) if !name.trim().is_empty() => {
            Some(vault.ensure_channel(&name).map_err(|e| e.to_string())?)
        }
        _ => None,
    };
    vault
        .list_credentials(channel_id.as_deref())
        .map_err(|e| e.to_string())
}

/// 删除一条凭据。
#[tauri::command]
pub fn vault_delete_secret(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    vault.delete_credential(&id).map_err(|e| e.to_string())
}

/// 轮换主密钥（逐条重新加密）。
#[tauri::command]
pub fn vault_rotate_master_key(state: State<'_, AppState>) -> Result<i64, String> {
    let mut vault = state.vault.write().map_err(|e| e.to_string())?;
    vault.rotate_master_key(None).map_err(|e| e.to_string())
}

/// 解锁保险库（主密码模式）。
#[tauri::command]
pub fn vault_unlock(state: State<'_, AppState>, password: String) -> Result<bool, String> {
    let mut vault = state.vault.write().map_err(|e| e.to_string())?;
    vault.unlock(&password).map_err(|e| e.to_string())?;
    Ok(true)
}

/// 启用主密码模式。
#[tauri::command]
pub fn vault_enable_password_mode(
    state: State<'_, AppState>,
    password: String,
) -> Result<bool, String> {
    let mut vault = state.vault.write().map_err(|e| e.to_string())?;
    vault.enable_password_mode(&password).map_err(|e| e.to_string())?;
    Ok(true)
}

/// 关闭主密码模式。
#[tauri::command]
pub fn vault_disable_password_mode(state: State<'_, AppState>) -> Result<bool, String> {
    let mut vault = state.vault.write().map_err(|e| e.to_string())?;
    vault.disable_password_mode().map_err(|e| e.to_string())?;
    Ok(true)
}

/// 导出明文（**必须二次确认**）。
///
/// `confirm` 为 false 时直接拒绝，不做任何解密。
#[tauri::command]
pub fn vault_export_plaintext(
    state: State<'_, AppState>,
    ids: Vec<String>,
    confirm: bool,
) -> Result<String, String> {
    if !confirm {
        return Err("导出明文需二次确认：请在界面勾选确认后重试".to_string());
    }
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for id in ids {
        let secret = vault.get_credential(&id).map_err(|e| e.to_string())?;
        let meta = vault
            .list_credentials(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|m| m.id == id);
        items.push(serde_json::json!({
            "id": id,
            "kind": meta.as_ref().map(|m| m.kind.clone()).unwrap_or_default(),
            "channel_id": meta.as_ref().map(|m| m.channel_id.clone()).unwrap_or_default(),
            "secret": secret.to_string_lossy(),
        }));
    }
    Ok(serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?)
}

/// 导出加密备份（vault.bin + 数据库文件，均为密文）。
///
/// `target_dir` 为空时落到默认目录：`%APPDATA%\llm-router\backup\vault-<时间戳>`。
#[tauri::command]
pub fn vault_export_encrypted_backup(
    state: State<'_, AppState>,
    target_dir: String,
) -> Result<Vec<String>, String> {
    let vault = state.vault.read().map_err(|e| e.to_string())?;
    let dir = if target_dir.trim().is_empty() {
        crate::store::app_data_dir()
            .join("backup")
            .join(format!("vault-{}", crate::vault::now_secs()))
    } else {
        std::path::PathBuf::from(target_dir)
    };
    let written = vault
        .export_encrypted_backup(&dir)
        .map_err(|e| e.to_string())?;
    Ok(written
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>())
}

/// 便捷别名：给上层拿到保险库句柄的写锁（供后续任务使用）。
pub type SharedVault = Arc<std::sync::RwLock<crate::vault::Vault>>;
