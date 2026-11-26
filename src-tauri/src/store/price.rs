//! 价目表与成本换算（T13；02 §8.3）。
//!
//! - **价目表**：`price` 表，主键 `(provider, model_pattern)`，用户可编辑，预填国产渠道公开牌价。
//! - **匹配**：`model_pattern` 里的 `*` 通配任意字符串；不含 `*` 即精确匹配。
//!   命中多条时取**最具体**的一条（通配符少 → 字面量长 → 字典序）。
//! - **换算**：输入 / 输出 / 缓存读三档价，换算出 `cost_micros`（1e-6 元）。
//!
//! 单位约定：`input_per_1m` 等字段的单位是 **1e-6 元 / 1M tokens**，
//! 即 1 元/1M tokens 记 `1_000_000`。换算时 `cost = tokens × 单价 / 1_000_000`。

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::{Result, Store};

/// 一条价目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceEntry {
    /// 渠道 provider（预设标识，如 `deepseek` / `kimi` / `glm`）。
    pub provider: String,
    /// 模型名匹配模式（`*` 通配）。
    pub model_pattern: String,
    /// 输入单价（1e-6 元 / 1M tokens）。
    pub input_per_1m: i64,
    /// 输出单价（1e-6 元 / 1M tokens）。
    pub output_per_1m: i64,
    /// 缓存命中单价（1e-6 元 / 1M tokens）。
    pub cache_read_per_1m: i64,
}

/// 参与计费的 token 数（02 §8.3）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BillableTokens {
    /// 输入 token（**含**缓存命中部分）。
    pub prompt_tokens: i64,
    /// 输出 token。
    pub completion_tokens: i64,
    /// 缓存命中 token（是 `prompt_tokens` 的子集）。
    pub cache_read_tokens: i64,
    /// 缓存写入 token。
    pub cache_write_tokens: i64,
}

