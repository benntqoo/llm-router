//! 应用级设置（键值对）。
//!
//! 用于放用户可调的运行参数（02 §2.3 的熔断时长等）。
//! 取值一律按**字符串**存，读出来由调用方解析——不同 setting 的类型不同，
//! 数据库层不替它们做类型判断。

use crate::store::{Result, Store};

/// 连续失败熔断时长的设置键（秒；默认 60，最小 1）。
pub const KEY_CIRCUIT_OPEN_SECS: &str = "circuit_open_secs";

/// 读设置（不存在返回 `None`）。
pub fn get_setting(store: &Store, key: &str) -> Result<Option<String>> {
    store.with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT value FROM app_setting WHERE key = ?1")?;
        let mut rows = stmt.query([key])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    })
}

/// 写设置（覆盖）。
pub fn set_setting(store: &Store, key: &str, value: &str) -> Result<()> {
    let now = crate::vault::now_secs();
    store.with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_setting (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![key, value, now],
        )?;
        Ok(())
    })
}

/// 读整数设置（值非法或缺失时返回 `None`）。
pub fn get_int_setting(store: &Store, key: &str) -> Option<i64> {
    get_setting(store, key)
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
}

/// 写整数设置。
pub fn set_int_setting(store: &Store, key: &str, value: i64) -> Result<()> {
    set_setting(store, key, &value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        (dir, store)
    }

    #[test]
    fn setting_round_trip() {
        let (_dir, store) = store();
        assert_eq!(get_setting(&store, KEY_CIRCUIT_OPEN_SECS).expect("读取失败"), None);
        set_int_setting(&store, KEY_CIRCUIT_OPEN_SECS, 120).expect("写入失败");
        assert_eq!(
            get_int_setting(&store, KEY_CIRCUIT_OPEN_SECS),
            Some(120)
        );
    }

    #[test]
    fn setting_is_overwritten_not_duplicated() {
        let (_dir, store) = store();
        set_int_setting(&store, KEY_CIRCUIT_OPEN_SECS, 30).expect("写入失败");
        set_int_setting(&store, KEY_CIRCUIT_OPEN_SECS, 90).expect("覆盖失败");
        assert_eq!(get_int_setting(&store, KEY_CIRCUIT_OPEN_SECS), Some(90));
    }

    #[test]
    fn invalid_integer_falls_back_to_none() {
        let (_dir, store) = store();
        set_setting(&store, "broken", "not-a-number").expect("写入失败");
        assert_eq!(get_int_setting(&store, "broken"), None);
    }
}
