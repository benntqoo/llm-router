//! 限额自学习（02 §8.2）。
//!
//! - **落点唯一**：`account.quota_json`（用户可编辑的 JSON）；
//! - **账号口径**：API Key 型渠道用**默认账号**（id = `acct_default_<channel_id>`），
//!   OAuth / 登录态型渠道用账号池里的真实账号（T8/T9）；
//! - **只收紧不放宽**：新学到的上限低于已记录值才覆盖——撞到上限说明预检放行了太多，
//!   放宽上限只会更糟；用户按渠道文档手填的值同样只会被压低，不会被抬高。

use serde::{Deserialize, Serialize};

use crate::store::{Result, Store};

/// 默认账号 id 的前缀（API Key 型渠道）。
pub const DEFAULT_ACCOUNT_PREFIX: &str = "acct_default_";

/// 默认账号的展示名。
pub const DEFAULT_ACCOUNT_LABEL: &str = "默认账号";

/// 上限来源标记（写进 `quota_json.source`）。
pub const SOURCE_UPSTREAM_HEADERS: &str = "upstream_headers";

/// API Key 型渠道的默认账号 id（确定性生成）。
pub fn default_account_id(channel_id: &str) -> String {
    format!("{DEFAULT_ACCOUNT_PREFIX}{channel_id}")
}

/// 一次限流观测（来自上游 429 的响应头）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimitObservation {
    /// `x-ratelimit-limit-requests`：上游每分钟请求数上限。
    pub limit_requests: Option<i64>,
    /// `x-ratelimit-limit-tokens`：上游每分钟 token 上限。
    pub limit_tokens: Option<i64>,
    /// `Retry-After`（秒）。
    pub retry_after_secs: Option<i64>,
    /// `x-ratelimit-reset-requests`（秒）。
    pub reset_requests_secs: Option<i64>,
    /// `x-ratelimit-reset-tokens`（秒）。
    pub reset_tokens_secs: Option<i64>,
    /// `x-ratelimit-remaining-requests`。
    pub remaining_requests: Option<i64>,
    /// `x-ratelimit-remaining-tokens`。
    pub remaining_tokens: Option<i64>,
}

/// 上次限流的事实（这些是上游**报的**，不是我们推断的）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitFact {
    /// 观测时刻（Unix 秒）。
    pub ts: i64,
    /// `Retry-After`（秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
    /// `x-ratelimit-reset-requests`（秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_requests_secs: Option<i64>,
    /// `x-ratelimit-reset-tokens`（秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_tokens_secs: Option<i64>,
    /// `x-ratelimit-remaining-requests`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_requests: Option<i64>,
    /// `x-ratelimit-remaining-tokens`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_tokens: Option<i64>,
}

/// 账号的限额状态（`account.quota_json` 的形状）。
///
/// 字段是"上限"，`None` = 未知；用户可以按渠道文档手填（写进同一组字段），
/// 自学习只会把它压低、不会抬高。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaState {
    /// 每分钟请求数上限（RPM）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<i64>,
    /// 每日请求数上限（RPD）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpd: Option<i64>,
    /// 每分钟 token 上限（TPM）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<i64>,
    /// 每日 token 上限（TPD）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpd: Option<i64>,
    /// 上限来源（`upstream_headers` / 用户手填）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 学习时刻（Unix 秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_at: Option<i64>,
    /// 上次限流的事实。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rate_limit: Option<RateLimitFact>,
}

impl QuotaState {
    /// 是否记下了任何上限。
    pub fn has_ceiling(&self) -> bool {
        self.rpm.is_some() || self.rpd.is_some() || self.tpm.is_some() || self.tpd.is_some()
    }
}

