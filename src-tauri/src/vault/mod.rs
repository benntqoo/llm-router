//! 加密保险库（02 §5）。
//!
//! 密钥层次：
//! ```text
//! OS 原生保护（DPAPI / Keychain）或 Argon2id(主密码)  ──派生/保护──► KEK
//!     └── AES-256-GCM wrap ──► MasterKey (32B 随机)
//!         └── AES-256-GCM（每条独立 12B nonce + AAD=凭证 id）──► credential.ciphertext
//! ```
//!
//! 落盘：`%APPDATA%\llm-router\vault.bin`（JSON，内容全部为密文）。
//! 约束：
//! - 解密只在内存中进行，不写临时文件；
//! - 敏感缓冲（主密钥 / KEK / 明文）离开作用域自动清零；
//! - 轮换崩溃安全：新密钥先落盘、旧密钥留作 prev，再逐条重加密。

pub mod crypto;
pub mod oskey;
pub mod redact;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::store::Store;
use crypto::{SecretBytes, KEY_LEN};
use oskey::OsKeyError;

/// 保险库文件名。
pub const VAULT_FILE_NAME: &str = "vault.bin";

/// 主密码模式的 Argon2id 默认参数（02 §5.1：m=64MiB、t=3、p=1；m 以 KiB 计）。
pub const DEFAULT_ARGON2_M_COST: u32 = 65_536;
pub const DEFAULT_ARGON2_T_COST: u32 = 3;
pub const DEFAULT_ARGON2_P_COST: u32 = 1;

/// 保险库错误。
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("保险库文件操作失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("保险库数据格式错误: {0}")]
    Format(String),

    #[error("保险库加解密失败: {0}")]
    Crypto(#[from] crypto::CryptoError),

    #[error("OS 原生保护失败: {0}")]
    OsKey(#[from] OsKeyError),

    #[error("数据库操作失败: {0}")]
    Store(#[from] crate::store::StoreError),

    #[error("该保险库为主密码模式，需要提供主密码")]
    PasswordRequired,

    #[error("主密码错误或保险库已损坏")]
    PasswordMismatch,

    #[error("凭证不存在: {0}")]
    CredentialNotFound(String),

    #[error("保险库已锁定：需要先解锁（输入主密码）")]
    Locked,

    #[error("缺少可用主密钥版本: {0}")]
    KeyVersionUnavailable(i64),
}

/// 保险库结果别名。
pub type Result<T> = std::result::Result<T, VaultError>;

/// KEK 的来源。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KdfSpec {
    /// OS 原生保护：Windows DPAPI / macOS Keychain。
    Os {
        /// OS 保护后的 KEK（base64）
        os_blob: String,
    },
    /// 主密码模式：Argon2id 派生 KEK。
    Argon2id {
        /// 盐（base64）
        salt: String,
        m_cost: u32,
        t_cost: u32,
        p_cost: u32,
    },
}

/// 被 KEK 包裹的主密钥。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WrappedKey {
    pub kdf: KdfSpec,
    /// 包裹时使用的 nonce（base64）
    pub kek_nonce: String,
    /// AES-GCM(KEK, MasterKey) 密文（base64）
    pub wrapped_key: String,
}

/// `vault.bin` 的文件内容。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VaultFile {
    /// 文件格式版本
    pub format_version: u32,
    /// 当前主密钥版本（轮换递增）
    pub key_version: i64,
    /// 当前主密钥
    pub key: WrappedKey,
    /// 上一个主密钥（轮换期间保留，用于崩溃恢复）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_key: Option<WrappedKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_key_version: Option<i64>,
}

/// 凭证元信息（**不含任何明文或密文**）。
#[derive(Serialize, Clone, Debug)]
pub struct CredentialMeta {
    pub id: String,
    pub channel_id: String,
    pub kind: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
    pub key_version: i64,
}

/// 保险库句柄：持有主密钥与数据层。
///
/// 主密码模式下，进程启动时 `master_key` 为 `None`（保险库处于锁定态），
/// 需要用户输入主密码调用 `unlock` 后才能读写凭证。
pub struct Vault {
    master_key: Option<SecretBytes>,
    prev_master_key: Option<SecretBytes>,
    key_version: i64,
    file: VaultFile,
    path: PathBuf,
    store: Arc<Store>,
}

