//! 数据层：SQLite（WAL）+ 版本化迁移。
//!
//! 依据 `docs/planning/01-architecture-and-feasibility.md` §4.4 与
//! `docs/planning/02-technical-design.md` §9：表结构全量落库，密文边界由 T2 保险库负责。
//!
//! 目录分工：
//! - `schema.rs` —— DDL 唯一权威载体（改表结构先改这里）；
//! - `migrations.rs` —— 迁移框架（版本记录、up/down 编排）；
//! - `mod.rs` —— `Store`（打开、PRAGMA、迁移入口、路径解析）。

pub mod error;
pub mod migrations;
pub mod schema;
pub mod settings;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

pub use error::{Result, StoreError};

use migrations::{MIGRATIONS, MIGRATIONS_TABLE};

/// 数据库文件默认名。
pub const DB_FILE_NAME: &str = "store.db";

/// 允许用环境变量指定数据库文件路径（开发与测试用）。
pub const ENV_STORE_PATH: &str = "LLMR_STORE_PATH";

/// 应用数据目录：Windows `%APPDATA%\llm-router`，
/// macOS `~/Library/Application Support/llm-router`，Linux `$XDG_DATA_HOME/llm-router`。
///
/// 与 02 §5 的 `vault.bin` 同目录（保险库任务 T2 复用本函数）。
pub fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("llm-router")
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        home.join("Library")
            .join("Application Support")
            .join("llm-router")
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".local")
                    .join("share")
            });
        base.join("llm-router")
    }
}

/// 数据库默认路径：`%APPDATA%\llm-router\store.db`（可用 `LLMR_STORE_PATH` 覆盖）。
pub fn default_db_path() -> PathBuf {
    match std::env::var_os(ENV_STORE_PATH) {
        Some(path) => PathBuf::from(path),
        None => app_data_dir().join(DB_FILE_NAME),
    }
}