/// 预填的国产渠道牌价（快照 **2026-11-26**）。
///
/// 取价规则：**取主列表价**——不取促销档 / 限时折扣档 / 时段折扣档；分段计价取第一档。
/// 数值单位：1e-6 元 / 1M tokens（`2 元` 记 `2_000_000`）。
///
/// 来源（均为厂商官方页面，核验于 2026-11-26）：
/// - `deepseek`：`api-docs.deepseek.com/zh-cn/quick_start/pricing`（**高峰价**；空闲时段为高峰的一半）
/// - `kimi`：`platform.kimi.com/docs/pricing/chat`
/// - `glm`：`docs.bigmodel.cn/cn/guide/start/pricing`（分段计价取第一档）
/// - `dashscope`：`bailian.console.aliyun.com` 模型价格页（qwen-plus「输入 ≤128k」档）
/// - `minimax_cn`：`platform.minimax.cn/docs/guides/pricing-paygo`（MiniMax-M3「≤512K 输入」档，永久五折后现价）
/// - `siliconflow`：`siliconflow.cn/pricing`
///
/// `minimax_intl`（国际站，USD 计价）**不在预填范围**——本表以元计价、不做汇率折算。
pub const DEFAULT_PRICES: &[(&str, &str, i64, i64, i64)] = &[
    // ── DeepSeek 官方（高峰价；空闲时段为一半）──
    // deepseek-flash：输入 2 元、输出 8 元、缓存命中 0.04 元
    ("deepseek", "deepseek-flash", 2_000_000, 8_000_000, 40_000),
    // deepseek-v4-flash*：官方注明旧模型名仍可调用并按 Flash 价计费
    ("deepseek", "deepseek-v4-flash*", 2_000_000, 8_000_000, 40_000),
    // deepseek-v4-pro：输入 9 元、输出 27 元、缓存命中 0.30 元
    ("deepseek", "deepseek-v4-pro", 9_000_000, 27_000_000, 300_000),
    // ── Kimi 官方 ──
    // kimi-k3.1-pro：输入 13 元、输出 68 元、缓存命中 1.30 元
    ("kimi", "kimi-k3.1-pro", 13_000_000, 68_000_000, 1_300_000),
    // kimi-k3.1-pro-highspeed：输入 26 元、输出 136 元、缓存命中 1.30 元
    ("kimi", "kimi-k3.1-pro-highspeed", 26_000_000, 136_000_000, 1_300_000),
    // kimi-k3：输入 20 元、输出 100 元、缓存命中 2.00 元
    ("kimi", "kimi-k3", 20_000_000, 100_000_000, 2_000_000),
    // kimi-k2.7-code：输入 6.5 元、输出 27 元、缓存命中 1.30 元
    ("kimi", "kimi-k2.7-code", 6_500_000, 27_000_000, 1_300_000),
    // kimi-k2.6：输入 6.5 元、输出 27 元、缓存命中 1.10 元
    ("kimi", "kimi-k2.6", 6_500_000, 27_000_000, 1_100_000),
    // kimi-k2.5：输入 4 元、输出 21 元、缓存命中 0.70 元
    ("kimi", "kimi-k2.5", 4_000_000, 21_000_000, 700_000),
    // ── 智谱 GLM 官方（分段计价，取第一档）──
    // glm-5.3：输入 8 元、输出 28 元、缓存命中 2 元
    ("glm", "glm-5.3", 8_000_000, 28_000_000, 2_000_000),
    // glm-5.3-flash：输入 0.8 元、输出 2.8 元、缓存命中 0.23 元
    ("glm", "glm-5.3-flash", 800_000, 2_800_000, 230_000),
    // glm-5.3-flash-x：输入 2 元、输出 7 元、缓存命中 0.57 元
    ("glm", "glm-5.3-flash-x", 2_000_000, 7_000_000, 570_000),
    // glm-5.2：输入 8 元、输出 28 元、缓存命中 2 元
    ("glm", "glm-5.2", 8_000_000, 28_000_000, 2_000_000),
    // glm-5.1（<32K 档）：输入 6 元、输出 24 元、缓存命中 1.3 元
    ("glm", "glm-5.1", 6_000_000, 24_000_000, 1_300_000),
    // glm-5-turbo（<32K 档）：输入 5 元、输出 22 元、缓存命中 1.2 元
    ("glm", "glm-5-turbo", 5_000_000, 22_000_000, 1_200_000),
    // glm-5（<32K 档）：输入 4 元、输出 18 元、缓存命中 1 元
    ("glm", "glm-5", 4_000_000, 18_000_000, 1_000_000),
    // glm-4.7（输入<32K 且输出<0.2K 档）：输入 2 元、输出 8 元、缓存命中 0.4 元
    ("glm", "glm-4.7", 2_000_000, 8_000_000, 400_000),
    // glm-4.6v（<32K 档）：输入 1 元、输出 3 元、缓存命中 0.2 元
    ("glm", "glm-4.6v", 1_000_000, 3_000_000, 200_000),
    // glm-4.7-flash：官方长期免费
    ("glm", "glm-4.7-flash", 0, 0, 0),
    // ── 阿里云百炼 ──
    // qwen-plus（输入 ≤128k 档，非思考模式）：输入 0.8 元、输出 2 元、缓存命中 0.16 元
    ("dashscope", "qwen-plus", 800_000, 2_000_000, 160_000),
    // ── MiniMax 国内站 ──
    // MiniMax-M3.1-Flash：输入 1 元、输出 4 元、缓存读取 0.10 元
    ("minimax_cn", "MiniMax-M3.1-Flash", 1_000_000, 4_000_000, 100_000),
    // MiniMax-M3（≤512K 输入，永久五折后现价）：输入 2.1 元、输出 8.4 元、缓存读取 0.42 元
    ("minimax_cn", "MiniMax-M3", 2_100_000, 8_400_000, 420_000),
    // ── 硅基流动 ──
    // deepseek-ai/DeepSeek-V4.1-Flash（主档）：输入 2 元、输出 8 元、缓存 0.04 元
    (
        "siliconflow",
        "deepseek-ai/DeepSeek-V4.1-Flash",
        2_000_000,
        8_000_000,
        40_000,
    ),
    // deepseek-ai/DeepSeek-V4-Flash（主档）：输入 3 元、输出 9 元、缓存 0.30 元
    (
        "siliconflow",
        "deepseek-ai/DeepSeek-V4-Flash",
        3_000_000,
        9_000_000,
        300_000,
    ),
    // deepseek-ai/DeepSeek-V4-Pro：输入 12 元、输出 24 元、缓存 1 元
    (
        "siliconflow",
        "deepseek-ai/DeepSeek-V4-Pro",
        12_000_000,
        24_000_000,
        1_000_000,
    ),
    // deepseek-ai/DeepSeek-V3.2：输入 4 元、输出 6 元、缓存 0.40 元
    (
        "siliconflow",
        "deepseek-ai/DeepSeek-V3.2",
        4_000_000,
        6_000_000,
        400_000,
    ),
    // Pro/deepseek-ai/DeepSeek-V3.2：输入 4 元、输出 6 元、缓存 0.40 元
    (
        "siliconflow",
        "Pro/deepseek-ai/DeepSeek-V3.2",
        4_000_000,
        6_000_000,
        400_000,
    ),
    // deepseek-ai/DeepSeek-V3.1-Terminus：输入 4 元、输出 12 元、缓存 0.40 元
    (
        "siliconflow",
        "deepseek-ai/DeepSeek-V3.1-Terminus",
        4_000_000,
        12_000_000,
        400_000,
    ),
    // zai-org/GLM-5.3-Flash：输入 0.8 元、输出 2.8 元、缓存 0.23 元
    (
        "siliconflow",
        "zai-org/GLM-5.3-Flash",
        800_000,
        2_800_000,
        230_000,
    ),
    // zai-org/GLM-5.3：输入 8 元、输出 28 元、缓存 2 元
    ("siliconflow", "zai-org/GLM-5.3", 8_000_000, 28_000_000, 2_000_000),
    // zai-org/GLM-5.2：输入 8 元、输出 28 元、缓存 2 元
    ("siliconflow", "zai-org/GLM-5.2", 8_000_000, 28_000_000, 2_000_000),
    // zai-org/GLM-4.5-Air：输入 1 元、输出 6 元（官方未列缓存价）
    ("siliconflow", "zai-org/GLM-4.5-Air", 1_000_000, 6_000_000, 0),
    // THUDM/GLM-4-32B-0414：输入 1.89 元、输出 1.89 元
    (
        "siliconflow",
        "THUDM/GLM-4-32B-0414",
        1_890_000,
        1_890_000,
        0,
    ),
    // THUDM/GLM-Z1-9B-0414：官方标注免费
    ("siliconflow", "THUDM/GLM-Z1-9B-0414", 0, 0, 0),
    // tencent/Hy4-preview：输入 6 元、输出 18 元、缓存 0.30 元
    (
        "siliconflow",
        "tencent/Hy4-preview",
        6_000_000,
        18_000_000,
        300_000,
    ),
    // tencent/Hunyuan-MT-7B：官方标注免费
    ("siliconflow", "tencent/Hunyuan-MT-7B", 0, 0, 0),
    // meituan-longcat/LongCat-2.0：输入 5 元、输出 20 元、缓存 0.10 元
    (
        "siliconflow",
        "meituan-longcat/LongCat-2.0",
        5_000_000,
        20_000_000,
        100_000,
    ),
    // moonshotai/Kimi-K2.7-Code：输入 6.5 元、输出 27 元、缓存 1.30 元
    (
        "siliconflow",
        "moonshotai/Kimi-K2.7-Code",
        6_500_000,
        27_000_000,
        1_300_000,
    ),
    // Pro/moonshotai/Kimi-K2.6：输入 6.5 元、输出 27 元、缓存 1.10 元
    (
        "siliconflow",
        "Pro/moonshotai/Kimi-K2.6",
        6_500_000,
        27_000_000,
        1_100_000,
    ),
    // Qwen/Qwen3.8-27B：输入 3 元、输出 12 元
    ("siliconflow", "Qwen/Qwen3.8-27B", 3_000_000, 12_000_000, 0),
    // Qwen/Qwen3.6-35B-A3B：输入 1.8 元、输出 10.8 元
    ("siliconflow", "Qwen/Qwen3.6-35B-A3B", 1_800_000, 10_800_000, 0),
    // Qwen/Qwen3.6-27B：输入 3 元、输出 18 元
    ("siliconflow", "Qwen/Qwen3.6-27B", 3_000_000, 18_000_000, 0),
    // Qwen/Qwen3.5-35B-A3B（输入 <128k 档）：输入 0.4 元、输出 3.2 元
    ("siliconflow", "Qwen/Qwen3.5-35B-A3B", 400_000, 3_200_000, 0),
    // Qwen/Qwen3.5-27B（输入 <128k 档）：输入 0.6 元、输出 4.8 元
    ("siliconflow", "Qwen/Qwen3.5-27B", 600_000, 4_800_000, 0),
    // Qwen/Qwen3.5-9B（输入 <128k 档）：输入 0.5 元、输出 4 元
    ("siliconflow", "Qwen/Qwen3.5-9B", 500_000, 4_000_000, 0),
    // inclusionAI/Ling-flash-2.0：输入 1 元、输出 4 元
    (
        "siliconflow",
        "inclusionAI/Ling-flash-2.0",
        1_000_000,
        4_000_000,
        0,
    ),
    // ByteDance-Seed/Seed-OSS-36B-Instruct：输入 1.5 元、输出 4 元
    (
        "siliconflow",
        "ByteDance-Seed/Seed-OSS-36B-Instruct",
        1_500_000,
        4_000_000,
        0,
    ),
];

