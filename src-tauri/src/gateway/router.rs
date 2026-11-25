//! 路由与别名解析（02 §2.1 ③）。
//!
//! 三条路径（02 §2.1 ③ 原文）：
//! 1. 命中 `route_group.alias` → 按 strategy 选出 model；
//! 2. 命中 `model.alias` → 直接使用；
//! 3. 未命中 → 400，响应体附可用别名列表。
//!
//! 路由输出 `(channel, upstream_model, credential_id)`，并给出 fallback 候选链
//! （02 §2.1 ④：首选 + 组内按 priority 排序的备选）。候选链的消费（熔断跳过、
//! 逐个重试）属 T6。
//!
//! **策略内存状态只存内存**（round_robin 轮询游标、sticky_session 粘性表）：
//! 与熔断状态（02 §2.3）同一口径——重启即清零，不做持久化，避免误伤。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::store::Store;

/// 路由策略（02 §2.1 ③）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// 按渠道优先级（`channel.priority` 越小越优先）。
    Priority,
    /// 平滑加权轮询（权重取 `route_member.weight`）。
    RoundRobin,
    /// 按价目表估算成本，低者优先。
    Cost,
    /// 同一 client_key + 别名粘住同一渠道。
    StickySession,
}

impl Strategy {
    /// 解析 `route_group.strategy` 文本。
    pub fn parse(raw: &str) -> Option<Strategy> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "priority" => Some(Strategy::Priority),
            "round_robin" => Some(Strategy::RoundRobin),
            "cost" => Some(Strategy::Cost),
            "sticky_session" => Some(Strategy::StickySession),
            _ => None,
        }
    }

    /// 文本形式。
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Priority => "priority",
            Strategy::RoundRobin => "round_robin",
            Strategy::Cost => "cost",
            Strategy::StickySession => "sticky_session",
        }
    }
}

/// 一个可出站候选（模型 + 渠道 + 凭证引用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteCandidate {
    /// `model.id`
    pub model_id: String,
    /// `channel.id`
    pub channel_id: String,
    /// `channel.name`（错误体与 `X-Routed-Via` 用）
    pub channel_name: String,
    /// `channel.provider`
    pub provider: String,
    /// 上游真实模型名（R-1 用）
    pub upstream_model: String,
    /// `channel.priority`（越小越优先）
    pub channel_priority: i64,
    /// `route_member.weight`（轮询权重）
    pub weight: i64,
    /// 成本排序键（1e-6 元；`None` = 无价目，按最贵处理）
    pub cost_key: Option<i64>,
    /// 该渠道下的首选凭证 id（**只有 id，不含明文**）
    pub credential_id: Option<String>,
}

/// 路由方案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePlan {
    /// 请求里的别名
    pub alias: String,
    /// 实际使用的策略
    pub strategy: Strategy,
    /// 首选候选
    pub primary: RouteCandidate,
    /// fallback 候选链（首选在首位，其余按 priority 排序）
    pub candidates: Vec<RouteCandidate>,
}

impl RoutePlan {
    /// 首选候选的渠道 id。
    pub fn channel_id(&self) -> &str {
        &self.primary.channel_id
    }

    /// 首选候选的上游模型名。
    pub fn upstream_model(&self) -> &str {
        &self.primary.upstream_model
    }
}

/// 路由错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// 别名未命中（附可用别名列表）。
    AliasNotFound {
        /// 请求的别名
        alias: String,
        /// 当前可用的别名
        available: Vec<String>,
    },
    /// 别名命中但没有可用候选。
    NoCandidate {
        /// 请求的别名
        alias: String,
        /// 原因
        reason: String,
    },
    /// 内部错误（数据库等）。
    Internal(String),
}

/// 路由器（别名 → 路由目标）。
#[derive(Debug)]
pub struct AliasRouter {
    store: Arc<Store>,
    /// sticky_session 粘性表：`client_key_id \u{1} alias` → `model_id`
    sticky: Mutex<HashMap<String, String>>,
    /// round_robin 游标：别名 → (model_id → 当前权重)
    round_robin: Mutex<HashMap<String, HashMap<String, i64>>>,
}

