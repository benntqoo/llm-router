//! 用量聚合与图表数据（T14；02 §8.4）。
//!
//! - **四维聚合**：按 日 / 渠道 / 模型 / 客户端 分桶，指标全部**从 `usage_log` 派生**
//!   ——流水是唯一真值，不另设计数表（与 02 §8.2 的窗口计数同一口径）。
//! - **最近请求明细**：逐条给出 tokens / TTFT / tokens·s⁻¹ / 成本 / `estimated` 标记，
//!   供 M2 判据「一次流式请求后 UI 显示 tokens / TTFT / tokens·s⁻¹ / 估算成本」。
//!
//! 单位约定：token 数为**个数**；`cost_micros` 为 **1e-6 元**；`output_tps` 为 **tokens / 秒**。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::store::{Result, Store};

/// 聚合维度（02 §8.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dim {
    /// 按 **UTC 日**（与 02 §8.2 的 UTC 日界一致）。
    Day,
    /// 按出站渠道。
    Channel,
    /// 按上游真实模型名（R-1 替换后的名字）。
    Model,
    /// 按入站 client key。
    Client,
}

impl Dim {
    /// 稳定字符串标识（IPC 与文档共用）。
    pub fn as_str(self) -> &'static str {
        match self {
            Dim::Day => "day",
            Dim::Channel => "channel",
            Dim::Model => "model",
            Dim::Client => "client",
        }
    }

    /// 解析维度标识。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "day" => Some(Dim::Day),
            "channel" => Some(Dim::Channel),
            "model" => Some(Dim::Model),
            "client" => Some(Dim::Client),
            _ => None,
        }
    }

    /// `GROUP BY` 表达式。
    ///
    /// **只返回固定字面量**（按枚举分支选定），不接受外部输入 → 无 SQL 注入面。
    /// 可空列一律 `COALESCE` 成 `(未记录)`，避免 NULL 单独成桶。
    fn group_expr(self) -> &'static str {
        match self {
            Dim::Day => "strftime('%Y-%m-%d', ts, 'unixepoch')",
            Dim::Channel => "COALESCE(channel_id, '(未记录)')",
            Dim::Model => "COALESCE(upstream_model, '(未记录)')",
            Dim::Client => "COALESCE(client_key_id, '(未记录)')",
        }
    }

    /// 该维度的桶是否按时间/用量升序（其余维度按用量降序）。
    fn sorted_ascending(self) -> bool {
        matches!(self, Dim::Day)
    }
}

/// 聚合查询参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateQuery {
    /// 聚合维度。
    pub dim: Dim,
    /// 起始时刻（**含**）；`None` = 不限。
    pub since_ts: Option<i64>,
    /// 结束时刻（**不含**）；`None` = 不限。
    pub until_ts: Option<i64>,
    /// 返回桶数上限；`None` = 用默认。
    pub limit: Option<i64>,
}

/// 默认时间窗：**近 30 天**（`since_ts` = 现在 − 30 天，`until_ts` = 不限）。
pub const DEFAULT_WINDOW_SECS: i64 = 30 * 24 * 3600;
/// 默认返回桶数上限。
pub const DEFAULT_BUCKET_LIMIT: i64 = 50;
/// 合计桶的固定 key（不与任何维度取值冲突）。
pub const TOTAL_KEY: &str = "__total__";

impl AggregateQuery {
    /// 构造查询（带默认时间窗与桶数上限）。
    pub fn new(dim: Dim) -> Self {
        Self {
            dim,
            since_ts: Some(crate::vault::now_secs() - DEFAULT_WINDOW_SECS),
            until_ts: None,
            limit: Some(DEFAULT_BUCKET_LIMIT),
        }
    }

    /// 生效的起始时刻（`None` = 不限）。
    pub fn effective_since(&self) -> Option<i64> {
        self.since_ts
    }

    /// 生效的桶数上限。
    pub fn effective_limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_BUCKET_LIMIT)
    }
}