/// 模型名是否命中匹配模式。
///
/// `*` 通配任意字符串（可出现在任意位置，可有多个）；不含 `*` 即精确匹配。
pub fn matches_pattern(pattern: &str, model: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == model;
    }
    // 首段锚定开头
    let first = parts[0];
    if !model.starts_with(first) {
        return false;
    }
    let mut rest = &model[first.len()..];
    // 中间段按顺序出现
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(pos) => rest = &rest[pos + part.len()..],
            None => return false,
        }
    }
    // 末段锚定结尾
    rest.ends_with(parts[parts.len() - 1])
}

/// 模式的"具体程度"：通配符越少越具体，字面量越长越具体。
fn specificity(pattern: &str) -> (usize, usize) {
    let wildcards = pattern.matches('*').count();
    let literal = pattern.chars().filter(|c| *c != '*').count();
    (wildcards, literal)
}

/// `a` 是否比 `b` 更具体。
fn more_specific(a: &PriceEntry, b: &PriceEntry) -> bool {
    let (a_wild, a_len) = specificity(&a.model_pattern);
    let (b_wild, b_len) = specificity(&b.model_pattern);
    match a_wild.cmp(&b_wild) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => match a_len.cmp(&b_len) {
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => a.model_pattern < b.model_pattern,
        },
    }
}