impl AliasRouter {
    /// 构造路由器。
    pub fn new(store: Arc<Store>) -> Self {
        AliasRouter {
            store,
            sticky: Mutex::new(HashMap::new()),
            round_robin: Mutex::new(HashMap::new()),
        }
    }

    /// 解析别名 → 路由方案。
    ///
    /// 数据库访问在 `spawn_blocking` 里做（避免阻塞 tokio 工作线程），
    /// 策略选择是纯内存操作。
    pub async fn resolve(
        &self,
        client_key_id: &str,
        alias: &str,
    ) -> Result<RoutePlan, RouteError> {
        let store = Arc::clone(&self.store);
        let alias_for_db = alias.to_string();
        let loaded = tokio::task::spawn_blocking(move || load_route(&store, &alias_for_db))
            .await
            .map_err(|e| RouteError::Internal(format!("路由查询任务异常: {e}")))?
            .map_err(RouteError::Internal)?;

        let Some(loaded) = loaded else {
            return Err(RouteError::AliasNotFound {
                alias: alias.to_string(),
                available: self.available_aliases().await?,
            });
        };

        self.select(client_key_id, alias, loaded)
    }

    /// 当前可用别名列表（`model.alias` ∪ `route_group.alias`）。
    pub async fn available_aliases(&self) -> Result<Vec<String>, RouteError> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || list_available_aliases(&store))
            .await
            .map_err(|e| RouteError::Internal(format!("别名查询任务异常: {e}")))?
            .map_err(RouteError::Internal)
    }

    /// 按策略选出首选候选并组装候选链。
    fn select(
        &self,
        client_key_id: &str,
        alias: &str,
        loaded: LoadedRoute,
    ) -> Result<RoutePlan, RouteError> {
        if loaded.candidates.is_empty() {
            return Err(RouteError::NoCandidate {
                alias: alias.to_string(),
                reason: "该别名的候选全部被禁用或组内为空".to_string(),
            });
        }

        // fallback 链的统一顺序：priority 升序 → weight 降序 → model_id 升序
        let mut ordered = loaded.candidates;
        sort_by_priority(&mut ordered);

        let primary_index = match loaded.strategy {
            Strategy::Priority => 0,
            Strategy::RoundRobin => self.pick_round_robin(alias, &ordered),
            Strategy::Cost => pick_by_cost(&ordered),
            Strategy::StickySession => self.pick_sticky(client_key_id, alias, &ordered),
        };

        let primary = ordered[primary_index].clone();
        let mut candidates = Vec::with_capacity(ordered.len());
        candidates.push(primary.clone());
        candidates.extend(
            ordered
                .into_iter()
                .filter(|c| c.model_id != primary.model_id),
        );

        Ok(RoutePlan {
            alias: alias.to_string(),
            strategy: loaded.strategy,
            primary,
            candidates,
        })
    }

    /// 平滑加权轮询（SWRR）。
    fn pick_round_robin(&self, alias: &str, ordered: &[RouteCandidate]) -> usize {
        let total: i64 = ordered.iter().map(|c| c.weight.max(0)).sum();
        if total <= 0 {
            return 0;
        }
        let mut guard = lock(&self.round_robin);
        let state = guard.entry(alias.to_string()).or_default();

        let mut best_index = 0usize;
        let mut best_current = i64::MIN;
        for (index, candidate) in ordered.iter().enumerate() {
            let weight = candidate.weight.max(0);
            let current = state.entry(candidate.model_id.clone()).or_insert(0);
            *current += weight;
            if *current > best_current {
                best_current = *current;
                best_index = index;
            }
        }
        // 选中者减去总权重（本轮不再被选中）
        let total_weight: i64 = ordered.iter().map(|c| c.weight.max(0)).sum();
        if let Some(current) = state.get_mut(&ordered[best_index].model_id) {
            *current -= total_weight;
        }
        best_index
    }

    /// sticky_session：同一 client_key + 别名粘住同一渠道。
    fn pick_sticky(
        &self,
        client_key_id: &str,
        alias: &str,
        ordered: &[RouteCandidate],
    ) -> usize {
        let key = format!("{client_key_id}\u{1}{alias}");
        {
            let guard = lock(&self.sticky);
            if let Some(model_id) = guard.get(&key) {
                if let Some(index) = ordered.iter().position(|c| &c.model_id == model_id) {
                    return index;
                }
            }
        }
        // 首次或原候选已失效 → 按 priority 选首个并记住
        let index = 0usize;
        let mut guard = lock(&self.sticky);
        guard.insert(key, ordered[index].model_id.clone());
        index
    }
}