/// 确保渠道有一个承载限额状态的账号，返回账号 id。
///
/// API Key 型渠道用**默认账号**（确定性 id）；已存在则不动它的 `quota_json`。
pub fn ensure_account(store: &Store, channel_id: &str) -> Result<String> {
    let account_id = default_account_id(channel_id);
    store.with_conn(|conn| {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM account WHERE id = ?1",
            [&account_id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            conn.execute(
                "INSERT INTO account (id, channel_id, label, quota_json, enabled)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                rusqlite::params![account_id, channel_id, DEFAULT_ACCOUNT_LABEL, "{}"],
            )?;
        }
        Ok(())
    })?;
    Ok(account_id)
}

/// 读账号的限额状态（没有行或 JSON 损坏时返回默认状态）。
pub fn read_quota(store: &Store, account_id: &str) -> Result<QuotaState> {
    let raw: Option<String> = store.with_conn(|conn| {
        let mut stmt =
            conn.prepare("SELECT quota_json FROM account WHERE id = ?1")?;
        let mut rows = stmt.query([account_id])?;
        match rows.next()? {
            Some(row) => Ok(row.get(0)?),
            None => Ok(None),
        }
    })?;
    Ok(raw
        .and_then(|text| serde_json::from_str::<QuotaState>(&text).ok())
        .unwrap_or_default())
}

/// 写账号的限额状态。
pub fn write_quota(store: &Store, account_id: &str, state: &QuotaState) -> Result<()> {
    let text = serde_json::to_string(state).map_err(|e| {
        crate::store::StoreError::Migration(format!("序列化限额状态失败: {e}"))
    })?;
    store.with_conn(|conn| {
        conn.execute(
            "UPDATE account SET quota_json = ?1 WHERE id = ?2",
            rusqlite::params![text, account_id],
        )?;
        Ok(())
    })
}

