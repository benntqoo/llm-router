//! 迁移框架：可升可降（up/down）的版本化建表流程。
//!
//! 设计约束：
//! - 版本表 `schema_migrations` 由框架自建，不属于任何业务迁移；
//! - 每个迁移自带 `up` 与 `down` SQL，`down` 按版本倒序执行；
//! - 单个迁移在事务内执行，失败即整体回滚。

use rusqlite::{Connection, OptionalExtension};

use crate::store::error::{Result, StoreError};

/// 版本记录表（框架自建）。
pub const MIGRATIONS_TABLE: &str = "schema_migrations";

/// 一条迁移。
pub struct Migration {
    /// 版本号，从 1 开始递增，永不复用。
    pub version: i64,
    /// 迁移名，用于日志与人工排查。
    pub name: &'static str,
    /// 建表 / 变更 SQL。
    pub up: &'static [&'static str],
    /// 回滚 SQL（必须与 `up` 对称）。
    pub down: &'static [&'static str],
}

/// 内置迁移列表（按 version 升序）。
///
/// 版本 1 一次性建齐 v1 全部业务表（01 §4.4 DDL 全量 + 02 §9 修订），
/// 拆成 4 条是为了让 down 有语义化粒度，而不是整库推倒。
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "core_channel_model_route",
        up: &[
            crate::store::schema::CHANNEL_DDL,
            crate::store::schema::MODEL_DDL,
            crate::store::schema::ROUTE_GROUP_DDL,
            crate::store::schema::ROUTE_MEMBER_DDL,
        ],
        down: &[
            "DROP TABLE IF EXISTS route_member;",
            "DROP TABLE IF EXISTS route_group;",
            "DROP TABLE IF EXISTS model;",
            "DROP TABLE IF EXISTS channel;",
        ],
    },
    Migration {
        version: 2,
        name: "credentials_account_client_key",
        up: &[
            crate::store::schema::CREDENTIAL_DDL,
            crate::store::schema::ACCOUNT_DDL,
            crate::store::schema::CLIENT_KEY_DDL,
        ],
        down: &[
            "DROP TABLE IF EXISTS client_key;",
            "DROP TABLE IF EXISTS account;",
            "DROP TABLE IF EXISTS credential;",
        ],
    },
    Migration {
        version: 3,
        name: "usage_and_price",
        up: &[crate::store::schema::USAGE_LOG_DDL, crate::store::schema::PRICE_DDL],
        down: &["DROP TABLE IF EXISTS price;", "DROP TABLE IF EXISTS usage_log;"],
    },
    Migration {
        version: 4,
        name: "checkin",
        up: &[
            crate::store::schema::CHECKIN_SITE_DDL,
            crate::store::schema::CHECKIN_RESULT_DDL,
        ],
        down: &[
            "DROP TABLE IF EXISTS checkin_result;",
            "DROP TABLE IF EXISTS checkin_site;",
        ],
    },
    Migration {
        version: 5,
        name: "circuit_state",
        up: &[crate::store::schema::CIRCUIT_STATE_DDL],
        down: &["DROP TABLE IF EXISTS circuit_state;"],
    },
];

/// 确保版本表存在。
pub fn ensure_migrations_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {MIGRATIONS_TABLE} (
             version    INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             applied_at INTEGER NOT NULL
         );"
    ))
    .map_err(|e| StoreError::Migration(format!("创建版本表失败: {e}")))
}

/// 已应用的版本号（升序）。
pub fn applied_versions(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn
        .prepare(&format!("SELECT version FROM {MIGRATIONS_TABLE} ORDER BY version"))
        .map_err(|e| StoreError::Migration(format!("读取版本表失败: {e}")))?;
    let versions = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|e| StoreError::Migration(format!("读取版本表失败: {e}")))?
        .collect::<std::result::Result<Vec<i64>, _>>()
        .map_err(|e| StoreError::Migration(format!("读取版本表失败: {e}")))?;
    Ok(versions)
}

/// 当前 schema 版本（无迁移记录时为 0）。
pub fn current_version(conn: &Connection) -> Result<i64> {
    Ok(applied_versions(conn)?.into_iter().max().unwrap_or(0))
}

/// 应用所有尚未应用的迁移，返回本次应用的版本号。
pub fn migrate_up(conn: &Connection) -> Result<Vec<i64>> {
    ensure_migrations_table(conn)?;
    let applied = applied_versions(conn)?;
    let mut newly_applied = Vec::new();

    for migration in MIGRATIONS {
        if applied.contains(&migration.version) {
            continue;
        }
        apply_one(conn, migration)?;
        newly_applied.push(migration.version);
    }
    Ok(newly_applied)
}

/// 回滚最近一次应用的迁移，返回被回滚的版本号（无可回滚时为 `None`）。
pub fn migrate_down(conn: &Connection) -> Result<Option<i64>> {
    ensure_migrations_table(conn)?;
    let current = current_version(conn)?;
    if current == 0 {
        return Ok(None);
    }
    let Some(target) = MIGRATIONS.iter().rev().find(|m| m.version <= current) else {
        return Ok(None);
    };
    revert_one(conn, target)?;
    Ok(Some(target.version))
}

/// 回滚到指定版本（不含），返回被回滚的版本号列表。
pub fn migrate_down_to(conn: &Connection, target_version: i64) -> Result<Vec<i64>> {
    ensure_migrations_table(conn)?;
    let mut reverted = Vec::new();
    loop {
        let current = current_version(conn)?;
        if current <= target_version {
            break;
        }
        let Some(migration) = MIGRATIONS.iter().rev().find(|m| m.version <= current) else {
            break;
        };
        revert_one(conn, migration)?;
        reverted.push(migration.version);
    }
    Ok(reverted)
}

fn apply_one(conn: &Connection, migration: &Migration) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| StoreError::Migration(format!("开启事务失败: {e}")))?;
    for sql in migration.up {
        tx.execute_batch(sql)
            .map_err(|e| StoreError::Migration(format!("迁移 {} ({}) 失败: {e}", migration.version, migration.name)))?;
    }
    tx.execute(
        &format!("INSERT INTO {MIGRATIONS_TABLE} (version, name, applied_at) VALUES (?1, ?2, ?3)"),
        rusqlite::params![migration.version, migration.name, now_secs()],
    )
    .map_err(|e| StoreError::Migration(format!("记录迁移版本失败: {e}")))?;
    tx.commit()
        .map_err(|e| StoreError::Migration(format!("提交迁移事务失败: {e}")))?;
    Ok(())
}

fn revert_one(conn: &Connection, migration: &Migration) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| StoreError::Migration(format!("开启事务失败: {e}")))?;
    for sql in migration.down {
        tx.execute_batch(sql)
            .map_err(|e| StoreError::Migration(format!("回滚迁移 {} ({}) 失败: {e}", migration.version, migration.name)))?;
    }
    tx.execute(
        &format!("DELETE FROM {MIGRATIONS_TABLE} WHERE version = ?1"),
        rusqlite::params![migration.version],
    )
    .map_err(|e| StoreError::Migration(format!("删除迁移版本记录失败: {e}")))?;
    tx.commit()
        .map_err(|e| StoreError::Migration(format!("提交回滚事务失败: {e}")))?;
    Ok(())
}

/// 当前 Unix 秒（迁移记录用）。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 查询单个表是否存在（测试与诊断用）。
pub fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let found: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| StoreError::Migration(format!("查询表存在性失败: {e}")))?;
    Ok(found.is_some())
}