/// 一个聚合桶（= 一个维度取值上的全部用量）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bucket {
    /// 稳定键（UTC 日串 / 渠道 id / 模型名 / client key id）。
    pub key: String,
    /// 可读名（渠道名 / client key 标签；查不到时回退为 `key`）。
    pub label: String,
    /// 请求数（= **出站尝试数**，一条 `usage_log` 一行）。
    pub requests: i64,
    /// 成功数。
    pub success_count: i64,
    /// 失败数（`status='error'`）。
    pub error_count: i64,
    /// 中途断连数（`status='aborted'`）。
    pub aborted_count: i64,
    /// 输入 token 合计。
    pub prompt_tokens: i64,
    /// 输出 token 合计。
    pub completion_tokens: i64,
    /// 缓存命中 token 合计（**是 `prompt_tokens` 的子集**，不重复计入总量）。
    pub cache_read_tokens: i64,
    /// 缓存写入 token 合计。
    pub cache_write_tokens: i64,
    /// 推理 token 合计。
    pub reasoning_tokens: i64,
    /// 总 token 合计（= 输入 + 输出；缓存读写已含在输入里，不重复计）。
    pub total_tokens: i64,
    /// **已知**成本合计（1e-6 元；`cost_micros` 为 NULL 的行按 0 计入本项）。
    pub cost_micros: i64,
    /// **成本未知**的行数（`cost_micros IS NULL`，即没命中价目表）。
    pub cost_unknown_rows: i64,
    /// 估算行数（`estimated=1`）。
    pub estimated_rows: i64,
    /// TTFT 中位数（毫秒；只在有 TTFT 的行上算，无数据为 `None`）。
    pub ttft_p50_ms: Option<i64>,
    /// TTFT P95（毫秒；同上）。
    pub ttft_p95_ms: Option<i64>,
    /// 输出速度（tokens/秒；成功行按 `输出 token ÷ 耗时` 加权，无数据为 `None`）。
    pub output_tps: Option<f64>,
}

/// 聚合结果：分桶 + 全范围合计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateResult {
    /// 维度标识。
    pub dim: String,
    /// 生效的起始时刻（`None` = 不限）。
    pub since_ts: Option<i64>,
    /// 生效的结束时刻（`None` = 不限）。
    pub until_ts: Option<i64>,
    /// 分桶（受 `limit` 限制）。
    pub buckets: Vec<Bucket>,
    /// **全范围合计**（不受 `limit` 影响）。
    pub total: Bucket,
}

/// 最近一条请求的明细（M2 判据要显示的四项都在这里）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentRequest {
    /// 行 id。
    pub id: i64,
    /// 时刻（Unix 秒）。
    pub ts: i64,
    /// 入站 client key id。
    pub client_key_id: Option<String>,
    /// client key 标签（查不到时回退为 id）。
    pub client_key_label: Option<String>,
    /// 出站渠道 id。
    pub channel_id: Option<String>,
    /// 渠道名（查不到时回退为 id）。
    pub channel_name: Option<String>,
    /// 入站别名。
    pub model_alias: Option<String>,
    /// 上游真实模型名。
    pub upstream_model: Option<String>,
    /// 入站协议。
    pub protocol: Option<String>,
    /// 输入 token。
    pub prompt_tokens: i64,
    /// 输出 token。
    pub completion_tokens: i64,
    /// 缓存命中 token。
    pub cache_read_tokens: i64,
    /// 总 token（= 输入 + 输出）。
    pub total_tokens: i64,
    /// 是否为本地估算值（上游没回 usage）。
    pub estimated: bool,
    /// 首个 token 到达耗时（毫秒；非流式为 `None`）。
    pub ttft_ms: Option<i64>,
    /// 本次出站尝试耗时（毫秒）。
    pub duration_ms: Option<i64>,
    /// 输出速度（tokens/秒；**仅成功且有耗时**才有值）。
    pub output_tps: Option<f64>,
    /// 状态（`success` / `error` / `aborted`）。
    pub status: Option<String>,
    /// 失败时的错误类别。
    pub error_kind: Option<String>,
    /// 成本（1e-6 元；没命中价目表为 `None` = 未知）。
    pub cost_micros: Option<i64>,
}

