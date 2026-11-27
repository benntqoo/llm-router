//! 账号额度快照里的**签到积分**（T18；02 §7.4 §4）。
//!
//! **落点唯一**：`account.quota_json`（与 §8.2 的限额自学习同一列）。
//!
//! **合并写，不覆盖限额字段**：`quota_json` 里 `points` / `unit` / `source` /
//! `updated_at` 与限额字段（`rpm` / `rpd` / `tpm` / `tpd` / `learned_at` /
//! `last_rate_limit`）**并存**——回写只改这四个键，**不动**其它键。
//!
//! **语义**：签到侧**只写「本次拿到了多少」**，**不猜测账户总额**（余额真值由取数方负责）。
//!
//! 密文边界（AGENT.md §5.3）：这里只写**数量与单位**，**不写任何凭据值**。

use serde::{Deserialize, Serialize};

use crate::store::{Result, Store};

/// 积分来源标记（与 §8.2 的限额来源 `upstream_headers` 区分）。
pub const SOURCE_CHECKIN: &str = "checkin";

/// 键名（`quota_json` 内）。
const KEY_POINTS: &str = "points";
const KEY_UNIT: &str = "unit";
const KEY_SOURCE: &str = "source";
const KEY_UPDATED_AT: &str = "updated_at";

/// 账号额度快照里的签到积分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardSnapshot {
    /// 本次拿到的数量。
    pub points: i64,
    /// 单位（`magicube` = 魔粒；`point` = MiniMax 积分）。
    pub unit: String,
    /// 来源（`checkin`）。
    pub source: String,
    /// 写入时刻（Unix 秒）。
    pub updated_at: i64,
}

/// 把一次签到拿到的积分**合并写**进账号的 `quota_json`。
///
/// - **只改四个键**：`points` / `unit` / `source` / `updated_at`；
/// - `quota_json` 原本是 `NULL` / 空 / 坏 JSON → 当成空对象处理（**坏 JSON 会被覆盖成合法对象**）；
/// - 账号不存在 → **报错**（不静默新建：账号的归属由渠道决定）。
pub fn write_reward(
    store: &Store,
    account_id: &str,
    points: i64,
    unit: &str,
    updated_at: i64,
) -> Result<()> {
    let raw: Option<String> = store.with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT quota_json FROM account WHERE id = ?1")?;
        let mut rows = stmt.query([account_id])?;
        match rows.next()? {
            Some(row) => Ok(row.get(0)?),
            None => Ok(None),
        }
    })?;

    // 账号不存在 → 拒绝写（额度快照必须挂在真实账号上）
    let Some(raw) = raw else {
        return Err(crate::store::StoreError::Migration(format!(
            "账号不存在，无法回写积分：{account_id}"
        )));
    };

    let mut map: serde_json::Map<String, serde_json::Value> = match raw.trim() {
        "" => serde_json::Map::new(),
        text => serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| match value {
                serde_json::Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default(),
    };
    map.insert(
        KEY_POINTS.to_string(),
        serde_json::Value::Number(points.into()),
    );
    map.insert(
        KEY_UNIT.to_string(),
        serde_json::Value::String(unit.to_string()),
    );
    map.insert(
        KEY_SOURCE.to_string(),
        serde_json::Value::String(SOURCE_CHECKIN.to_string()),
    );
    map.insert(
        KEY_UPDATED_AT.to_string(),
        serde_json::Value::Number(updated_at.into()),
    );
    let text = serde_json::to_string(&serde_json::Value::Object(map)).map_err(|e| {
        crate::store::StoreError::Migration(format!("序列化额度快照失败: {e}"))
    })?;
    store.with_conn(|conn| {
        conn.execute(
            "UPDATE account SET quota_json = ?1 WHERE id = ?2",
            rusqlite::params![text, account_id],
        )?;
        Ok(())
    })
}

