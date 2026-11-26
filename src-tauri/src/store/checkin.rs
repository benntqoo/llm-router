//! 签到站点模板的读写（T15；02 §7.1.1）。
//!
//! 表结构沿用 01 §4.4 的 `checkin_site`（v1 已建，本任务**不改 schema**）：
//! `config_json` 存**站点模板**（占位符化后的 HAR 片段 + 变量表，**零明文**），
//! 变量的真实取值在保险库 `credential` 表里（密文）。
//!
//! 与 T7/T8 一致：表行只放"配置"，凭证不进这里（02 §5 密文边界）。

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::Result;

/// 用户自建 HAR 模板的插件标识（内置站点插件属 T18）。
pub const PLUGIN_HAR: &str = "har";

/// 签到站点一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteRow {
    /// `checkin_site.id`
    pub id: String,
    /// 站点名（用户可读）
    pub name: String,
    /// 插件标识（`har` = 用户自建 HAR 模板；内置站点插件见 T18）
    pub plugin: String,
    /// 站点模板 JSON（**不含明文密文**）
    pub config_json: Option<String>,
    /// cron 表达式（调度属 T17；T15 留空）
    pub schedule: Option<String>,
    /// 是否启用（**导入的模板默认关**——02 §7.3：站点插件默认全关）
    pub enabled: bool,
}

/// 生成站点 id（`site_` + 32 位十六进制）。
pub fn new_site_id() -> String {
    let raw = rand::random::<[u8; 16]>();
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    format!("site_{hex}")
}

/// 写（新增或覆盖）一个签到站点。
pub fn upsert_site(conn: &Connection, site: &SiteRow) -> Result<()> {
    conn.execute(
        "INSERT INTO checkin_site (id, name, plugin, config_json, schedule, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           plugin = excluded.plugin,
           config_json = excluded.config_json,
           schedule = excluded.schedule,
           enabled = excluded.enabled",
        rusqlite::params![
            site.id,
            site.name,
            site.plugin,
            site.config_json,
            site.schedule,
            if site.enabled { 1_i64 } else { 0_i64 },
        ],
    )?;
    Ok(())
}

/// 列出全部签到站点（按创建顺序）。
pub fn list_sites(conn: &Connection) -> Result<Vec<SiteRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, plugin, config_json, schedule, enabled
         FROM checkin_site ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], row_to_site)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// 取一个签到站点。
pub fn get_site(conn: &Connection, id: &str) -> Result<Option<SiteRow>> {
    let site = conn
        .query_row(
            "SELECT id, name, plugin, config_json, schedule, enabled
             FROM checkin_site WHERE id = ?1",
            [id],
            row_to_site,
        )
        .optional()?;
    Ok(site)
}

/// 按站点名查（重录 HAR 时复用同一行）。
pub fn find_site_by_name(conn: &Connection, name: &str) -> Result<Option<SiteRow>> {
    let site = conn
        .query_row(
            "SELECT id, name, plugin, config_json, schedule, enabled
             FROM checkin_site WHERE name = ?1",
            [name],
            row_to_site,
        )
        .optional()?;
    Ok(site)
}

/// 删一个签到站点（返回是否确实删掉）。
pub fn delete_site(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM checkin_site WHERE id = ?1", [id])?;
    Ok(affected > 0)
}

fn row_to_site(row: &rusqlite::Row<'_>) -> rusqlite::Result<SiteRow> {
    Ok(SiteRow {
        id: row.get(0)?,
        name: row.get(1)?,
        plugin: row.get(2)?,
        config_json: row.get(3)?,
        schedule: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn site(name: &str) -> SiteRow {
        SiteRow {
            id: new_site_id(),
            name: name.to_string(),
            plugin: PLUGIN_HAR.to_string(),
            config_json: Some("{\"version\":1}".to_string()),
            schedule: None,
            enabled: false,
        }
    }

    #[test]
    fn site_ids_are_unique_and_prefixed() {
        let a = new_site_id();
        let b = new_site_id();
        assert_ne!(a, b);
        assert!(a.starts_with("site_"));
        assert_eq!(a.len(), "site_".len() + 32);
    }

    #[test]
    fn upsert_and_list_round_trip() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let conn = store.lock();
        let mut row = site("ModelScope");
        upsert_site(&conn, &row).expect("写入失败");
        row.schedule = Some("0 9 * * *".to_string());
        row.enabled = true;
        upsert_site(&conn, &row).expect("覆盖失败");

        let rows = list_sites(&conn).expect("列出失败");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "ModelScope");
        assert_eq!(rows[0].schedule.as_deref(), Some("0 9 * * *"));
        assert!(rows[0].enabled);
    }

    #[test]
    fn find_by_name_supports_re_recording_the_same_site() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let conn = store.lock();
        let row = site("ModelScope");
        upsert_site(&conn, &row).expect("写入失败");
        let found = find_site_by_name(&conn, "ModelScope")
            .expect("查询失败")
            .expect("应能按名字找到");
        assert_eq!(found.id, row.id);
        assert!(find_site_by_name(&conn, "不存在").unwrap().is_none());
    }

    #[test]
    fn delete_reports_whether_it_existed() {
        let store = Store::open_in_memory().expect("打开内存库失败");
        let conn = store.lock();
        let row = site("临时");
        upsert_site(&conn, &row).expect("写入失败");
        assert!(delete_site(&conn, &row.id).expect("删除失败"));
        assert!(!delete_site(&conn, &row.id).expect("删除失败"));
        assert!(get_site(&conn, &row.id).unwrap().is_none());
    }
}