/// 聚合指标列（`GROUP BY` 与合计查询共用，保证两处口径一致）。
const METRIC_COLUMNS: &str = "
    COUNT(*) AS requests,
    SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) AS success_count,
    SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) AS error_count,
    SUM(CASE WHEN status = 'aborted' THEN 1 ELSE 0 END) AS aborted_count,
    SUM(COALESCE(prompt_tokens, 0)) AS prompt_tokens,
    SUM(COALESCE(completion_tokens, 0)) AS completion_tokens,
    SUM(COALESCE(cache_read_tokens, 0)) AS cache_read_tokens,
    SUM(COALESCE(cache_write_tokens, 0)) AS cache_write_tokens,
    SUM(COALESCE(reasoning_tokens, 0)) AS reasoning_tokens,
    SUM(COALESCE(cost_micros, 0)) AS cost_micros,
    SUM(CASE WHEN cost_micros IS NULL THEN 1 ELSE 0 END) AS cost_unknown_rows,
    SUM(CASE WHEN estimated = 1 THEN 1 ELSE 0 END) AS estimated_rows,
    SUM(CASE WHEN status = 'success' AND duration_ms > 0
             THEN COALESCE(completion_tokens, 0) ELSE 0 END) AS ok_completion,
    SUM(CASE WHEN status = 'success' AND duration_ms > 0
             THEN duration_ms ELSE 0 END) AS ok_duration";

/// 时间范围条件（`since_ts` 含 / `until_ts` 不含；`None` = 不限）。
const RANGE_CONDITION: &str = "(?1 IS NULL OR ts >= ?1) AND (?2 IS NULL OR ts < ?2)";