impl Vault {
    /// 打开保险库；文件不存在时以 OS 原生保护模式创建。
    ///
    /// `master_password` 仅在主密码模式下需要。
    pub fn open(
        store: Arc<Store>,
        path: impl AsRef<Path>,
        master_password: Option<&str>,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if path.exists() {
            Self::load(store, path, master_password)
        } else {
            Self::create(store, path, KdfMode::Os, master_password)
        }
    }

    /// 以主密码模式创建保险库（新建或覆盖已有文件）。
    pub fn create_with_password(
        store: Arc<Store>,
        path: impl AsRef<Path>,
        password: &str,
    ) -> Result<Self> {
        Self::create(store, path.as_ref().to_path_buf(), KdfMode::Password, Some(password))
    }

    fn create(
        store: Arc<Store>,
        path: PathBuf,
        mode: KdfMode,
        password: Option<&str>,
    ) -> Result<Self> {
        let master_key = SecretBytes::random(KEY_LEN);
        let wrapped = match mode {
            KdfMode::Os => wrap_with_os(&master_key)?,
            KdfMode::Password => {
                let password = password.ok_or(VaultError::PasswordRequired)?;
                wrap_with_password(&master_key, password)?
            }
        };
        let file = VaultFile {
            format_version: 1,
            key_version: 1,
            key: wrapped,
            prev_key: None,
            prev_key_version: None,
        };
        let vault = Vault {
            master_key: Some(master_key),
            prev_master_key: None,
            key_version: file.key_version,
            file,
            path: path.clone(),
            store,
        };
        vault.write_file(&vault.file)?;
        Ok(vault)
    }

    fn load(store: Arc<Store>, path: PathBuf, password: Option<&str>) -> Result<Self> {
        let raw = std::fs::read_to_string(&path)?;
        let file: VaultFile = serde_json::from_str(&raw)
            .map_err(|e| VaultError::Format(format!("vault.bin 解析失败: {e}")))?;
        if file.format_version != 1 {
            return Err(VaultError::Format(format!(
                "不支持的保险库格式版本: {}",
                file.format_version
            )));
        }
        // 主密码模式且未提供密码 → 保险库处于锁定态，等待用户解锁
        let (master_key, prev_master_key) = match &file.key.kdf {
            KdfSpec::Os { .. } => (
                Some(unwrap_key(&file.key, None)?),
                match &file.prev_key {
                    Some(prev) => Some(unwrap_key(prev, None)?),
                    None => None,
                },
            ),
            KdfSpec::Argon2id { .. } => match password {
                Some(pw) => (
                    Some(unwrap_key(&file.key, Some(pw))?),
                    match &file.prev_key {
                        Some(prev) => Some(unwrap_key(prev, Some(pw))?),
                        None => None,
                    },
                ),
                None => (None, None),
            },
        };
        Ok(Vault {
            master_key,
            prev_master_key,
            key_version: file.key_version,
            file,
            path,
            store,
        })
    }

    /// 保险库是否处于锁定态（主密码模式且尚未解锁）。
    pub fn is_locked(&self) -> bool {
        self.master_key.is_none()
    }

    /// 用主密码解锁保险库。
    pub fn unlock(&mut self, password: &str) -> Result<()> {
        let raw = std::fs::read_to_string(&self.path)?;
        let file: VaultFile = serde_json::from_str(&raw)
            .map_err(|e| VaultError::Format(format!("vault.bin 解析失败: {e}")))?;
        self.master_key = Some(unwrap_key(&file.key, Some(password))?);
        self.prev_master_key = match &file.prev_key {
            Some(prev) => Some(unwrap_key(prev, Some(password))?),
            None => None,
        };
        self.key_version = file.key_version;
        self.file = file;
        Ok(())
    }

    /// 切换到主密码模式（重新用主密码包裹当前主密钥）。
    ///
    /// 切换后进程内主密钥仍然可用；**下次启动**需输入主密码解锁。
    pub fn enable_password_mode(&mut self, password: &str) -> Result<()> {
        let master = self
            .master_key
            .as_ref()
            .ok_or(VaultError::Locked)?
            .clone();
        let mut file = self.file.clone();
        file.key = wrap_with_password(&master, password)?;
        file.prev_key = None;
        file.prev_key_version = None;
        self.write_file(&file)?;
        self.file = file;
        Ok(())
    }