/// 从数据库加载别名对应的候选。
fn load_route(store: &Store, alias: &str) -> Result<Option<LoadedRoute>, String> {
    store
        .with_conn(|conn| {
            let group: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, strategy FROM route_group WHERE alias = ?1",
                    [alias],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let is_group = group.is_some();

            let (strategy_raw, candidates) = match group {
                Some((group_id, strategy_raw)) => {
                    let candidates = load_group_candidates(conn, &group_id)?;
                    (strategy_raw, candidates)
                }
                None => {
                    let candidates = load_model_candidates(conn, alias)?;
                    ("priority".to_string(), candidates)
                }
            };

            if candidates.is_empty() {
                // 组命中但无候选 → 交给上层区分「未命中」与「无候选」
                if is_group {
                    return Ok(Some(LoadedRoute {
                        strategy: Strategy::parse(&strategy_raw).unwrap_or(Strategy::Priority),
                        candidates,
                    }));
                }
                return Ok(None);
            }

            let prices = load_prices(conn)?;
            let mut candidates = candidates;
            attach_cost(&mut candidates, &prices);
            for candidate in &mut candidates {
                candidate.credential_id = load_channel_credential(conn, &candidate.channel_id)?;
            }

            Ok(Some(LoadedRoute {
                strategy: Strategy::parse(&strategy_raw).unwrap_or(Strategy::Priority),
                candidates,
            }))
        })
        .map_err(|e| format!("路由查询失败: {e}"))
}

/// 数据库加载结果。
struct LoadedRoute {
    strategy: Strategy,
    candidates: Vec<RouteCandidate>,
}

/// 组候选（route_member → model → channel）。
fn load_group_candidates(
    conn: &Connection,
    group_id: &str,
) -> rusqlite::Result<Vec<RouteCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.channel_id, m.upstream_id, c.name, c.provider, c.priority, rm.weight
         FROM route_member rm
         JOIN model m ON m.id = rm.model_id
         JOIN channel c ON c.id = m.channel_id
         WHERE rm.group_id = ?1 AND m.enabled = 1 AND c.enabled = 1
         ORDER BY c.priority ASC, rm.weight DESC, m.id ASC",
    )?;
    let rows = stmt.query_map([group_id], |row| {
        Ok(RouteCandidate {
            model_id: row.get(0)?,
            channel_id: row.get(1)?,
            upstream_model: row.get(2)?,
            channel_name: row.get(3)?,
            provider: row.get(4)?,
            channel_priority: row.get(5)?,
            weight: row.get(6)?,
            cost_key: None,
            credential_id: None,
        })
    })?;
    rows.collect()
}

/// 单别名候选（model.alias）。
fn load_model_candidates(
    conn: &Connection,
    alias: &str,
) -> rusqlite::Result<Vec<RouteCandidate>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.channel_id, m.upstream_id, c.name, c.provider, c.priority, 100
         FROM model m
         JOIN channel c ON c.id = m.channel_id
         WHERE m.alias = ?1 AND m.enabled = 1 AND c.enabled = 1",
    )?;
    let rows = stmt.query_map([alias], |row| {
        Ok(RouteCandidate {
            model_id: row.get(0)?,
            channel_id: row.get(1)?,
            upstream_model: row.get(2)?,
            channel_name: row.get(3)?,
            provider: row.get(4)?,
            channel_priority: row.get(5)?,
            weight: row.get(6)?,
            cost_key: None,
            credential_id: None,
        })
    })?;
    rows.collect()
}

