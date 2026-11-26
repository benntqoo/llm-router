//! 用量流水落库与窗口计数（02 §8.1、§8.2）。
//!
//! - **一条记录 = 一次出站尝试**（不是一次入站请求）：fallback 链试了 3 个候选就是 3 行；
//! - **落库失败不影响请求**：写用量失败只记日志（统计丢一条不该让用户的请求失败）；
//! - **计数从流水派生**：RPM/RPD/TPM/TPD 直接由 `usage_log` 聚合，不另设计数器表
//!   ——流水是唯一真值，重启不丢计数。

use std::sync::Arc;

use crate::store::{Result, Store};

/// 一条用量记录（对应 `usage_log` 一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// 记录时刻（Unix 秒）。
    pub ts: i64,
    /// 入站 client key id。
    pub client_key_id: Option<String>,
    /// 出站渠道 id。
    pub channel_id: String,
    /// 账号 id（02 §8.2：API Key 型渠道用默认账号）。
    pub account_id: String,
    /// 入站别名。
    pub model_alias: String,
    /// 上游真实模型名。
    pub upstream_model: String,
    /// 入站协议（v1 恒为 `openai`）。
    pub protocol: String,
    /// 输入 token。
    pub prompt_tokens: i64,
    /// 输出 token。
    pub completion_tokens: i64,
    /// 缓存命中 token。
    pub cache_read_tokens: i64,
    /// 缓存写入 token。
    pub cache_write_tokens: i64,
    /// 推理 token（MiniMax 特例，02 §4.2）。
    pub reasoning_tokens: i64,
    /// 是否为本地估算值（上游没回 usage）。
    pub estimated: bool,
    /// 首个 token 到达耗时（毫秒；非流式为 `None`）。
    pub ttft_ms: Option<i64>,
    /// 本次出站尝试耗时（毫秒）。
    pub duration_ms: i64,
    /// 结果状态。
    pub status: UsageStatus,
    /// 失败时的错误类别。
    pub error_kind: Option<String>,
}

/// 用量记录的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStatus {
    /// 成功。
    Success,
    /// 失败（上游错误 / 4xx / 5xx / 429）。
    Error,
    /// 客户端中途断连。
    Aborted,
}

impl UsageStatus {
    /// 落库字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            UsageStatus::Success => "success",
            UsageStatus::Error => "error",
            UsageStatus::Aborted => "aborted",
        }
    }
}

/// 用量记录的消费端（网关采集 → 数据层落库）。
pub trait UsageSink: Send + Sync {
    /// 记录一条用量（**不得阻塞调用方太久**，也不得让记录失败影响请求）。
    fn record(&self, record: UsageRecord);
}

/// 基于 `Store` 的用量落库实现。
pub struct StoreUsageSink {
    store: Arc<Store>,
}

impl std::fmt::Debug for StoreUsageSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreUsageSink").finish_non_exhaustive()
    }
}

impl StoreUsageSink {
    /// 构造（`store` 为共享句柄）。
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl UsageSink for StoreUsageSink {
    fn record(&self, record: UsageRecord) {
        let store = Arc::clone(&self.store);
        // 不在 tokio 工作线程上做同步写：丢给阻塞线程池（store/mod.rs 的约定）
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(move || {
                if let Err(err) = insert_usage(&store, &record) {
                    eprintln!("[usage] 落库失败: {err}");
                }
            });
        } else if let Err(err) = insert_usage(&store, &record) {
            eprintln!("[usage] 落库失败: {err}");
        }
    }
}

/// 写一条用量记录。
///
/// **成本换算（T13；02 §8.3）**：按渠道的 `provider` 查价目表，把 token 数换算成
/// `cost_micros`（1e-6 元）。**没命中价目 → NULL**（未知），不是 0。
pub fn insert_usage(store: &Store, record: &UsageRecord) -> Result<()> {
    store.with_conn(|conn| {
        let provider = crate::store::price::provider_of_channel(conn, &record.channel_id)?;
        let cost = match crate::store::price::lookup(conn, &provider, &record.upstream_model)? {
            Some(price) => Some(crate::store::price::compute_cost_micros(
                crate::store::price::BillableTokens {
                    prompt_tokens: record.prompt_tokens,
                    completion_tokens: record.completion_tokens,
                    cache_read_tokens: record.cache_read_tokens,
                    cache_write_tokens: record.cache_write_tokens,
                },
                &price,
            )),
            None => None,
        };
        conn.execute(
            "INSERT INTO usage_log
                 (ts, client_key_id, channel_id, account_id, model_alias, upstream_model, protocol,
                  prompt_tokens, completion_tokens, cache_read_tokens, cache_write_tokens,
                  reasoning_tokens, estimated, ttft_ms, duration_ms, status, error_kind, cost_micros)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            rusqlite::params![
                record.ts,
                record.client_key_id,
                record.channel_id,
                record.account_id,
                record.model_alias,
                record.upstream_model,
                record.protocol,
                record.prompt_tokens,
                record.completion_tokens,
                record.cache_read_tokens,
                record.cache_write_tokens,
                record.reasoning_tokens,
                if record.estimated { 1 } else { 0 },
                record.ttft_ms,
                record.duration_ms,
                record.status.as_str(),
                record.error_kind,
                cost,
            ],
        )?;
        Ok(())
    })
}

/// 四个窗口的用量计数（02 §8.2）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuotaCounters {
    /// 最近 60 秒的请求次数。
    pub rpm: i64,
    /// 当日（UTC 日界）的请求次数。
    pub rpd: i64,
    /// 最近 60 秒的 token 总数。
    pub tpm: i64,
    /// 当日的 token 总数。
    pub tpd: i64,
}

/// 当日（UTC 日界）的起始秒。
pub fn utc_day_start(now: i64) -> i64 {
    now.div_euclid(86_400) * 86_400
}