/// 查某个 `(provider, model)` 的价目；命中多条时取**最具体**的一条。
///
/// 没命中返回 `None`——调用方据此把 `cost_micros` 记为 NULL（未知），而不是 0。
pub fn lookup(conn: &Connection, provider: &str, model: &str) -> Result<Option<PriceEntry>> {
    let mut stmt = conn.prepare(
        "SELECT provider, model_pattern, input_per_1m, output_per_1m, cache_read_per_1m
         FROM price WHERE provider = ?1",
    )?;
    let rows = stmt.query_map([provider], |row| {
        Ok(PriceEntry {
            provider: row.get(0)?,
            model_pattern: row.get(1)?,
            // NULL 单价 ≡ 0（该档不计费）；没命中价目才是 NULL
            input_per_1m: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            output_per_1m: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            cache_read_per_1m: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
        })
    })?;
    let mut best: Option<PriceEntry> = None;
    for row in rows {
        let entry = row?;
        if !matches_pattern(&entry.model_pattern, model) {
            continue;
        }
        best = Some(match best {
            None => entry,
            Some(current) => {
                if more_specific(&entry, &current) {
                    entry
                } else {
                    current
                }
            }
        });
    }
    Ok(best)
}

/// 列出全部价目（按 provider、pattern 排序）。
pub fn list(conn: &Connection) -> Result<Vec<PriceEntry>> {
    let mut stmt = conn.prepare(
        "SELECT provider, model_pattern, input_per_1m, output_per_1m, cache_read_per_1m
         FROM price ORDER BY provider, model_pattern",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PriceEntry {
            provider: row.get(0)?,
            model_pattern: row.get(1)?,
            input_per_1m: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            output_per_1m: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            cache_read_per_1m: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// 写一条价目（同 `(provider, model_pattern)` 则覆盖）。
pub fn upsert(conn: &Connection, entry: &PriceEntry) -> Result<()> {
    conn.execute(
        "INSERT INTO price (provider, model_pattern, input_per_1m, output_per_1m, cache_read_per_1m)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(provider, model_pattern) DO UPDATE SET
             input_per_1m = excluded.input_per_1m,
             output_per_1m = excluded.output_per_1m,
             cache_read_per_1m = excluded.cache_read_per_1m",
        rusqlite::params![
            entry.provider,
            entry.model_pattern,
            entry.input_per_1m,
            entry.output_per_1m,
            entry.cache_read_per_1m,
        ],
    )?;
    Ok(())
}

/// 删一条价目；返回是否确实删掉了。
pub fn delete(conn: &Connection, provider: &str, model_pattern: &str) -> Result<bool> {
    let affected = conn.execute(
        "DELETE FROM price WHERE provider = ?1 AND model_pattern = ?2",
        rusqlite::params![provider, model_pattern],
    )?;
    Ok(affected > 0)
}

/// 渠道的 provider（渠道不存在时返回空串）。
pub fn provider_of_channel(conn: &Connection, channel_id: &str) -> Result<String> {
    let provider: Option<String> = conn
        .query_row(
            "SELECT provider FROM channel WHERE id = ?1",
            [channel_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(provider.unwrap_or_default())
}

/// 按价目把 token 数换算成成本（单位 1e-6 元）。
///
/// ```text
/// 可计费输入 = prompt_tokens - cache_read_tokens
/// cost = ( 可计费输入 × input
///        + completion   × output
///        + cache_read   × cache_read
///        + cache_write  × input ) / 1_000_000
/// ```
///
/// - 缓存命中 token 有独立档价，**只按该档计一次**（它已含在 `prompt_tokens` 里）。
/// - 价目表没有独立的缓存写入档，故**缓存写入按输入价计**。
/// - 四舍五入在**总额**上做一次（不逐项舍入）。
pub fn compute_cost_micros(tokens: BillableTokens, price: &PriceEntry) -> i64 {
    let billable_input = (tokens.prompt_tokens - tokens.cache_read_tokens).max(0);
    let total: i128 = billable_input as i128 * price.input_per_1m as i128
        + tokens.completion_tokens as i128 * price.output_per_1m as i128
        + tokens.cache_read_tokens as i128 * price.cache_read_per_1m as i128
        + tokens.cache_write_tokens as i128 * price.input_per_1m as i128;
    // 总额四舍五入（加半后整除）；total 非负（token 数非负、单价非负）
    ((total + 500_000) / 1_000_000) as i64
}

/// 播种预填牌价（迁移用）。
///
/// 用 `INSERT OR IGNORE`：**不覆盖用户已改过的价**（同一主键已存在就跳过）。
pub fn seed_default_prices(conn: &Connection) -> rusqlite::Result<usize> {
    let mut inserted = 0usize;
    for (provider, pattern, input, output, cache_read) in DEFAULT_PRICES {
        inserted += conn.execute(
            "INSERT OR IGNORE INTO price
                 (provider, model_pattern, input_per_1m, output_per_1m, cache_read_per_1m)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![provider, pattern, input, output, cache_read],
        )?;
    }
    Ok(inserted)
}

/// 播种预填牌价（迁移 `up_fn` 入口；迁移框架要求返回 `()`）。
pub fn seed_default_prices_migration(conn: &Connection) -> rusqlite::Result<()> {
    seed_default_prices(conn).map(|_| ())
}

/// 重算某个 provider 的历史用量成本；返回重算的行数。
///
/// `cost_micros` 是**落库时算好的快照**，改价 / 删价后必须重算，
/// 否则历史成本会永远停留在旧牌价上。
pub fn recompute_provider_costs(store: &Store, provider: &str) -> Result<usize> {
    store.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT u.id, u.upstream_model, u.prompt_tokens, u.completion_tokens,
                    u.cache_read_tokens, u.cache_write_tokens
             FROM usage_log u JOIN channel c ON c.id = u.channel_id
             WHERE c.provider = ?1",
        )?;
        let rows = stmt.query_map([provider], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        // 先收集再更新：同一连接上边查边改会拿到过期结果集
        let mut pending: Vec<(i64, Option<i64>)> = Vec::new();
        for row in rows {
            let (id, model, prompt, completion, cache_read, cache_write) = row?;
            let cost = match lookup(conn, provider, &model)? {
                Some(price) => Some(compute_cost_micros(
                    BillableTokens {
                        prompt_tokens: prompt,
                        completion_tokens: completion,
                        cache_read_tokens: cache_read,
                        cache_write_tokens: cache_write,
                    },
                    &price,
                )),
                None => None,
            };
            pending.push((id, cost));
        }
        drop(stmt);
        let mut updated = 0usize;
        for (id, cost) in pending {
            updated += conn.execute(
                "UPDATE usage_log SET cost_micros = ?1 WHERE id = ?2",
                rusqlite::params![cost, id],
            )?;
        }
        Ok(updated)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pattern: &str, input: i64, output: i64, cache_read: i64) -> PriceEntry {
        PriceEntry {
            provider: "deepseek".to_string(),
            model_pattern: pattern.to_string(),
            input_per_1m: input,
            output_per_1m: output,
            cache_read_per_1m: cache_read,
        }
    }

    // ── 匹配语义 ──

    #[test]
    fn exact_pattern_matches_only_itself() {
        assert!(matches_pattern("deepseek-chat", "deepseek-chat"));
        assert!(!matches_pattern("deepseek-chat", "deepseek-chat-v2"));
        assert!(!matches_pattern("deepseek-chat", "xdeepseek-chat"));
    }

    #[test]
    fn star_matches_any_where() {
        assert!(matches_pattern("deepseek-*", "deepseek-chat"));
        assert!(matches_pattern("deepseek-*", "deepseek-v4-pro"));
        assert!(matches_pattern("*-pro", "deepseek-v4-pro"));
        assert!(matches_pattern("*-v4-*", "deepseek-v4-pro"));
        assert!(matches_pattern("*", "anything"));
        assert!(!matches_pattern("deepseek-*", "kimi-k3"));
        assert!(!matches_pattern("*-pro", "deepseek-chat"));
    }

    #[test]
    fn most_specific_pattern_wins() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                upsert(conn, &entry("*", 1_000_000, 1_000_000, 1_000_000))?;
                upsert(conn, &entry("deepseek-*", 2_000_000, 2_000_000, 2_000_000))?;
                upsert(conn, &entry("deepseek-chat", 3_000_000, 3_000_000, 3_000_000))?;
                Ok(())
            })
            .expect("写价目失败");

        store
            .with_conn(|conn| {
                let hit = lookup(conn, "deepseek", "deepseek-chat")
                    .expect("查价失败")
                    .expect("应命中");
                // 精确匹配最具体
                assert_eq!(hit.model_pattern, "deepseek-chat");
                assert_eq!(hit.input_per_1m, 3_000_000);

                let hit = lookup(conn, "deepseek", "deepseek-reasoner")
                    .expect("查价失败")
                    .expect("应命中");
                // 通配符少的优先于通配符多的
                assert_eq!(hit.model_pattern, "deepseek-*");
                assert_eq!(hit.input_per_1m, 2_000_000);

                let hit = lookup(conn, "deepseek", "other-model")
                    .expect("查价失败")
                    .expect("应命中");
                assert_eq!(hit.model_pattern, "*");
                assert_eq!(hit.input_per_1m, 1_000_000);
                Ok(())
            })
            .expect("断言失败");
    }

    #[test]
    fn lookup_is_scoped_to_provider() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                upsert(conn, &entry("deepseek-chat", 1_000_000, 1_000_000, 1_000_000))?;
                Ok(())
            })
            .expect("写价目失败");
        store
            .with_conn(|conn| {
                assert!(lookup(conn, "kimi", "deepseek-chat")
                    .expect("查价失败")
                    .is_none());
                Ok(())
            })
            .expect("断言失败");
    }

    // ── 三价换算（任务卡验收判据）──

    #[test]
    fn three_prices_are_charged_separately() {
        // 输入 2 元/1M、输出 8 元/1M、缓存命中 0.04 元/1M
        let price = entry("m", 2_000_000, 8_000_000, 40_000);
        let cost = compute_cost_micros(
            BillableTokens {
                prompt_tokens: 1_000_000,
                completion_tokens: 1_000_000,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            &price,
        );
        // 2 元 + 8 元 = 10 元 = 10_000_000 micros
        assert_eq!(cost, 10_000_000);
    }

    #[test]
    fn cache_read_is_not_charged_twice() {
        let price = entry("m", 2_000_000, 8_000_000, 40_000);
        // 100 万输入里 80 万命中缓存 → 可计费输入 20 万
        let cost = compute_cost_micros(
            BillableTokens {
                prompt_tokens: 1_000_000,
                completion_tokens: 0,
                cache_read_tokens: 800_000,
                cache_write_tokens: 0,
            },
            &price,
        );
        // 20 万 × 2 元/1M = 0.4 元；80 万 × 0.04 元/1M = 0.032 元 → 共 0.432 元
        assert_eq!(cost, 432_000, "缓存命中部分只按缓存价计一次");
        // 对照：若按输入价重复计，会得到 2 + 0.032 = 2.032 元
        assert!(cost < 2_032_000);
    }

    #[test]
    fn cache_write_is_charged_at_input_price() {
        let price = entry("m", 2_000_000, 8_000_000, 40_000);
        let cost = compute_cost_micros(
            BillableTokens {
                prompt_tokens: 0,
                completion_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 1_000_000,
            },
            &price,
        );
        // 价目表没有独立的缓存写档 → 按输入价 2 元/1M 计
        assert_eq!(cost, 2_000_000);
    }

    #[test]
    fn rounding_happens_once_on_the_total() {
        // 单价 1_500_000（1.5 元/1M）→ 1 个 token 的成本是 1.5 micros，须四舍五入成 2
        let price = entry("m", 1_500_000, 0, 0);
        let cost = compute_cost_micros(
            BillableTokens {
                prompt_tokens: 1,
                ..Default::default()
            },
            &price,
        );
        assert_eq!(cost, 2, "总额四舍五入（不进位截断）");

        // 逐项舍入会得到 1 + 1 = 2，这里验证总额一次舍入的结果一致
        let price = entry("m", 1_500_000, 1_500_000, 0);
        let cost = compute_cost_micros(
            BillableTokens {
                prompt_tokens: 1,
                completion_tokens: 1,
                ..Default::default()
            },
            &price,
        );
        assert_eq!(cost, 3, "总额 3 micros 一次舍入得 3");
    }

    #[test]
    fn free_model_costs_zero_and_missing_price_is_none() {
        let free = entry("free", 0, 0, 0);
        let cost = compute_cost_micros(
            BillableTokens {
                prompt_tokens: 1_000_000,
                completion_tokens: 1_000_000,
                ..Default::default()
            },
            &free,
        );
        assert_eq!(cost, 0, "免费档成本为 0");

        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                // NULL 单价 ≡ 0（该档不计费），不是「未知」
                conn.execute(
                    "INSERT INTO price (provider, model_pattern, input_per_1m, output_per_1m)
                     VALUES ('p', 'm', NULL, NULL)",
                    [],
                )?;
                let hit = lookup(conn, "p", "m").expect("查价失败").expect("应命中");
                assert_eq!(hit.input_per_1m, 0);
                assert_eq!(hit.output_per_1m, 0);
                // 没命中 → None（未知），与「免费」区分开
                assert!(lookup(conn, "p", "other").expect("查价失败").is_none());
                Ok(())
            })
            .expect("断言失败");
    }

    #[test]
    fn negative_input_is_clamped_to_zero() {
        // 缓存命中数大于输入数（口径异常）时不得算出负成本
        let price = entry("m", 2_000_000, 8_000_000, 40_000);
        let cost = compute_cost_micros(
            BillableTokens {
                prompt_tokens: 10,
                completion_tokens: 0,
                cache_read_tokens: 50,
                cache_write_tokens: 0,
            },
            &price,
        );
        // 50 个缓存命中 token × 0.04 元/1M = 2e-6 元 = 2 micros
        assert_eq!(cost, 2, "可计费输入被钳到 0，只按缓存价计");
        assert!(cost >= 0, "不得为负");
    }

    // ── CRUD / 播种 / 重算 ──

    #[test]
    fn upsert_overwrites_and_delete_reports_whether_removed() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                upsert(conn, &entry("m", 1_000_000, 1_000_000, 1_000_000))?;
                upsert(conn, &entry("m", 2_000_000, 2_000_000, 2_000_000))?;
                let hit = lookup(conn, "deepseek", "m").expect("查价失败").expect("应命中");
                assert_eq!(hit.input_per_1m, 2_000_000, "同主键应覆盖");
                assert!(delete(conn, "deepseek", "m")?);
                assert!(!delete(conn, "deepseek", "m")?, "第二次删应返回 false");
                Ok(())
            })
            .expect("断言失败");
    }

    #[test]
    fn seeding_is_idempotent_and_does_not_overwrite_user_edits() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                // 迁移 v8 开库时已播种 → 预填行已经在表里
                let hit = lookup(conn, "deepseek", "deepseek-flash")
                    .expect("查价失败")
                    .expect("应命中");
                assert_eq!(hit.input_per_1m, 2_000_000, "预填应是 2 元/1M");
                // 用户改价
                upsert(
                    conn,
                    &PriceEntry {
                        provider: "deepseek".to_string(),
                        model_pattern: "deepseek-flash".to_string(),
                        input_per_1m: 999_000_000,
                        output_per_1m: 0,
                        cache_read_per_1m: 0,
                    },
                )?;
                // 再播一次：已存在的主键被忽略
                let again = seed_default_prices(conn)?;
                assert_eq!(again, 0, "重复播种不应插入新行");
                // 用户改过的价不被覆盖
                let hit = lookup(conn, "deepseek", "deepseek-flash")
                    .expect("查价失败")
                    .expect("应命中");
                assert_eq!(hit.input_per_1m, 999_000_000, "用户改过的价不能被覆盖");
                Ok(())
            })
            .expect("断言失败");
    }

    #[test]
    fn seeding_inserts_every_default_row_into_an_empty_table() {
        let dir = tempfile::tempdir().expect("临时目录");
        let store = Store::open(dir.path().join("store.db")).expect("打开库");
        store
            .with_conn(|conn| {
                conn.execute("DELETE FROM price", [])?;
                let inserted = seed_default_prices(conn)?;
                assert_eq!(inserted, DEFAULT_PRICES.len(), "播种应插入全部预填行");
                let listed = list(conn)?;
                assert_eq!(listed.len(), DEFAULT_PRICES.len());
                Ok(())
            })
            .expect("断言失败");
    }

    #[test]
    fn default_prices_cover_all_domestic_providers() {
        let providers: std::collections::BTreeSet<&str> =
            DEFAULT_PRICES.iter().map(|(p, _, _, _, _)| *p).collect();
        for expected in ["deepseek", "kimi", "glm", "dashscope", "minimax_cn", "siliconflow"] {
            assert!(providers.contains(expected), "预填应覆盖 {expected}");
        }
        // 国际站按 USD 计价，不做汇率折算 → 不预填
        assert!(!providers.contains("minimax_intl"));
    }
}