/// 从一次 429 观测里**学习**上限并回写 `quota_json`。
///
/// 只收紧：学习到的上限低于已记录值（或此前未知）才覆盖。
/// 账号不存在时先补建默认账号（02 §8.2）。
pub fn learn_rate_limit(
    store: &Store,
    channel_id: &str,
    observation: &RateLimitObservation,
) -> Result<QuotaState> {
    let account_id = ensure_account(store, channel_id)?;
    let mut state = read_quota(store, &account_id)?;
    let now = crate::vault::now_secs();

    if let Some(limit) = observation.limit_requests {
        if state.rpm.map_or(true, |known| limit < known) {
            state.rpm = Some(limit);
        }
    }
    if let Some(limit) = observation.limit_tokens {
        if state.tpm.map_or(true, |known| limit < known) {
            state.tpm = Some(limit);
        }
    }
    state.source = Some(SOURCE_UPSTREAM_HEADERS.to_string());
    state.learned_at = Some(now);
    state.last_rate_limit = Some(RateLimitFact {
        ts: now,
        retry_after_secs: observation.retry_after_secs,
        reset_requests_secs: observation.reset_requests_secs,
        reset_tokens_secs: observation.reset_tokens_secs,
        remaining_requests: observation.remaining_requests,
        remaining_tokens: observation.remaining_tokens,
    });

    write_quota(store, &account_id, &state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        (dir, store)
    }

    fn seed_channel(store: &Store) {
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
    }

    #[test]
    fn default_account_is_created_on_demand() {
        let (_dir, store) = store();
        seed_channel(&store);

        let account_id = ensure_account(&store, "c1").expect("补建账号失败");
        assert_eq!(account_id, "acct_default_c1");

        let (label, quota) = store
            .with_conn(|conn| {
                let mut stmt =
                    conn.prepare("SELECT label, quota_json FROM account WHERE id = 'acct_default_c1'")?;
                let mut rows = stmt.query([])?;
                let row = rows.next()?.expect("应有一行");
                let label: String = row.get(0)?;
                let quota: Option<String> = row.get(1)?;
                Ok((label, quota))
            })
            .expect("读取失败");
        assert_eq!(label, DEFAULT_ACCOUNT_LABEL);
        assert_eq!(quota.as_deref(), Some("{}"));
    }

    #[test]
    fn learning_writes_the_ceiling_into_quota_json() {
        let (_dir, store) = store();
        seed_channel(&store);

        let observation = RateLimitObservation {
            limit_requests: Some(60),
            limit_tokens: Some(120_000),
            retry_after_secs: Some(30),
            remaining_requests: Some(0),
            ..Default::default()
        };
        let state = learn_rate_limit(&store, "c1", &observation).expect("学习失败");
        assert_eq!(state.rpm, Some(60));
        assert_eq!(state.tpm, Some(120_000));
        assert_eq!(state.last_rate_limit.and_then(|f| f.retry_after_secs), Some(30));

        // 落库的 JSON 里能看到
        let raw: String = store
            .with_conn(|conn| {
                let mut stmt =
                    conn.prepare("SELECT quota_json FROM account WHERE id = 'acct_default_c1'")?;
                let mut rows = stmt.query([])?;
                let row = rows.next()?.expect("应有一行");
                let raw: String = row.get(0)?;
                Ok(raw)
            })
            .expect("读取失败");
        assert!(raw.contains("\"rpm\":60"), "实际 {raw}");
        assert!(raw.contains("\"tpm\":120000"), "实际 {raw}");
        assert!(raw.contains("upstream_headers"), "实际 {raw}");
    }

    #[test]
    fn learning_only_tightens_never_relaxes() {
        let (_dir, store) = store();
        seed_channel(&store);

        // 先学一个紧的上限
        learn_rate_limit(
            &store,
            "c1",
            &RateLimitObservation {
                limit_requests: Some(30),
                ..Default::default()
            },
        )
        .expect("学习失败");

        // 再来一个更宽松的 → 不应覆盖
        let state = learn_rate_limit(
            &store,
            "c1",
            &RateLimitObservation {
                limit_requests: Some(600),
                ..Default::default()
            },
        )
        .expect("学习失败");
        assert_eq!(state.rpm, Some(30), "只收紧，不放宽");

        // 更紧的 → 覆盖
        let state = learn_rate_limit(
            &store,
            "c1",
            &RateLimitObservation {
                limit_requests: Some(10),
                ..Default::default()
            },
        )
        .expect("学习失败");
        assert_eq!(state.rpm, Some(10));
    }

    #[test]
    fn user_declared_ceiling_is_also_only_tightened() {
        let (_dir, store) = store();
        seed_channel(&store);
        let account_id = ensure_account(&store, "c1").expect("补建账号失败");

        // 用户按渠道文档手填上限
        let mut state = read_quota(&store, &account_id).expect("读取失败");
        state.rpm = Some(100);
        state.source = Some("channel_doc".to_string());
        write_quota(&store, &account_id, &state).expect("写入失败");

        // 自学习只压低
        let state = learn_rate_limit(
            &store,
            "c1",
            &RateLimitObservation {
                limit_requests: Some(1000),
                ..Default::default()
            },
        )
        .expect("学习失败");
        assert_eq!(state.rpm, Some(100), "手填的上限不被抬高");
    }

    #[test]
    fn missing_account_reads_as_default_state() {
        let (_dir, store) = store();
        let state = read_quota(&store, "acct_missing").expect("读取失败");
        assert!(!state.has_ceiling());
        assert_eq!(state.rpm, None);
    }

    #[test]
    fn corrupted_quota_json_reads_as_default_state() {
        let (_dir, store) = store();
        seed_channel(&store);
        let account_id = ensure_account(&store, "c1").expect("补建账号失败");
        store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE account SET quota_json = 'not-json' WHERE id = ?1",
                    [&account_id],
                )?;
                Ok(())
            })
            .expect("写入失败");

        let state = read_quota(&store, &account_id).expect("读取失败");
        assert!(!state.has_ceiling(), "坏 JSON 不应让读失败");
    }
}