/// 存储句柄：持有一条连接与库文件路径。
///
/// 单进程本地应用，先用一把互斥锁串行化访问；后续若出现并发写瓶颈再评估连接池。
/// 注意：axum 处理函数里调用数据库应走 `spawn_blocking`，避免阻塞 tokio 工作线程。
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl Store {
    /// 打开数据库文件（不存在则创建），设置 PRAGMA，并执行迁移到最新版本。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(&path)?;
        let store = Self::from_connection(conn, path)?;
        store.migrate_up()?;
        Ok(store)
    }

    /// 打开内存库（测试用；不支持 WAL）。
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self::from_connection(conn, PathBuf::from(":memory:"))?;
        store.migrate_up()?;
        Ok(store)
    }

    /// 由已有连接构造（负责 PRAGMA 设置，不做迁移）。
    fn from_connection(conn: Connection, path: PathBuf) -> Result<Self> {
        // WAL：持久化到库文件；内存库会被 SQLite 自动降级为 memory 模式。
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| StoreError::Migration(format!("启用 WAL 失败: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| StoreError::Migration(format!("设置 synchronous 失败: {e}")))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| StoreError::Migration(format!("开启外键约束失败: {e}")))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| StoreError::Migration(format!("设置 busy_timeout 失败: {e}")))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    /// 取得连接句柄（调用方负责加锁）。
    pub fn conn(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// 在锁保护下执行一段数据库操作。
    ///
    /// 注意：`Mutex` 不可重入，闭包里不要再调用本类型的方法。
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.lock();
        f(&guard)
    }

    /// 库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 应用所有未应用的迁移。
    pub fn migrate_up(&self) -> Result<()> {
        let conn = self.lock();
        migrations::migrate_up(&conn)?;
        Ok(())
    }

    /// 回滚最近一条迁移。
    pub fn migrate_down(&self) -> Result<Option<i64>> {
        let conn = self.lock();
        migrations::migrate_down(&conn)
    }

    /// 回滚到指定版本（不含）。
    pub fn migrate_down_to(&self, target_version: i64) -> Result<Vec<i64>> {
        let conn = self.lock();
        migrations::migrate_down_to(&conn, target_version)
    }

    /// 当前 schema 版本。
    pub fn version(&self) -> Result<i64> {
        let conn = self.lock();
        migrations::current_version(&conn)
    }

    /// 迁移总数（内置迁移条数）。
    pub fn migration_count() -> i64 {
        MIGRATIONS.len() as i64
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 版本表名（对外只读暴露，便于诊断命令）。
pub fn migrations_table_name() -> &'static str {
    MIGRATIONS_TABLE
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_list(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("准备查询失败");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("执行查询失败");
        rows.map(|r| r.expect("读取行失败")).collect()
    }

    #[test]
    fn migrate_up_creates_all_v1_tables() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let conn = store.lock();
        let tables = table_list(&conn);
        for expected in schema::ALL_TABLES {
            assert!(
                tables.iter().any(|t| t == expected),
                "缺少表 {expected}，实际表：{tables:?}"
            );
        }
        assert!(tables.iter().any(|t| t == MIGRATIONS_TABLE));
        // 注意：此处已持有连接锁，不能再调用 store.version()（Mutex 不可重入）
        assert_eq!(
            migrations::current_version(&conn).expect("读取版本失败"),
            Store::migration_count(),
            "迁移后版本应等于内置迁移条数"
        );
    }

    #[test]
    fn migrate_up_is_idempotent() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let version = store.version().expect("读取版本失败");
        store.migrate_up().expect("重复迁移应成功");
        assert_eq!(store.version().expect("读取版本失败"), version);
    }

    #[test]
    fn migrate_down_drops_tables_step_by_step() {
        let store = Store::open_in_memory().expect("打开内存库失败");

        // 逐条回滚到底（版本倒序：N, N-1, ... 1）
        let mut dropped = 0;
        while let Some(version) = store.migrate_down().expect("回滚失败") {
            dropped += 1;
            assert_eq!(
                version,
                Store::migration_count() - dropped as i64 + 1,
                "回滚版本应倒序递减"
            );
        }
        assert_eq!(dropped as i64, Store::migration_count());
        assert_eq!(store.version().expect("读取版本失败"), 0);

        let conn = store.lock();
        let tables = table_list(&conn);
        for expected in schema::ALL_TABLES {
            assert!(
                !tables.iter().any(|t| t == expected),
                "回滚后不应残留表 {expected}"
            );
        }
        // 版本表由框架维护，始终保留
        assert!(tables.iter().any(|t| t == MIGRATIONS_TABLE));
    }

    #[test]
    fn migrate_down_to_rolls_back_partially() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let reverted = store.migrate_down_to(2).expect("回滚失败");
        assert_eq!(reverted, vec![6, 5, 4, 3], "应回滚版本 6、5、4 与 3");
        assert_eq!(store.version().expect("读取版本失败"), 2);

        let conn = store.lock();
        let tables = table_list(&conn);
        assert!(tables.iter().any(|t| t == "channel"), "核心表应保留");
        assert!(!tables.iter().any(|t| t == "usage_log"), "用量表应已回滚");
        assert!(!tables.iter().any(|t| t == "checkin_site"), "签到表应已回滚");
        assert!(!tables.iter().any(|t| t == "circuit_state"), "熔断表应已回滚");
        assert!(!tables.iter().any(|t| t == "app_setting"), "设置表应已回滚");

        // 回滚后重新 up 应能回到最新版本
        drop(conn);
        store.migrate_up().expect("重新迁移失败");
        assert_eq!(store.version().expect("读取版本失败"), Store::migration_count());
    }

    #[test]
    fn file_store_uses_wal_mode() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let db = dir.path().join("wal-test.db");
        let store = Store::open(&db).expect("打开文件库失败");
        let conn = store.lock();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("读取 journal_mode 失败");
        assert_eq!(mode.to_lowercase(), "wal");
        assert!(db.exists(), "数据库文件应已创建");
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let conn = store.lock();
        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("读取 foreign_keys 失败");
        assert_eq!(enabled, 1);

        // channel_id 不存在 → 外键约束应拒绝
        let result = conn.execute(
            "INSERT INTO model (id, channel_id, upstream_id, alias) VALUES ('m1', 'missing', 'x', 'a')",
            [],
        );
        assert!(result.is_err(), "外键约束应生效");
    }

    #[test]
    fn model_alias_is_globally_unique() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let conn = store.lock();
        conn.execute(
            "INSERT INTO channel (id, name, kind, provider, base_url, created_at)
             VALUES ('c1', 'DeepSeek 官方', 'api_key', 'deepseek', 'https://api.deepseek.com', 0)",
            [],
        )
        .expect("插入渠道失败");
        conn.execute(
            "INSERT INTO model (id, channel_id, upstream_id, alias) VALUES ('m1', 'c1', 'deepseek-chat', 'chat')",
            [],
        )
        .expect("插入模型失败");
        let dup = conn.execute(
            "INSERT INTO model (id, channel_id, upstream_id, alias) VALUES ('m2', 'c1', 'deepseek-reasoner', 'chat')",
            [],
        );
        assert!(dup.is_err(), "别名必须全局唯一");
    }

    #[test]
    fn account_has_meta_json_column_for_session_channels() {
        // 02 §9：account 增加 meta_json（WorkBuddy 登录态路径引用）
        let store = Store::open_in_memory().expect("打开内存库失败");
        let conn = store.lock();
        conn.execute(
            "INSERT INTO channel (id, name, kind, provider, base_url, created_at)
             VALUES ('c1', 'WorkBuddy', 'session', 'workbuddy', 'https://example.invalid', 0)",
            [],
        )
        .expect("插入渠道失败");
        conn.execute(
            "INSERT INTO account (id, channel_id, label, quota_json, meta_json)
             VALUES ('a1', 'c1', 'user@***', '{}', '{\"users_dir\":\"C:/Users/x/.workbuddy/app/users/abc\"}')",
            [],
        )
        .expect("插入账号失败");
        let meta: String = conn
            .query_row("SELECT meta_json FROM account WHERE id = 'a1'", [], |row| {
                row.get(0)
            })
            .expect("读取 meta_json 失败");
        assert!(meta.contains("users_dir"));
    }

    #[test]
    fn opening_empty_path_creates_database_with_tables() {
        // 空库启动自动建表
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let db = dir.path().join("nested").join("store.db");
        let store = Store::open(&db).expect("打开失败");
        assert!(db.exists(), "数据库文件应已自动创建");
        let conn = store.lock();
        for expected in schema::ALL_TABLES {
            assert!(
                table_list(&conn).iter().any(|t| t == expected),
                "空库启动后应自动建表 {expected}"
            );
        }
    }

    #[test]
    fn default_db_path_is_under_app_data_dir() {
        let path = default_db_path();
        assert!(
            path.ends_with(DB_FILE_NAME),
            "默认库路径应以 {DB_FILE_NAME} 结尾，实际 {path:?}"
        );
    }
}