/// 统计某 `(channel, account)` 的四个窗口用量。
///
/// 计数直接由 `usage_log` 派生——流水是唯一真值。
pub fn window_counters(
    store: &Store,
    channel_id: &str,
    account_id: &str,
    now: i64,
) -> Result<QuotaCounters> {
    let minute_start = now - 60;
    let day_start = utc_day_start(now);
    store.with_conn(|conn| {
        let rpm: i64 = conn.query_row(
            "SELECT COUNT(*) FROM usage_log
             WHERE channel_id = ?1 AND account_id = ?2 AND ts > ?3",
            rusqlite::params![channel_id, account_id, minute_start],
            |row| row.get(0),
        )?;
        let rpd: i64 = conn.query_row(
            "SELECT COUNT(*) FROM usage_log
             WHERE channel_id = ?1 AND account_id = ?2 AND ts >= ?3",
            rusqlite::params![channel_id, account_id, day_start],
            |row| row.get(0),
        )?;
        let tpm: i64 = conn.query_row(
            "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0) FROM usage_log
             WHERE channel_id = ?1 AND account_id = ?2 AND ts > ?3",
            rusqlite::params![channel_id, account_id, minute_start],
            |row| row.get(0),
        )?;
        let tpd: i64 = conn.query_row(
            "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0) FROM usage_log
             WHERE channel_id = ?1 AND account_id = ?2 AND ts >= ?3",
            rusqlite::params![channel_id, account_id, day_start],
            |row| row.get(0),
        )?;
        Ok(QuotaCounters {
            rpm,
            rpd,
            tpm,
            tpd,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        (dir, store)
    }

    fn record(ts: i64, prompt: i64, completion: i64) -> UsageRecord {
        UsageRecord {
            ts,
            client_key_id: Some("ck1".to_string()),
            channel_id: "c1".to_string(),
            account_id: "acct_default_c1".to_string(),
            model_alias: "alias".to_string(),
            upstream_model: "deepseek-chat".to_string(),
            protocol: "openai".to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            estimated: false,
            ttft_ms: Some(120),
            duration_ms: 900,
            status: UsageStatus::Success,
            error_kind: None,
        }
    }

    #[test]
    fn usage_row_round_trips() {
        let (_dir, store) = store();
        insert_usage(&store, &record(1_000, 11, 7)).expect("写入失败");

        store
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT ts, client_key_id, channel_id, account_id, prompt_tokens,
                            completion_tokens, estimated, ttft_ms, status
                     FROM usage_log",
                )?;
                let mut rows = stmt.query([])?;
                let row = rows.next()?.expect("应有一行");
                let ts: i64 = row.get(0)?;
                let client_key_id: Option<String> = row.get(1)?;
                let channel_id: String = row.get(2)?;
                let account_id: Option<String> = row.get(3)?;
                let prompt_tokens: i64 = row.get(4)?;
                let completion_tokens: i64 = row.get(5)?;
                let estimated: i64 = row.get(6)?;
                let ttft_ms: Option<i64> = row.get(7)?;
                let status: String = row.get(8)?;

                assert_eq!(ts, 1_000);
                assert_eq!(client_key_id.as_deref(), Some("ck1"));
                assert_eq!(channel_id, "c1");
                assert_eq!(account_id.as_deref(), Some("acct_default_c1"));
                assert_eq!(prompt_tokens, 11);
                assert_eq!(completion_tokens, 7);
                assert_eq!(estimated, 0, "上游回报的用量不是估算值");
                assert_eq!(ttft_ms, Some(120));
                assert_eq!(status, "success");
                Ok(())
            })
            .expect("读取失败");
    }

    #[test]
    fn window_counters_split_minute_and_day() {
        let (_dir, store) = store();
        let now = 1_700_000_000i64;
        let day = utc_day_start(now);
        // 30 秒前、今天
        insert_usage(&store, &record(now - 30, 10, 5)).expect("写入失败");
        // 5 分钟前、今天
        insert_usage(&store, &record(now - 300, 10, 5)).expect("写入失败");
        // 昨天
        insert_usage(&store, &record(day - 60, 100, 50)).expect("写入失败");

        let counters = window_counters(&store, "c1", "acct_default_c1", now).expect("统计失败");
        assert_eq!(counters.rpm, 1, "只有最近 60 秒内的算 RPM");
        assert_eq!(counters.rpd, 2, "昨天的不算今日");
        assert_eq!(counters.tpm, 15);
        assert_eq!(counters.tpd, 30);
    }

    #[test]
    fn counters_are_scoped_to_channel_and_account() {
        let (_dir, store) = store();
        insert_usage(&store, &record(1_000, 10, 5)).expect("写入失败");

        let other_channel = window_counters(&store, "c2", "acct_default_c1", 1_000).expect("统计失败");
        assert_eq!(other_channel.rpm, 0, "别的渠道不受影响");

        let other_account = window_counters(&store, "c1", "acct_other", 1_000).expect("统计失败");
        assert_eq!(other_account.rpm, 0, "别的账号不受影响");
    }

    #[test]
    fn utc_day_start_is_midnight() {
        assert_eq!(utc_day_start(0), 0);
        assert_eq!(utc_day_start(86_399), 0);
        assert_eq!(utc_day_start(86_400), 86_400);
        // 负数时间戳（1970 之前）也要向下取整
        assert_eq!(utc_day_start(-1), -86_400);
    }

    #[test]
    fn status_strings_follow_the_schema() {
        assert_eq!(UsageStatus::Success.as_str(), "success");
        assert_eq!(UsageStatus::Error.as_str(), "error");
        assert_eq!(UsageStatus::Aborted.as_str(), "aborted");
    }
}