/// 读账号的签到积分（没有就返回 `None`）。
pub fn read_reward(store: &Store, account_id: &str) -> Option<RewardSnapshot> {
    let raw: Option<String> = store
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT quota_json FROM account WHERE id = ?1")?;
            let mut rows = stmt.query([account_id])?;
            match rows.next()? {
                Some(row) => Ok(row.get(0)?),
                None => Ok(None),
            }
        })
        .ok()?;
    let text = raw?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let points = value.get(KEY_POINTS)?.as_i64()?;
    Some(RewardSnapshot {
        points,
        unit: value
            .get(KEY_UNIT)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        source: value
            .get(KEY_SOURCE)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        updated_at: value.get(KEY_UPDATED_AT).and_then(|v| v.as_i64()).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::quota;

    fn store_with_channel() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO channel (id, name, kind, provider, base_url, created_at)
                     VALUES ('c1', '签到 · ModelScope', 'api_key', 'custom', '', 0)",
                    [],
                )?;
                Ok(())
            })
            .expect("写入渠道");
        (dir, store)
    }

    #[test]
    fn reward_is_written_into_quota_json() {
        let (_dir, store) = store_with_channel();
        let account_id = quota::ensure_account(&store, "c1").expect("建账号失败");
        write_reward(&store, &account_id, 400, "magicube", 1_700_000_000).expect("回写失败");

        let snapshot = read_reward(&store, &account_id).expect("应能读回积分");
        assert_eq!(snapshot.points, 400);
        assert_eq!(snapshot.unit, "magicube");
        assert_eq!(snapshot.source, SOURCE_CHECKIN);
        assert_eq!(snapshot.updated_at, 1_700_000_000);
    }

    #[test]
    fn writing_reward_does_not_touch_the_quota_fields() {
        // **合并写**：限额字段（§8.2 自学习）与签到积分并存，互不覆盖
        let (_dir, store) = store_with_channel();
        let account_id = quota::ensure_account(&store, "c1").expect("建账号失败");
        let mut state = quota::read_quota(&store, &account_id).expect("读限额失败");
        state.rpm = Some(60);
        state.source = Some("upstream_headers".to_string());
        quota::write_quota(&store, &account_id, &state).expect("写限额失败");

        write_reward(&store, &account_id, 400, "magicube", 1_700_000_000).expect("回写失败");

        let after = quota::read_quota(&store, &account_id).expect("读限额失败");
        assert_eq!(after.rpm, Some(60), "限额字段不能被覆盖");
        let raw: String = store
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT quota_json FROM account WHERE id = ?1")?;
                let mut rows = stmt.query([account_id])?;
                let row = rows.next()?.expect("应有一行");
                Ok(row.get(0)?)
            })
            .expect("读取失败");
        assert!(raw.contains("\"rpm\":60"), "实际 {raw}");
        assert!(raw.contains("\"points\":400"), "实际 {raw}");
        assert!(raw.contains("\"unit\":\"magicube\""), "实际 {raw}");
    }

    #[test]
    fn missing_account_is_rejected() {
        let (_dir, store) = store_with_channel();
        let err = write_reward(&store, "acct_missing", 1, "point", 0).expect_err("应报错");
        assert!(err.to_string().contains("账号不存在"), "实际 {err}");
    }

    #[test]
    fn corrupted_quota_json_is_replaced_by_a_valid_object() {
        let (_dir, store) = store_with_channel();
        let account_id = quota::ensure_account(&store, "c1").expect("建账号失败");
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE account SET quota_json = 'not-json' WHERE id = ?1",
                    [&account_id],
                )?;
                Ok(())
            })
            .expect("写坏 JSON 失败");

        write_reward(&store, &account_id, 7, "point", 42).expect("回写失败");
        let snapshot = read_reward(&store, &account_id).expect("应能读回");
        assert_eq!(snapshot.points, 7);
        assert_eq!(snapshot.updated_at, 42);
    }

    #[test]
    fn reading_without_a_snapshot_returns_none() {
        let (_dir, store) = store_with_channel();
        let account_id = quota::ensure_account(&store, "c1").expect("建账号失败");
        assert!(read_reward(&store, &account_id).is_none());
    }
}