/// 按维度聚合用量（02 §8.4）。
pub fn aggregate(store: &Store, query: &AggregateQuery) -> Result<AggregateResult> {
    let grp = query.dim.group_expr();
    let since = query.effective_since();
    let until = query.until_ts;

    store.with_conn(|conn| {
        // ── 1. 分桶指标 ──
        let sql = format!(
            "SELECT {grp} AS bucket_key, {METRIC_COLUMNS}
             FROM usage_log
             WHERE {RANGE_CONDITION}
             GROUP BY {grp}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params![since, until], |row| {
            let ok_completion: i64 = row.get(13)?;
            let ok_duration: i64 = row.get(14)?;
            Ok(Bucket {
                key: row.get(0)?,
                label: String::new(),
                requests: row.get(1)?,
                success_count: row.get(2)?,
                error_count: row.get(3)?,
                aborted_count: row.get(4)?,
                prompt_tokens: row.get(5)?,
                completion_tokens: row.get(6)?,
                cache_read_tokens: row.get(7)?,
                cache_write_tokens: row.get(8)?,
                reasoning_tokens: row.get(9)?,
                total_tokens: 0,
                cost_micros: row.get(10)?,
                cost_unknown_rows: row.get(11)?,
                estimated_rows: row.get(12)?,
                ttft_p50_ms: None,
                ttft_p95_ms: None,
                output_tps: output_tps(ok_completion, ok_duration),
            })
        })?;

        let mut buckets: Vec<Bucket> = Vec::new();
        for row in rows {
            let mut bucket = row?;
            bucket.total_tokens = bucket.prompt_tokens + bucket.completion_tokens;
            buckets.push(bucket);
        }

        // ── 2. TTFT 分位（SQLite 无分位函数，取出原始值在内存里算）──
        let ttft_sql = format!(
            "SELECT {grp} AS bucket_key, ttft_ms FROM usage_log
             WHERE ttft_ms IS NOT NULL AND {RANGE_CONDITION}"
        );
        let mut ttft_stmt = conn.prepare(&ttft_sql)?;
        let ttft_rows = ttft_stmt.query_map(rusqlite::params![since, until], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut ttft_map: HashMap<String, Vec<i64>> = HashMap::new();
        for row in ttft_rows {
            let (key, value) = row?;
            ttft_map.entry(key).or_default().push(value);
        }
        for bucket in buckets.iter_mut() {
            if let Some(values) = ttft_map.get_mut(&bucket.key) {
                values.sort_unstable();
                bucket.ttft_p50_ms = percentile(values, 0.50);
                bucket.ttft_p95_ms = percentile(values, 0.95);
            }
        }

        // ── 3. 可读标签 ──
        let channel_names = load_channel_names(conn)?;
        let client_labels = load_client_labels(conn)?;
        for bucket in buckets.iter_mut() {
            bucket.label = match query.dim {
                Dim::Channel => channel_names
                    .get(&bucket.key)
                    .cloned()
                    .unwrap_or_else(|| bucket.key.clone()),
                Dim::Client => client_labels
                    .get(&bucket.key)
                    .cloned()
                    .unwrap_or_else(|| bucket.key.clone()),
                _ => bucket.key.clone(),
            };
        }

        // ── 4. 排序 + 截断 ──
        if query.dim.sorted_ascending() {
            buckets.sort_by(|a, b| a.key.cmp(&b.key));
        } else {
            buckets.sort_by(|a, b| {
                b.total_tokens
                    .cmp(&a.total_tokens)
                    .then_with(|| a.key.cmp(&b.key))
            });
        }
        let limit = query.effective_limit();
        if limit > 0 && buckets.len() > limit as usize {
            buckets.truncate(limit as usize);
        }

        // ── 5. 全范围合计（不受 limit 影响）──
        let total_sql = format!(
            "SELECT {METRIC_COLUMNS} FROM usage_log WHERE {RANGE_CONDITION}"
        );
        let mut total_stmt = conn.prepare(&total_sql)?;
        let mut total = total_stmt
            .query_row(rusqlite::params![since, until], |row| {
                let ok_completion: i64 = row.get(12)?;
                let ok_duration: i64 = row.get(13)?;
                Ok(Bucket {
                    key: TOTAL_KEY.to_string(),
                    label: "合计".to_string(),
                    requests: row.get(0)?,
                    success_count: row.get(1)?,
                    error_count: row.get(2)?,
                    aborted_count: row.get(3)?,
                    prompt_tokens: row.get(4)?,
                    completion_tokens: row.get(5)?,
                    cache_read_tokens: row.get(6)?,
                    cache_write_tokens: row.get(7)?,
                    reasoning_tokens: row.get(8)?,
                    total_tokens: 0,
                    cost_micros: row.get(9)?,
                    cost_unknown_rows: row.get(10)?,
                    estimated_rows: row.get(11)?,
                    ttft_p50_ms: None,
                    ttft_p95_ms: None,
                    output_tps: output_tps(ok_completion, ok_duration),
                })
            })
            .unwrap_or_else(|_| empty_bucket(TOTAL_KEY, "合计"));
        total.total_tokens = total.prompt_tokens + total.completion_tokens;

        // 合计的 TTFT 分位在全范围上重算
        let mut all_ttft: Vec<i64> = ttft_map.values().flatten().copied().collect();
        all_ttft.sort_unstable();
        total.ttft_p50_ms = percentile(&all_ttft, 0.50);
        total.ttft_p95_ms = percentile(&all_ttft, 0.95);

        Ok(AggregateResult {
            dim: query.dim.as_str().to_string(),
            since_ts: since,
            until_ts: until,
            buckets,
            total,
        })
    })
}

/// 最近的请求明细（按时间倒序）。
pub fn recent_requests(store: &Store, limit: i64) -> Result<Vec<RecentRequest>> {
    let limit = limit.clamp(1, 500);
    store.with_conn(|conn| {
        let channel_names = load_channel_names(conn)?;
        let client_labels = load_client_labels(conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, ts, client_key_id, channel_id, model_alias, upstream_model, protocol,
                    COALESCE(prompt_tokens, 0), COALESCE(completion_tokens, 0),
                    COALESCE(cache_read_tokens, 0), COALESCE(estimated, 0),
                    ttft_ms, duration_ms, status, error_kind, cost_micros
             FROM usage_log
             ORDER BY ts DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![limit], |row| {
            let prompt_tokens: i64 = row.get(7)?;
            let completion_tokens: i64 = row.get(8)?;
            let duration_ms: Option<i64> = row.get(12)?;
            let status: Option<String> = row.get(13)?;
            let output_tps = match (status.as_deref(), duration_ms) {
                (Some("success"), Some(duration)) if duration > 0 && completion_tokens > 0 => {
                    Some(completion_tokens as f64 / (duration as f64 / 1000.0))
                }
                _ => None,
            };
            Ok(RecentRequest {
                id: row.get(0)?,
                ts: row.get(1)?,
                client_key_id: row.get(2)?,
                client_key_label: None,
                channel_id: row.get(3)?,
                channel_name: None,
                model_alias: row.get(4)?,
                upstream_model: row.get(5)?,
                protocol: row.get(6)?,
                prompt_tokens,
                completion_tokens,
                cache_read_tokens: row.get(9)?,
                total_tokens: prompt_tokens + completion_tokens,
                estimated: row.get::<_, i64>(10)? != 0,
                ttft_ms: row.get(11)?,
                duration_ms,
                output_tps,
                status,
                error_kind: row.get(14)?,
                cost_micros: row.get(15)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let mut item = row?;
            if let Some(id) = &item.channel_id {
                item.channel_name = Some(
                    channel_names
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| id.clone()),
                );
            }
            if let Some(id) = &item.client_key_id {
                item.client_key_label = Some(
                    client_labels
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| id.clone()),
                );
            }
            out.push(item);
        }
        Ok(out)
    })
}

/// 输出速度（tokens/秒）：成功行的 `输出 token ÷ 耗时` 加权平均。
///
/// 只统计 `status='success' AND duration_ms > 0` 的行——失败行没有产出，
/// 混进去会把速度拉低到没有意义。无有效样本返回 `None`（**不返回 0**）。
fn output_tps(ok_completion: i64, ok_duration_ms: i64) -> Option<f64> {
    if ok_duration_ms <= 0 || ok_completion <= 0 {
        return None;
    }
    Some(ok_completion as f64 * 1000.0 / ok_duration_ms as f64)
}

/// 最近秩法的分位数（p ∈ (0, 1]）。
///
/// 排序后取第 `ceil(p × n)` 个（1 起）——`p50` 即中位数，`p95` 即 P95。
/// 空样本返回 `None`（**不返回 0**：没有 TTFT 与 TTFT 为 0 是两回事）。
fn percentile(sorted: &[i64], p: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    Some(sorted[idx])
}

/// 渠道 id → 渠道名。
fn load_channel_names(conn: &rusqlite::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, name FROM channel")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, name) = row?;
        out.insert(id, name);
    }
    Ok(out)
}

/// client key id → 标签。
fn load_client_labels(conn: &rusqlite::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, label FROM client_key")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, label) = row?;
        out.insert(id, label);
    }
    Ok(out)
}