/// 价目表行。
struct PriceRow {
    provider: String,
    model_pattern: String,
    input_per_1m: Option<i64>,
    output_per_1m: Option<i64>,
}

/// 读取价目表（表小，一次读完在内存里匹配）。
fn load_prices(conn: &Connection) -> rusqlite::Result<Vec<PriceRow>> {
    let mut stmt = conn.prepare(
        "SELECT provider, model_pattern, input_per_1m, output_per_1m FROM price",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PriceRow {
            provider: row.get(0)?,
            model_pattern: row.get(1)?,
            input_per_1m: row.get(2)?,
            output_per_1m: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// 给候选附上成本排序键。
///
/// 成本键 = `input_per_1m + output_per_1m`（单位 1e-6 元，假设输入输出各 1M token）。
/// 没有匹配价目 → `None`，按最贵处理。
fn attach_cost(candidates: &mut [RouteCandidate], prices: &[PriceRow]) {
    for candidate in candidates.iter_mut() {
        candidate.cost_key = cost_key_for(&candidate.provider, &candidate.upstream_model, prices);
    }
}

/// 查某渠道下的首选凭证 id（**只取 id，不解密**）。
fn load_channel_credential(
    conn: &Connection,
    channel_id: &str,
) -> rusqlite::Result<Option<String>> {
    let now = crate::vault::now_secs();
    conn.query_row(
        "SELECT id FROM credential
         WHERE channel_id = ?1 AND (expires_at IS NULL OR expires_at > ?2)
         ORDER BY CASE kind
             WHEN 'api_key' THEN 0
             WHEN 'oauth_copilot_token' THEN 1
             WHEN 'oauth_gh_token' THEN 2
             WHEN 'session_workbuddy' THEN 3
             ELSE 9 END,
             created_at ASC
         LIMIT 1",
        rusqlite::params![channel_id, now],
        |row| row.get(0),
    )
    .optional()
}

/// 可用别名列表。
fn list_available_aliases(store: &Store) -> Result<Vec<String>, String> {
    store
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT alias FROM model WHERE enabled = 1
                 UNION
                 SELECT alias FROM route_group
                 ORDER BY alias",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .map_err(|e| format!("别名查询失败: {e}"))
}

/// priority 排序：渠道优先级升序 → 权重降序 → model_id 升序。
fn sort_by_priority(candidates: &mut [RouteCandidate]) {
    candidates.sort_by(|a, b| {
        a.channel_priority
            .cmp(&b.channel_priority)
            .then_with(|| b.weight.cmp(&a.weight))
            .then_with(|| a.model_id.cmp(&b.model_id))
    });
}

/// cost 排序：成本键升序（无价目排最后）→ 渠道优先级升序 → model_id 升序。
fn pick_by_cost(ordered: &[RouteCandidate]) -> usize {
    let mut best: Option<usize> = None;
    for (index, candidate) in ordered.iter().enumerate() {
        match (best, candidate.cost_key) {
            (None, _) => best = Some(index),
            (Some(current), Some(key)) => {
                let current_key = ordered[current].cost_key;
                let better = match current_key {
                    Some(current_key) => key < current_key,
                    None => true,
                };
                if better {
                    best = Some(index);
                }
            }
            (Some(_), None) => {}
        }
    }
    best.unwrap_or(0)
}

/// 某 provider + 模型的成本键。
fn cost_key_for(provider: &str, model: &str, prices: &[PriceRow]) -> Option<i64> {
    let mut best: Option<(usize, i64)> = None;
    for row in prices {
        if row.provider != provider {
            continue;
        }
        if !glob_match(&row.model_pattern, model) {
            continue;
        }
        let literal = row.model_pattern.chars().filter(|c| *c != '*').count();
        let key = row.input_per_1m.unwrap_or(0) + row.output_per_1m.unwrap_or(0);
        match best {
            Some((best_literal, best_key)) => {
                if literal > best_literal || (literal == best_literal && key < best_key) {
                    best = Some((literal, key));
                }
            }
            None => best = Some((literal, key)),
        }
    }
    best.map(|(_, key)| key)
}

/// 通配匹配（`*` 匹配任意长度，字节级安全）。
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(star_index) = star {
            pi = star_index + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// 取锁（忽略中毒，路由状态丢了只是重新分配，不该让请求失败）。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::router::Strategy::*;

    fn candidate(
        model_id: &str,
        channel_id: &str,
        priority: i64,
        weight: i64,
        cost: Option<i64>,
    ) -> RouteCandidate {
        RouteCandidate {
            model_id: model_id.to_string(),
            channel_id: channel_id.to_string(),
            channel_name: format!("渠道 {channel_id}"),
            provider: "deepseek".to_string(),
            upstream_model: format!("{model_id}-upstream"),
            channel_priority: priority,
            weight,
            cost_key: cost,
            credential_id: None,
        }
    }

    #[test]
    fn strategy_parse_covers_four_values() {
        assert_eq!(Strategy::parse("priority"), Some(Priority));
        assert_eq!(Strategy::parse("round_robin"), Some(RoundRobin));
        assert_eq!(Strategy::parse("cost"), Some(Cost));
        assert_eq!(Strategy::parse("sticky_session"), Some(StickySession));
        assert_eq!(Strategy::parse("sticky-session"), None);
        assert_eq!(Strategy::parse("unknown"), None);
    }

    #[test]
    fn priority_sort_uses_channel_priority_then_weight() {
        let mut candidates = vec![
            candidate("m2", "c2", 100, 100, None),
            candidate("m1", "c1", 50, 10, None),
            candidate("m3", "c3", 100, 300, None),
        ];
        sort_by_priority(&mut candidates);
        assert_eq!(
            candidates.iter().map(|c| c.model_id.as_str()).collect::<Vec<_>>(),
            vec!["m1", "m3", "m2"]
        );
    }

    #[test]
    fn cost_pick_prefers_cheapest_and_treats_missing_price_as_most_expensive() {
        let candidates = vec![
            candidate("m1", "c1", 10, 100, None),
            candidate("m2", "c2", 20, 100, Some(500)),
            candidate("m3", "c3", 30, 100, Some(100)),
        ];
        assert_eq!(pick_by_cost(&candidates), 2, "最便宜的是 m3");
    }

    #[test]
    fn cost_pick_falls_back_to_first_when_no_price_data() {
        let candidates = vec![
            candidate("m1", "c1", 10, 100, None),
            candidate("m2", "c2", 20, 100, None),
        ];
        assert_eq!(pick_by_cost(&candidates), 0);
    }

    #[test]
    fn glob_match_supports_star_wildcards() {
        assert!(glob_match("deepseek-chat", "deepseek-chat"));
        assert!(!glob_match("deepseek-chat", "deepseek-reasoner"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("gpt-4*", "gpt-4o-mini"));
        assert!(!glob_match("gpt-4*", "o1-mini"));
        assert!(glob_match("*-mini", "gpt-4o-mini"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("a*b*c", "axxbyy"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
    }

    #[test]
    fn cost_key_prefers_more_specific_pattern() {
        let prices = vec![
            PriceRow {
                provider: "deepseek".to_string(),
                model_pattern: "deepseek-*".to_string(),
                input_per_1m: Some(1),
                output_per_1m: Some(1),
            },
            PriceRow {
                provider: "deepseek".to_string(),
                model_pattern: "deepseek-chat".to_string(),
                input_per_1m: Some(10),
                output_per_1m: Some(10),
            },
        ];
        assert_eq!(cost_key_for("deepseek", "deepseek-chat", &prices), Some(20));
        assert_eq!(cost_key_for("deepseek", "deepseek-reasoner", &prices), Some(2));
        assert_eq!(cost_key_for("kimi", "deepseek-chat", &prices), None);
    }
}