    /// 关闭主密码模式（回到 OS 原生保护）。
    pub fn disable_password_mode(&mut self) -> Result<()> {
        let master = self
            .master_key
            .as_ref()
            .ok_or(VaultError::Locked)?
            .clone();
        let mut file = self.file.clone();
        file.key = wrap_with_os(&master)?;
        file.prev_key = None;
        file.prev_key_version = None;
        self.write_file(&file)?;
        self.file = file;
        Ok(())
    }

    /// 保险库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 数据层句柄。
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// 当前主密钥版本。
    pub fn key_version(&self) -> i64 {
        self.key_version
    }

    /// 是否为主密码模式。
    pub fn is_password_mode(&self) -> bool {
        matches!(self.file.key.kdf, KdfSpec::Argon2id { .. })
    }

    /// 确保存在同名渠道并返回其 id。
    ///
    /// `credential.channel_id` 有外键约束，凭证必须挂到某个渠道上。
    /// 这里按名称 upsert 一条占位渠道（`provider = 'custom'`），
    /// T8/T9 接管渠道管理后由正规渠道 CRUD 取代。
    pub fn ensure_channel(&self, channel_name: &str) -> Result<String> {
        let name = channel_name.trim();
        if name.is_empty() {
            return Err(VaultError::Format("渠道名不能为空".to_string()));
        }
        let existing: Option<String> = self.store.with_conn(|conn| {
            Ok(conn
                .query_row("SELECT id FROM channel WHERE name = ?1", [name], |row| {
                    row.get::<_, String>(0)
                })
                .ok())
        })?;
        if let Some(id) = existing {
            return Ok(id);
        }
        let id = new_id("ch");
        let now = now_secs();
        self.store.with_conn(|conn| {
            conn.execute(
                "INSERT INTO channel (id, name, kind, provider, base_url, enabled, priority, created_at)
                 VALUES (?1, ?2, 'api_key', 'custom', '', 1, 100, ?3)",
                rusqlite::params![id, name, now],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    /// 存入一条凭据（加密后落库），返回凭据 id。
    pub fn put_credential(
        &self,
        channel_id: &str,
        kind: &str,
        secret: &SecretBytes,
    ) -> Result<String> {
        self.put_credential_expiring(channel_id, kind, secret, None)
    }

    /// 存入一条凭据（**可带过期时间**），返回凭据 id。
    ///
    /// `expires_at` 是 Unix 秒（OAuth 型 token 用；`None` = 长期有效）。
    /// 密文与 AAD 绑定规则**同** `put_credential`（AAD = 凭据 id）。
    pub fn put_credential_expiring(
        &self,
        channel_id: &str,
        kind: &str,
        secret: &SecretBytes,
        expires_at: Option<i64>,
    ) -> Result<String> {
        let id = new_id("cred");
        // AAD 绑定凭证 id：密文无法在记录之间搬运
        let key = self.master_key.as_ref().ok_or(VaultError::Locked)?;
        let (ciphertext, nonce) =
            crypto::encrypt(key.as_bytes(), secret.as_bytes(), id.as_bytes())?;
        let now = now_secs();
        self.store.with_conn(|conn| {
            conn.execute(
                "INSERT INTO credential
                     (id, channel_id, kind, ciphertext, nonce, key_version, expires_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    id,
                    channel_id,
                    kind,
                    ciphertext,
                    nonce,
                    self.key_version,
                    expires_at,
                    now,
                    now
                ],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    /// 取出并解密一条凭据。
    pub fn get_credential(&self, id: &str) -> Result<SecretBytes> {
        let row = self.store.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT ciphertext, nonce, key_version FROM credential WHERE id = ?1")?;
            let mut rows = stmt.query([id])?;
            match rows.next()? {
                Some(row) => Ok(Some((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))),
                None => Ok(None),
            }
        })?;
        let Some((ciphertext, nonce, key_version)) = row else {
            return Err(VaultError::CredentialNotFound(id.to_string()));
        };
        let key = self.key_for_version(key_version)?;
        crypto::decrypt(key.as_bytes(), &nonce, &ciphertext, id.as_bytes()).map_err(VaultError::from)
    }

    /// 列出凭据元信息（不含密文与明文）。
    pub fn list_credentials(&self, channel_id: Option<&str>) -> Result<Vec<CredentialMeta>> {
        let items = self
            .store
            .with_conn(|conn| -> crate::store::Result<Vec<CredentialMeta>> {
                let mut out = Vec::new();
                let push = |row: &rusqlite::Row<'_>| -> rusqlite::Result<CredentialMeta> {
                    Ok(CredentialMeta {
                        id: row.get(0)?,
                        channel_id: row.get(1)?,
                        kind: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                        expires_at: row.get(5)?,
                        key_version: row.get(6)?,
                    })
                };
                match channel_id {
                    Some(channel) => {
                        let mut stmt = conn.prepare(
                            "SELECT id, channel_id, kind, created_at, updated_at, expires_at, key_version
                             FROM credential WHERE channel_id = ?1 ORDER BY created_at DESC",
                        )?;
                        let rows = stmt.query_map([channel], push)?;
                        for row in rows {
                            out.push(row?);
                        }
                    }
                    None => {
                        let mut stmt = conn.prepare(
                            "SELECT id, channel_id, kind, created_at, updated_at, expires_at, key_version
                             FROM credential ORDER BY created_at DESC",
                        )?;
                        let rows = stmt.query_map([], push)?;
                        for row in rows {
                            out.push(row?);
                        }
                    }
                }
                Ok(out)
            })?;
        Ok(items)
    }

    /// 删除一条凭据。
    pub fn delete_credential(&self, id: &str) -> Result<bool> {
        let affected = self.store.with_conn(|conn| {
            Ok(conn.execute("DELETE FROM credential WHERE id = ?1", [id])?)
        })?;
        Ok(affected > 0)
    }

    /// 轮换主密钥：逐条重新加密。
    ///
    /// 顺序保证崩溃安全：先落盘（新密钥为主、旧密钥为 prev），再逐条重加密；
    /// 中途崩溃时，残留记录按其 `key_version` 用对应密钥解密。
    pub fn rotate_master_key(&mut self, master_password: Option<&str>) -> Result<i64> {
        let new_master = SecretBytes::random(KEY_LEN);
        let new_wrapped = match &self.file.key.kdf {
            KdfSpec::Os { .. } => wrap_with_os(&new_master)?,
            KdfSpec::Argon2id { .. } => {
                let password = master_password.ok_or(VaultError::PasswordRequired)?;
                wrap_with_password(&new_master, password)?
            }
        };

        let mut new_file = self.file.clone();
        new_file.prev_key = Some(self.file.key.clone());
        new_file.prev_key_version = Some(self.key_version);
        new_file.key = new_wrapped;
        new_file.key_version = self.key_version + 1;
        self.write_file(&new_file)?;

        // 逐条重加密（全部记录都过一遍，按各自的 key_version 取对应密钥）
        let rows: Vec<(String, Vec<u8>, Vec<u8>, i64)> = self.store.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ciphertext, nonce, key_version FROM credential ORDER BY created_at",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                ));
            }
            Ok(out)
        })?;

        for (id, ciphertext, nonce, key_version) in rows {
            let old_key = self.key_for_version(key_version)?;
            let plaintext = crypto::decrypt(old_key.as_bytes(), &nonce, &ciphertext, id.as_bytes())?;
            let (new_ciphertext, new_nonce) =
                crypto::encrypt(new_master.as_bytes(), plaintext.as_bytes(), id.as_bytes())?;
            let now = now_secs();
            self.store.with_conn(|conn| {
                conn.execute(
                    "UPDATE credential SET ciphertext = ?1, nonce = ?2, key_version = ?3, updated_at = ?4
                     WHERE id = ?5",
                    rusqlite::params![new_ciphertext, new_nonce, new_file.key_version, now, id],
                )?;
                Ok(())
            })?;
        }

        // 内存态同步到新版本（旧密钥保留为 prev，用于崩溃恢复与迟到的写入）
        self.prev_master_key = self.master_key.replace(new_master);
        self.key_version = new_file.key_version;
        self.file = new_file;
        Ok(self.key_version)
    }

    /// 导出加密备份：`vault.bin` + 数据库文件（全部为密文）。
    ///
    /// 返回写出的文件路径。
    pub fn export_encrypted_backup(&self, target_dir: &Path) -> Result<Vec<PathBuf>> {
        std::fs::create_dir_all(target_dir)?;
        let mut written = Vec::new();

        let vault_copy = target_dir.join(VAULT_FILE_NAME);
        std::fs::copy(&self.path, &vault_copy)?;
        written.push(vault_copy);

        let db_path = self.store.path().to_path_buf();
        if db_path.exists() && db_path.to_string_lossy() != ":memory:" {
            let db_copy = target_dir.join("store.db");
            std::fs::copy(&db_path, &db_copy)?;
            written.push(db_copy);
        }
        Ok(written)
    }

    /// 依据版本号取主密钥。
    fn key_for_version(&self, key_version: i64) -> Result<&SecretBytes> {
        if key_version == self.key_version {
            return self.master_key.as_ref().ok_or(VaultError::Locked);
        }
        if let (Some(prev), Some(prev_version)) = (&self.prev_master_key, self.file.prev_key_version) {
            if key_version == prev_version {
                return Ok(prev);
            }
        }
        Err(VaultError::KeyVersionUnavailable(key_version))
    }

    /// 原子写入保险库文件（临时文件 + rename）。
    fn write_file(&self, file: &VaultFile) -> Result<()> {
        let json = serde_json::to_string_pretty(file)
            .map_err(|e| VaultError::Format(format!("序列化失败: {e}")))?;
        let tmp = self.path.with_extension("bin.tmp");
        std::fs::write(&tmp, json.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("key_version", &self.key_version)
            .field("locked", &self.master_key.is_none())
            .finish_non_exhaustive()
    }
}

/// KEK 模式。
enum KdfMode {
    Os,
    Password,
}

/// 用 OS 原生保护生成 KEK 并包裹主密钥。
fn wrap_with_os(master_key: &SecretBytes) -> Result<WrappedKey> {
    let kek = SecretBytes::random(KEY_LEN);
    let os_blob = oskey::protect(kek.as_bytes())?;
    let (ciphertext, nonce) = crypto::encrypt(kek.as_bytes(), master_key.as_bytes(), b"master-key")?;
    Ok(WrappedKey {
        kdf: KdfSpec::Os {
            os_blob: BASE64.encode(os_blob),
        },
        kek_nonce: BASE64.encode(nonce),
        wrapped_key: BASE64.encode(ciphertext),
    })
}

/// 用主密码（Argon2id）派生 KEK 并包裹主密钥。
fn wrap_with_password(master_key: &SecretBytes, password: &str) -> Result<WrappedKey> {
    let salt = SecretBytes::random(16);
    let kek = crypto::derive_kek_argon2id(
        password,
        salt.as_bytes(),
        DEFAULT_ARGON2_M_COST,
        DEFAULT_ARGON2_T_COST,
        DEFAULT_ARGON2_P_COST,
    )?;
    let (ciphertext, nonce) = crypto::encrypt(kek.as_bytes(), master_key.as_bytes(), b"master-key")?;
    Ok(WrappedKey {
        kdf: KdfSpec::Argon2id {
            salt: BASE64.encode(salt.as_bytes()),
            m_cost: DEFAULT_ARGON2_M_COST,
            t_cost: DEFAULT_ARGON2_T_COST,
            p_cost: DEFAULT_ARGON2_P_COST,
        },
        kek_nonce: BASE64.encode(nonce),
        wrapped_key: BASE64.encode(ciphertext),
    })
}

/// 解开 KEK 并还原主密钥。
fn unwrap_key(wrapped: &WrappedKey, password: Option<&str>) -> Result<SecretBytes> {
    let is_password_kdf = matches!(wrapped.kdf, KdfSpec::Argon2id { .. });
    let kek = match &wrapped.kdf {
        KdfSpec::Os { os_blob } => {
            let blob = BASE64
                .decode(os_blob)
                .map_err(|e| VaultError::Format(format!("os_blob base64 非法: {e}")))?;
            SecretBytes::from_slice(&oskey::unprotect(&blob)?)
        }
        KdfSpec::Argon2id {
            salt,
            m_cost,
            t_cost,
            p_cost,
        } => {
            let password = password.ok_or(VaultError::PasswordRequired)?;
            let salt = BASE64
                .decode(salt)
                .map_err(|e| VaultError::Format(format!("salt base64 非法: {e}")))?;
            crypto::derive_kek_argon2id(password, &salt, *m_cost, *t_cost, *p_cost)
                .map_err(|_| VaultError::PasswordMismatch)?
        }
    };
    let nonce = BASE64
        .decode(&wrapped.kek_nonce)
        .map_err(|e| VaultError::Format(format!("kek_nonce base64 非法: {e}")))?;
    let blob = BASE64
        .decode(&wrapped.wrapped_key)
        .map_err(|e| VaultError::Format(format!("wrapped_key base64 非法: {e}")))?;
    // 主密码错误不会在派生阶段暴露（Argon2id 对任何输入都能派生出密钥），
    // 而是表现为 GCM 认证失败——这里统一映射为"主密码错误"。
    crypto::decrypt(kek.as_bytes(), &nonce, &blob, b"master-key").map_err(|err| {
        if is_password_kdf {
            VaultError::PasswordMismatch
        } else {
            VaultError::Crypto(err)
        }
    })
}

/// 生成带前缀的唯一 id（32 位十六进制随机）。
pub fn new_id(prefix: &str) -> String {
    let raw = SecretBytes::random(16);
    let hex: String = raw
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{prefix}_{hex}")
}

/// 当前 Unix 秒。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 允许用环境变量指定保险库文件路径（开发与测试用）。
pub const ENV_VAULT_PATH: &str = "LLMR_VAULT_PATH";

/// 保险库默认路径：`%APPDATA%\llm-router\vault.bin`。
pub fn default_vault_path() -> PathBuf {
    match std::env::var_os(ENV_VAULT_PATH) {
        Some(path) => PathBuf::from(path),
        None => crate::store::app_data_dir().join(VAULT_FILE_NAME),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn temp_store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        (dir, Arc::new(store))
    }

    #[test]
    fn credential_round_trip_and_no_plaintext_in_vault_file() {
        let (dir, store) = temp_store();
        let vault_path = dir.path().join("vault.bin");
        let vault = Vault::open(store.clone(), &vault_path, None).expect("打开保险库");

        let secret = SecretBytes::from_slice(b"sk-fake-key-do-not-log-987654");
        let channel = vault.ensure_channel("DeepSeek 官方").expect("渠道");
        let id = vault
            .put_credential(&channel, "api_key", &secret)
            .expect("写入凭证");

        let restored = vault.get_credential(&id).expect("读取凭证");
        assert_eq!(restored.as_bytes(), secret.as_bytes());

        // vault.bin 不得包含明文
        let raw = std::fs::read(&vault_path).expect("读取 vault.bin");
        assert!(
            !contains(&raw, b"sk-fake-key-do-not-log-987654"),
            "vault.bin 不得包含明文"
        );
        // 数据库里也不得包含明文
        let db = std::fs::read(store.path()).expect("读取数据库");
        assert!(
            !contains(&db, b"sk-fake-key-do-not-log-987654"),
            "数据库不得包含明文"
        );
    }

    #[test]
    fn reopen_after_restart_decrypts() {
        let (dir, store) = temp_store();
        let vault_path = dir.path().join("vault.bin");
        let id = {
            let vault = Vault::open(store.clone(), &vault_path, None).expect("打开保险库");
            let channel = vault.ensure_channel("Kimi").expect("渠道");
            let secret = SecretBytes::from_slice(b"sk-restart-test-value");
            let id = vault
                .put_credential(&channel, "api_key", &secret)
                .expect("写入凭证");
            id
        };
        // 模拟重启：重新从磁盘加载保险库
        let vault = Vault::open(store, &vault_path, None).expect("重启后打开保险库");
        let restored = vault.get_credential(&id).expect("读取凭证");
        assert_eq!(restored.as_bytes(), b"sk-restart-test-value");
    }

    #[test]
    fn list_credentials_contains_no_secret_material() {
        let (dir, store) = temp_store();
        let vault = Vault::open(store, dir.path().join("vault.bin"), None).expect("打开保险库");
        let channel = vault.ensure_channel("GLM").expect("渠道");
        vault
            .put_credential(&channel, "api_key", &SecretBytes::from_slice(b"sk-list-test"))
            .expect("写入凭证");
        let list = vault.list_credentials(None).expect("列出凭证");
        assert_eq!(list.len(), 1);
        let json = serde_json::to_string(&list).expect("序列化");
        assert!(!json.contains("sk-list-test"), "元信息不得包含明文");
        assert!(!json.contains("ciphertext"));
    }

    #[test]
    fn rotate_master_key_reencrypts_and_old_key_still_works() {
        let (dir, store) = temp_store();
        let vault_path = dir.path().join("vault.bin");
        let mut vault = Vault::open(store.clone(), &vault_path, None).expect("打开保险库");
        let channel = vault.ensure_channel("MiniMax").expect("渠道");
        let id = vault
            .put_credential(&channel, "api_key", &SecretBytes::from_slice(b"sk-rotate-test"))
            .expect("写入凭证");

        let new_version = vault.rotate_master_key(None).expect("轮换主密钥");
        assert_eq!(new_version, 2);

        let restored = vault.get_credential(&id).expect("轮换后读取凭证");
        assert_eq!(restored.as_bytes(), b"sk-rotate-test");

        // 重新加载后仍能解密（旧密钥已不再是当前版本）
        let reopened = Vault::open(store, &vault_path, None).expect("重启后打开");
        let restored = reopened.get_credential(&id).expect("重启后读取凭证");
        assert_eq!(restored.as_bytes(), b"sk-rotate-test");
    }

    #[test]
    fn delete_credential_removes_row() {
        let (dir, store) = temp_store();
        let vault = Vault::open(store, dir.path().join("vault.bin"), None).expect("打开保险库");
        let channel = vault.ensure_channel("百炼").expect("渠道");
        let id = vault
            .put_credential(&channel, "api_key", &SecretBytes::from_slice(b"sk-del"))
            .expect("写入凭证");
        assert!(vault.delete_credential(&id).expect("删除"));
        assert!(vault.get_credential(&id).is_err());
    }

    #[test]
    fn password_mode_requires_password_and_rejects_wrong_one() {
        let (dir, store) = temp_store();
        let vault_path = dir.path().join("vault-pw.bin");
        let vault = Vault::create_with_password(store.clone(), &vault_path, "correct horse")
            .expect("创建主密码保险库");
        assert!(vault.is_password_mode());
        let channel = vault.ensure_channel("SiliconFlow").expect("渠道");
        vault
            .put_credential(&channel, "api_key", &SecretBytes::from_slice(b"sk-pw"))
            .expect("写入凭证");

        // 不给密码 → 保险库进入锁定态（不是报错，应用要能起来并提示解锁）
        let locked = Vault::open(store.clone(), &vault_path, None).expect("应能打开为锁定态");
        assert!(locked.is_locked());
        assert!(matches!(
            locked.get_credential(&locked.list_credentials(None).unwrap()[0].id),
            Err(VaultError::Locked)
        ));

        // 错误密码 → 认证失败
        assert!(matches!(
            Vault::open(store.clone(), &vault_path, Some("wrong")),
            Err(VaultError::PasswordMismatch)
        ));

        // 正确密码 → 解锁后可读
        let unlocked = Vault::open(store.clone(), &vault_path, Some("correct horse"))
            .expect("正确密码应能打开");
        assert!(!unlocked.is_locked());
        let id = unlocked.list_credentials(None).unwrap()[0].id.clone();
        assert_eq!(
            unlocked.get_credential(&id).unwrap().as_bytes(),
            b"sk-pw"
        );

        // 锁定态实例可用 unlock() 解锁
        let mut locked = locked;
        assert!(locked.unlock("correct horse").is_ok());
        assert!(!locked.is_locked());
        assert_eq!(locked.get_credential(&id).unwrap().as_bytes(), b"sk-pw");
        assert!(locked.unlock("wrong").is_err());
    }

    /// 在字节流中查找子串。
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