/// 空桶（全 0）。
fn empty_bucket(key: &str, label: &str) -> Bucket {
    Bucket {
        key: key.to_string(),
        label: label.to_string(),
        requests: 0,
        success_count: 0,
        error_count: 0,
        aborted_count: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 0,
        cost_micros: 0,
        cost_unknown_rows: 0,
        estimated_rows: 0,
        ttft_p50_ms: None,
        ttft_p95_ms: None,
        output_tps: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::store::usage::{insert_usage, UsageRecord, UsageStatus};

    fn store_with_rows() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO channel (id, name, kind, provider, base_url, created_at)
                     VALUES ('c1', 'DeepSeek 官方', 'api_key', 'deepseek', 'https://example.invalid', 0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO channel (id, name, kind, provider, base_url, created_at)
                     VALUES ('c2', 'Kimi 官方', 'api_key', 'kimi', 'https://example.invalid', 0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO client_key (id, label, key_hash, key_prefix, created_at)
                     VALUES ('ck1', 'pi-主力', 'hash-1', 'llmr_abc', 0)",
                    [],
                )?;
                Ok(())
            })
            .expect("写入渠道与 client key");
        (dir, store)
    }

    fn record(
        ts: i64,
        channel: &str,
        model: &str,
        prompt: i64,
        completion: i64,
        estimated: bool,
        ttft_ms: Option<i64>,
        duration_ms: i64,
        status: UsageStatus,
    ) -> UsageRecord {
        UsageRecord {
            ts,
            client_key_id: Some("ck1".to_string()),
            channel_id: channel.to_string(),
            account_id: format!("acct_default_{channel}"),
            model_alias: "alias".to_string(),
            upstream_model: model.to_string(),
            protocol: "openai".to_string(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            estimated,
            ttft_ms,
            duration_ms,
            status,
            error_kind: None,
        }
    }

    fn all_time(dim: Dim) -> AggregateQuery {
        AggregateQuery {
            dim,
            since_ts: None,
            until_ts: None,
            limit: Some(50),
        }
    }

    #[test]
    fn dim_parse_covers_four_dimensions() {
        assert_eq!(Dim::parse("day"), Some(Dim::Day));
        assert_eq!(Dim::parse("channel"), Some(Dim::Channel));
        assert_eq!(Dim::parse("model"), Some(Dim::Model));
        assert_eq!(Dim::parse("client"), Some(Dim::Client));
        assert_eq!(Dim::parse("nope"), None);
    }

    #[test]
    fn aggregates_by_channel_and_resolves_names() {
        let (_dir, store) = store_with_rows();
        insert_usage(
            &store,
            &record(100, "c1", "deepseek-flash", 10, 20, false, Some(50), 1000, UsageStatus::Success),
        )
        .expect("写用量");
        insert_usage(
            &store,
            &record(200, "c1", "deepseek-flash", 30, 40, false, Some(150), 2000, UsageStatus::Success),
        )
        .expect("写用量");
        insert_usage(
            &store,
            &record(300, "c2", "kimi-k3", 5, 5, false, None, 500, UsageStatus::Error),
        )
        .expect("写用量");

        let result = aggregate(&store, &all_time(Dim::Channel)).expect("聚合");
        assert_eq!(result.buckets.len(), 2);

        let deepseek = result
            .buckets
            .iter()
            .find(|b| b.key == "c1")
            .expect("应有 c1 桶");
        assert_eq!(deepseek.label, "DeepSeek 官方");
        assert_eq!(deepseek.requests, 2);
        assert_eq!(deepseek.success_count, 2);
        assert_eq!(deepseek.prompt_tokens, 40);
        assert_eq!(deepseek.completion_tokens, 60);
        assert_eq!(deepseek.total_tokens, 100);

        let kimi = result
            .buckets
            .iter()
            .find(|b| b.key == "c2")
            .expect("应有 c2 桶");
        assert_eq!(kimi.label, "Kimi 官方");
        assert_eq!(kimi.error_count, 1);
    }

    #[test]
    fn groups_by_utc_day() {
        let (_dir, store) = store_with_rows();
        // 2024-01-01T00:00:00Z = 1704067200
        insert_usage(
            &store,
            &record(1_704_067_200, "c1", "m", 1, 1, false, None, 10, UsageStatus::Success),
        )
        .expect("写用量");
        // +1 天
        insert_usage(
            &store,
            &record(1_704_067_200 + 86_400, "c1", "m", 2, 2, false, None, 10, UsageStatus::Success),
        )
        .expect("写用量");

        let result = aggregate(&store, &all_time(Dim::Day)).expect("聚合");
        assert_eq!(result.buckets.len(), 2);
        assert_eq!(result.buckets[0].key, "2024-01-01");
        assert_eq!(result.buckets[1].key, "2024-01-02");
        // 日维度按日期升序
        assert!(result.buckets[0].key < result.buckets[1].key);
    }

    #[test]
    fn groups_by_client_key_with_label() {
        let (_dir, store) = store_with_rows();
        insert_usage(
            &store,
            &record(100, "c1", "m", 7, 9, false, None, 10, UsageStatus::Success),
        )
        .expect("写用量");
        let result = aggregate(&store, &all_time(Dim::Client)).expect("聚合");
        assert_eq!(result.buckets.len(), 1);
        assert_eq!(result.buckets[0].key, "ck1");
        assert_eq!(result.buckets[0].label, "pi-主力");
    }

    #[test]
    fn ttft_percentiles_use_nearest_rank() {
        let (_dir, store) = store_with_rows();
        for (i, ttft) in [10, 20, 30, 40].iter().enumerate() {
            insert_usage(
                &store,
                &record(
                    100 + i as i64,
                    "c1",
                    "m",
                    1,
                    1,
                    false,
                    Some(*ttft),
                    100,
                    UsageStatus::Success,
                ),
            )
            .expect("写用量");
        }
        let result = aggregate(&store, &all_time(Dim::Channel)).expect("聚合");
        let bucket = &result.buckets[0];
        // 排序后 [10,20,30,40]：p50 → ceil(0.5*4)=2 → 20；p95 → ceil(0.95*4)=4 → 40
        assert_eq!(bucket.ttft_p50_ms, Some(20));
        assert_eq!(bucket.ttft_p95_ms, Some(40));
    }

    #[test]
    fn ttft_is_none_when_no_row_has_it() {
        let (_dir, store) = store_with_rows();
        insert_usage(
            &store,
            &record(100, "c1", "m", 1, 1, false, None, 100, UsageStatus::Success),
        )
        .expect("写用量");
        let result = aggregate(&store, &all_time(Dim::Channel)).expect("聚合");
        assert_eq!(result.buckets[0].ttft_p50_ms, None);
        assert_eq!(result.buckets[0].ttft_p95_ms, None);
    }

    #[test]
    fn output_tps_ignores_failed_rows() {
        let (_dir, store) = store_with_rows();
        insert_usage(
            &store,
            &record(100, "c1", "m", 0, 100, false, Some(10), 1000, UsageStatus::Success),
        )
        .expect("写用量");
        // 失败行：即使有耗时也不该计入速度
        insert_usage(
            &store,
            &record(200, "c1", "m", 0, 999, false, None, 1000, UsageStatus::Error),
        )
        .expect("写用量");

        let result = aggregate(&store, &all_time(Dim::Channel)).expect("聚合");
        let bucket = &result.buckets[0];
        let tps = bucket.output_tps.expect("应有输出速度");
        assert!(
            (tps - 100.0).abs() < 1e-6,
            "只按成功行算：100 token / 1s = 100，实际 {tps}"
        );
    }

    #[test]
    fn cost_unknown_rows_are_counted_separately() {
        let (_dir, store) = store_with_rows();
        // deepseek-flash 有预填牌价 → 有成本
        insert_usage(
            &store,
            &record(100, "c1", "deepseek-flash", 1_000_000, 1_000_000, false, None, 100, UsageStatus::Success),
        )
        .expect("写用量");
        // 没配牌价的模型 → cost_micros 为 NULL（未知）
        insert_usage(
            &store,
            &record(200, "c1", "某个没配牌价的模型", 1_000, 1_000, false, None, 100, UsageStatus::Success),
        )
        .expect("写用量");

        let result = aggregate(&store, &all_time(Dim::Channel)).expect("聚合");
        let bucket = &result.buckets[0];
        assert_eq!(bucket.cost_unknown_rows, 1, "未命中价目表的行应单独计数");
        assert_eq!(bucket.cost_micros, 10_000_000, "已知成本 = 2 元 + 8 元");
    }

    #[test]
    fn estimated_rows_are_counted() {
        let (_dir, store) = store_with_rows();
        insert_usage(
            &store,
            &record(100, "c1", "m", 1, 1, true, None, 10, UsageStatus::Success),
        )
        .expect("写用量");
        insert_usage(
            &store,
            &record(200, "c1", "m", 1, 1, false, None, 10, UsageStatus::Success),
        )
        .expect("写用量");
        let result = aggregate(&store, &all_time(Dim::Channel)).expect("聚合");
        assert_eq!(result.buckets[0].estimated_rows, 1);
    }

    #[test]
    fn time_range_filters_rows() {
        let (_dir, store) = store_with_rows();
        insert_usage(
            &store,
            &record(100, "c1", "m", 1, 1, false, None, 10, UsageStatus::Success),
        )
        .expect("写用量");
        insert_usage(
            &store,
            &record(1_000, "c1", "m", 1, 1, false, None, 10, UsageStatus::Success),
        )
        .expect("写用量");

        let query = AggregateQuery {
            dim: Dim::Channel,
            since_ts: Some(500),
            until_ts: None,
            limit: Some(50),
        };
        let result = aggregate(&store, &query).expect("聚合");
        assert_eq!(result.buckets[0].requests, 1, "since 之前的行应被排除");
        assert_eq!(result.total.requests, 1, "合计也遵守时间范围");
    }

    #[test]
    fn total_is_not_capped_by_limit() {
        let (_dir, store) = store_with_rows();
        for i in 0..5 {
            insert_usage(
                &store,
                &record(100 + i, "c1", "m", 1, 1, false, None, 10, UsageStatus::Success),
            )
            .expect("写用量");
        }
        let query = AggregateQuery {
            dim: Dim::Model,
            since_ts: None,
            until_ts: None,
            limit: Some(1),
        };
        let result = aggregate(&store, &query).expect("聚合");
        assert_eq!(result.buckets.len(), 1, "桶受 limit 限制");
        assert_eq!(result.total.requests, 5, "合计不受 limit 影响");
    }

    #[test]
    fn recent_requests_are_sorted_newest_first_with_metrics() {
        let (_dir, store) = store_with_rows();
        insert_usage(
            &store,
            &record(100, "c1", "m", 10, 20, false, Some(30), 1000, UsageStatus::Success),
        )
        .expect("写用量");
        insert_usage(
            &store,
            &record(200, "c2", "m2", 1, 2, true, None, 500, UsageStatus::Error),
        )
        .expect("写用量");

        let rows = recent_requests(&store, 10).expect("读明细");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts, 200, "新的在前");
        assert_eq!(rows[0].estimated, true);
        assert_eq!(rows[0].output_tps, None, "失败行没有输出速度");
        assert_eq!(rows[0].channel_name.as_deref(), Some("Kimi 官方"));
        assert_eq!(rows[1].total_tokens, 30);
        let tps = rows[1].output_tps.expect("成功行应有输出速度");
        assert!((tps - 20.0).abs() < 1e-6, "20 token / 1s = 20，实际 {tps}");
        assert_eq!(rows[1].client_key_label.as_deref(), Some("pi-主力"));
    }

    #[test]
    fn empty_table_aggregates_to_zero_total() {
        let (_dir, store) = store_with_rows();
        let result = aggregate(&store, &all_time(Dim::Day)).expect("聚合");
        assert!(result.buckets.is_empty());
        assert_eq!(result.total.requests, 0);
        assert_eq!(result.total.cost_micros, 0);
        assert_eq!(result.total.ttft_p50_ms, None);
    }

    #[test]
    fn output_tps_is_none_when_duration_is_zero() {
        let (_dir, store) = store_with_rows();
        // 耗时为 0（本地 mock 亚毫秒响应）→ 速度无从计算 → None
        // （**不是 0**，更不是无穷大）
        insert_usage(
            &store,
            &record(100, "c1", "m", 0, 50, false, None, 0, UsageStatus::Success),
        )
        .expect("写用量");
        let result = aggregate(&store, &all_time(Dim::Channel)).expect("聚合");
        assert_eq!(result.buckets[0].output_tps, None);
        assert_eq!(result.total.output_tps, None);
        let rows = recent_requests(&store, 10).expect("读明细");
        assert_eq!(rows[0].output_tps, None);
    }

    #[test]
    fn percentile_handles_empty_and_single() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[7], 0.5), Some(7));
        assert_eq!(percentile(&[7], 0.95), Some(7));
    }
}
