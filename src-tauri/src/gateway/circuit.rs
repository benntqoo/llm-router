//! 熔断状态机（02 §2.3）。
//!
//! 粒度：`(channel_id, upstream_model)`。
//!
//! ```text
//! closed --连续失败 3 次--> open(60s) --冷却到期--> half_open(放一次探测)
//!                                                     ├─成功→ closed
//!                                                     └─失败→ open(60s)
//! ```
//!
//! 429 走另一条冷却通道：**指数退避**（基数 2s，逐次翻倍，上限 60s），
//! 且**遵守上游的 `Retry-After`**（取两者较大值）。
//!
//! **持久化**：非 closed 的熔断状态写穿到 `circuit_state` 表（状态变化即落库），
//! **重启后恢复**——重启清零会让限流保护被绕过（重启网关就能立刻重试被 429 的上游）。
//! 冷却窗口本身很短（失败熔断 60s、429 冷却上限 60s），持久化不会造成长期误伤。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// 连续失败多少次后熔断。
pub const FAILURE_THRESHOLD: u32 = 3;
/// 熔断打开后的冷却时长（秒）——**默认值**，用户可在应用内调整（02 §2.3）。
pub const OPEN_DURATION_SECS: i64 = 60;
/// 熔断时长可调的下限（秒）——最小单位 1 秒。
pub const MIN_OPEN_DURATION_SECS: i64 = 1;
/// 429 冷却的指数退避基数（秒）。
pub const RATE_LIMIT_BASE_SECS: i64 = 2;
/// 429 冷却上限（秒）。
pub const RATE_LIMIT_MAX_SECS: i64 = 60;

/// 熔断粒度键中 `channel_id` 与 `upstream_model` 的分隔符。
pub const KEY_SEPARATOR: char = '\u{1}';

/// 拼装熔断粒度键（`(channel_id, upstream_model)`）。
pub fn circuit_key(channel_id: &str, upstream_model: &str) -> String {
    format!("{channel_id}{KEY_SEPARATOR}{upstream_model}")
}

/// 拆分熔断粒度键。
fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(KEY_SEPARATOR)
}

/// 熔断状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// 正常放行。
    Closed,
    /// 熔断中（`until` 为恢复的 Unix 秒）。
    Open { until: i64 },
    /// 冷却已过，放行一次探测。
    HalfOpen,
}

/// 熔断状态种类（落库 / UI 用；不携带截止时间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitStateKind {
    /// 正常放行（不落库）。
    Closed,
    /// 熔断中。
    Open,
    /// 半开（冷却已过，等一次探测）。
    HalfOpen,
}

impl CircuitStateKind {
    /// 落库用的字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            CircuitStateKind::Closed => "closed",
            CircuitStateKind::Open => "open",
            CircuitStateKind::HalfOpen => "half_open",
        }
    }

    /// 从落库字符串还原。
    pub fn from_str(raw: &str) -> Option<Self> {
        match raw {
            "closed" => Some(CircuitStateKind::Closed),
            "open" => Some(CircuitStateKind::Open),
            "half_open" => Some(CircuitStateKind::HalfOpen),
            _ => None,
        }
    }
}

/// 熔断打开的原因（决定冷却期内回给客户端的语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenReason {
    /// 连续失败触发（连接错误 / 5xx）。
    Failures,
    /// 上游限流触发（429）。
    RateLimit,
}

impl OpenReason {
    /// 落库用的字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            OpenReason::Failures => "failures",
            OpenReason::RateLimit => "rate_limit",
        }
    }

    /// 从落库字符串还原。
    pub fn from_str(raw: &str) -> Option<Self> {
        match raw {
            "failures" => Some(OpenReason::Failures),
            "rate_limit" => Some(OpenReason::RateLimit),
            _ => None,
        }
    }
}

/// 熔断状态快照（落库 / UI 的数据形状）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircuitSnapshot {
    /// 渠道 id。
    pub channel_id: String,
    /// 上游模型名。
    pub upstream_model: String,
    /// 状态种类。
    pub state: CircuitStateKind,
    /// 打开原因。
    pub reason: Option<OpenReason>,
    /// 连续失败次数。
    pub failures: u32,
    /// 429 命中次数（指数退避用）。
    pub rate_limit_hits: u32,
    /// 冷却截止（Unix 秒）。
    pub open_until: Option<i64>,
}

/// 熔断状态的持久化出口（写穿 + 启动恢复）。
///
/// 默认实现是"不落库"（单元测试与纯内存用法）。
pub trait CircuitPersistence: Send + Sync {
    /// 保存（写穿）。
    fn save(&self, snapshot: &CircuitSnapshot);
    /// 删除（回到 closed）。
    fn remove(&self, channel_id: &str, upstream_model: &str);
    /// 读取全部（启动恢复）。
    fn load_all(&self) -> Vec<CircuitSnapshot>;
}

/// 不落库的持久化实现（默认）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopPersistence;

impl CircuitPersistence for NoopPersistence {
    fn save(&self, _snapshot: &CircuitSnapshot) {}
    fn remove(&self, _channel_id: &str, _upstream_model: &str) {}
    fn load_all(&self) -> Vec<CircuitSnapshot> {
        Vec::new()
    }
}

/// 单个熔断器的内部状态。
#[derive(Debug, Default, Clone)]
struct Entry {
    /// 连续失败次数。
    failures: u32,
    /// 熔断/冷却截止时间（Unix 秒）。
    open_until: Option<i64>,
    /// 打开原因（`open_until` 为空时无意义）。
    open_reason: Option<OpenReason>,
    /// 是否已放行 half_open 探测（探测未回结果前不再放行）。
    probing: bool,
    /// 429 次数（指数退避用）。
    rate_limit_hits: u32,
}

impl Entry {
    /// 当前状态种类（`now` 用于判断冷却是否到期）。
    fn kind(&self, now: i64) -> CircuitStateKind {
        match self.open_until {
            None => CircuitStateKind::Closed,
            Some(until) if now < until => CircuitStateKind::Open,
            Some(_) => CircuitStateKind::HalfOpen,
        }
    }

    /// 快照（closed 不落库 → 返回 `None`）。
    fn snapshot(
        &self,
        channel_id: &str,
        upstream_model: &str,
        now: i64,
    ) -> Option<CircuitSnapshot> {
        let state = self.kind(now);
        if state == CircuitStateKind::Closed {
            return None;
        }
        Some(CircuitSnapshot {
            channel_id: channel_id.to_string(),
            upstream_model: upstream_model.to_string(),
            state,
            reason: self.open_reason,
            failures: self.failures,
            rate_limit_hits: self.rate_limit_hits,
            open_until: self.open_until,
        })
    }
}

/// 熔断器。
#[derive(Clone)]
pub struct CircuitBreaker {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    persistence: Arc<dyn CircuitPersistence>,
    /// 连续失败触发的熔断时长（秒）；运行期可改（02 §2.3）。
    open_duration_secs: Arc<AtomicI64>,
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("entries", &self.inner.lock().map(|m| m.len()).unwrap_or(0))
            .finish()
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl CircuitBreaker {
    /// 构造熔断器（使用真实时钟，不落库）。
    pub fn new() -> Self {
        CircuitBreaker {
            inner: Arc::new(Mutex::new(HashMap::new())),
            now: Arc::new(crate::vault::now_secs),
            persistence: Arc::new(NoopPersistence),
            open_duration_secs: Arc::new(AtomicI64::new(OPEN_DURATION_SECS)),
        }
    }

    /// 构造熔断器（注入时钟，仅测试用）。
    pub fn with_clock(now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        CircuitBreaker {
            inner: Arc::new(Mutex::new(HashMap::new())),
            now,
            persistence: Arc::new(NoopPersistence),
            open_duration_secs: Arc::new(AtomicI64::new(OPEN_DURATION_SECS)),
        }
    }

    /// 构造熔断器（带持久化；启动时恢复已落库的熔断状态）。
    pub fn with_persistence(persistence: Arc<dyn CircuitPersistence>) -> Self {
        let breaker = CircuitBreaker {
            inner: Arc::new(Mutex::new(HashMap::new())),
            now: Arc::new(crate::vault::now_secs),
            persistence,
            open_duration_secs: Arc::new(AtomicI64::new(OPEN_DURATION_SECS)),
        };
        breaker.restore();
        breaker
    }

    /// 构造熔断器（带持久化 + 注入时钟，测试用）。
    pub fn with_persistence_and_clock(
        persistence: Arc<dyn CircuitPersistence>,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        let breaker = CircuitBreaker {
            inner: Arc::new(Mutex::new(HashMap::new())),
            now,
            persistence,
            open_duration_secs: Arc::new(AtomicI64::new(OPEN_DURATION_SECS)),
        };
        breaker.restore();
        breaker
    }

    /// 当前熔断时长（秒）。
    pub fn open_duration_secs(&self) -> i64 {
        self.open_duration_secs.load(Ordering::SeqCst)
    }

    /// 调整熔断时长（秒）。
    ///
    /// 小于 `MIN_OPEN_DURATION_SECS` 的值会被抬到下限（最小单位 1 秒）。
    /// 只影响**之后**新打开的熔断，不追溯修改已在冷却中的熔断。
    pub fn set_open_duration_secs(&self, secs: i64) -> i64 {
        let clamped = secs.max(MIN_OPEN_DURATION_SECS);
        self.open_duration_secs.store(clamped, Ordering::SeqCst);
        clamped
    }

    /// 从持久化层恢复熔断状态（启动时调用一次）。
    fn restore(&self) {
        let snapshots = self.persistence.load_all();
        if snapshots.is_empty() {
            return;
        }
        let mut guard = lock(&self.inner);
        for snapshot in snapshots {
            let key = circuit_key(&snapshot.channel_id, &snapshot.upstream_model);
            guard.insert(
                key,
                Entry {
                    failures: snapshot.failures,
                    open_until: snapshot.open_until,
                    open_reason: snapshot.reason,
                    // 重启后没有在途探测
                    probing: false,
                    rate_limit_hits: snapshot.rate_limit_hits,
                },
            );
        }
    }

    /// 当前状态的快照（closed → `None`）。
    fn current_snapshot(&self, key: &str, now: i64) -> Option<CircuitSnapshot> {
        let (channel_id, upstream_model) = split_key(key)?;
        let guard = lock(&self.inner);
        guard
            .get(key)
            .and_then(|entry| entry.snapshot(channel_id, upstream_model, now))
    }

    /// 状态变化后写穿持久化（必须在释放内部锁之后调用）。
    ///
    /// **只在落库载荷真的变了时才写**：closed 状态下的一次成功、或未达阈值的失败，
    /// 都不产生落库行变化——不写库，也就不会每次请求都打一条 DELETE。
    fn persist_if_changed(&self, key: &str, before: Option<CircuitSnapshot>) {
        let now = (self.now)();
        let after = self.current_snapshot(key, now);
        if before == after {
            return;
        }
        match after {
            Some(snapshot) => self.persistence.save(&snapshot),
            None => {
                if let Some((channel_id, upstream_model)) = split_key(key) {
                    self.persistence.remove(channel_id, upstream_model);
                }
            }
        }
    }

    /// 当前处于非 closed 状态的熔断快照（UI 展示用）。
    pub fn active_snapshots(&self) -> Vec<CircuitSnapshot> {
        let now = (self.now)();
        let guard = lock(&self.inner);
        let mut out = Vec::new();
        for (key, entry) in guard.iter() {
            let Some((channel_id, upstream_model)) = split_key(key) else {
                continue;
            };
            if let Some(snapshot) = entry.snapshot(channel_id, upstream_model, now) {
                out.push(snapshot);
            }
        }
        out.sort_by(|a, b| {
            a.open_until
                .cmp(&b.open_until)
                .then_with(|| a.channel_id.cmp(&b.channel_id))
        });
        out
    }

    /// 是否允许请求。
    ///
    /// - closed → 放行；
    /// - open 且未到恢复时间 → 拒绝；
    /// - open 已到期 → 转为 half_open，**放行一次**探测（探测未回结果前不再放行）。
    pub fn allow(&self, key: &str) -> bool {
        let now = (self.now)();
        let before = self.current_snapshot(key, now);
        let mut guard = lock(&self.inner);
        let entry = guard.entry(key.to_string()).or_default();
        match entry.open_until {
            None => true,
            Some(until) if now < until => false,
            Some(_) => {
                // 冷却到期 → half_open
                if entry.probing {
                    return false;
                }
                entry.probing = true;
                drop(guard);
                self.persist_if_changed(key, before);
                true
            }
        }
    }

    /// 记录一次成功：回到 closed，清空计数。
    pub fn on_success(&self, key: &str) {
        let before = self.current_snapshot(key, (self.now)());
        {
            let mut guard = lock(&self.inner);
            let entry = guard.entry(key.to_string()).or_default();
            entry.failures = 0;
            entry.open_until = None;
            entry.open_reason = None;
            entry.probing = false;
            entry.rate_limit_hits = 0;
        }
        self.persist_if_changed(key, before);
    }

    /// 记录一次失败（网络错误 / 5xx）。
    ///
    /// 连续失败达到阈值 → 熔断打开 `OPEN_DURATION_SECS`。
    /// 若本次是 half_open 探测失败 → 立即重新打开。
    pub fn on_failure(&self, key: &str) {
        let now = (self.now)();
        let duration = self.open_duration_secs();
        let before = self.current_snapshot(key, now);
        {
            let mut guard = lock(&self.inner);
            let entry = guard.entry(key.to_string()).or_default();
            let was_probing = entry.probing;
            entry.failures += 1;
            entry.probing = false;
            if was_probing || entry.failures >= FAILURE_THRESHOLD {
                entry.failures = entry.failures.min(FAILURE_THRESHOLD);
                entry.open_until = Some(now + duration);
                entry.open_reason = Some(OpenReason::Failures);
            }
        }
        self.persist_if_changed(key, before);
    }

    /// 记录一次 429：进入冷却。
    ///
    /// 冷却时长 = `max(指数退避, Retry-After)`。
    pub fn on_rate_limited(&self, key: &str, retry_after: Option<Duration>) {
        let now = (self.now)();
        let before = self.current_snapshot(key, now);
        {
            let mut guard = lock(&self.inner);
            let entry = guard.entry(key.to_string()).or_default();
            entry.rate_limit_hits = entry.rate_limit_hits.saturating_add(1);
            entry.probing = false;
            let backoff = rate_limit_backoff(entry.rate_limit_hits);
            let retry_secs = retry_after.map(|d| d.as_secs() as i64).unwrap_or(0);
            let cooldown = backoff.max(retry_secs).max(1);
            entry.open_until = Some(now + cooldown);
            entry.open_reason = Some(OpenReason::RateLimit);
        }
        self.persist_if_changed(key, before);
    }

    /// 当前冷却信息：剩余秒数 + 打开原因。
    ///
    /// 不在冷却中返回 `None`（含冷却已到期、转为 half_open 的情形）。
    pub fn cooldown(&self, key: &str) -> Option<(i64, OpenReason)> {
        let now = (self.now)();
        let guard = lock(&self.inner);
        let entry = guard.get(key)?;
        let until = entry.open_until?;
        if now >= until {
            return None;
        }
        Some((
            until - now,
            entry.open_reason.unwrap_or(OpenReason::Failures),
        ))
    }

    /// 当前状态（诊断用）。
    pub fn state(&self, key: &str) -> CircuitState {
        let now = (self.now)();
        let guard = lock(&self.inner);
        let Some(entry) = guard.get(key) else {
            return CircuitState::Closed;
        };
        match entry.open_until {
            None => CircuitState::Closed,
            Some(until) if now < until => CircuitState::Open { until },
            Some(_) => CircuitState::HalfOpen,
        }
    }

    /// 熔断恢复时间（Unix 秒；未熔断返回 None）。
    pub fn open_until(&self, key: &str) -> Option<i64> {
        let guard = lock(&self.inner);
        guard.get(key).and_then(|e| e.open_until)
    }

    /// 清除某个 key 的状态（渠道被删除 / 重置时用）。
    pub fn reset(&self, key: &str) {
        {
            let mut guard = lock(&self.inner);
            guard.remove(key);
        }
        if let Some((channel_id, upstream_model)) = split_key(key) {
            self.persistence.remove(channel_id, upstream_model);
        }
    }
}

/// 429 的指数退避时长（第 n 次命中）。
pub fn rate_limit_backoff(hits: u32) -> i64 {
    if hits == 0 {
        return 0;
    }
    let shift = (hits - 1).min(6);
    (RATE_LIMIT_BASE_SECS << shift).min(RATE_LIMIT_MAX_SECS)
}

/// 取锁（忽略中毒：熔断状态丢失只是重新计数，不该让请求失败）。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// 可推进的假时钟。
    fn fake_clock() -> (Arc<AtomicI64>, CircuitBreaker) {
        let now = Arc::new(AtomicI64::new(1_000));
        let clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let breaker = CircuitBreaker::with_clock(clock);
        (now, breaker)
    }

    /// 记录写穿调用的假持久化层。
    #[derive(Default)]
    struct RecordingPersistence {
        saved: Mutex<Vec<CircuitSnapshot>>,
        removed: Mutex<Vec<(String, String)>>,
        loaded: Mutex<Vec<CircuitSnapshot>>,
    }

    impl CircuitPersistence for RecordingPersistence {
        fn save(&self, snapshot: &CircuitSnapshot) {
            lock(&self.saved).push(snapshot.clone());
        }
        fn remove(&self, channel_id: &str, upstream_model: &str) {
            lock(&self.removed).push((
                channel_id.to_string(),
                upstream_model.to_string(),
            ));
        }
        fn load_all(&self) -> Vec<CircuitSnapshot> {
            lock(&self.loaded).clone()
        }
    }

    /// 带假持久化层 + 假时钟的熔断器。
    fn persistent_breaker() -> (
        Arc<AtomicI64>,
        CircuitBreaker,
        Arc<RecordingPersistence>,
    ) {
        let now = Arc::new(AtomicI64::new(1_000));
        let clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let recorder = Arc::new(RecordingPersistence::default());
        let persistence: Arc<dyn CircuitPersistence> = recorder.clone();
        let breaker = CircuitBreaker::with_persistence_and_clock(persistence, clock);
        (now, breaker, recorder)
    }

    #[test]
    fn opening_the_circuit_is_written_through() {
        let (_now, breaker, persistence) = persistent_breaker();
        let key = circuit_key("c1", "deepseek-chat");

        breaker.on_failure(&key);
        breaker.on_failure(&key);
        assert!(
            lock(&persistence.saved).is_empty(),
            "未达阈值时状态仍是 closed，不落库"
        );

        breaker.on_failure(&key);
        let saved = lock(&persistence.saved).clone();
        assert_eq!(saved.len(), 1, "熔断打开应写穿一次");
        assert_eq!(saved[0].channel_id, "c1");
        assert_eq!(saved[0].upstream_model, "deepseek-chat");
        assert_eq!(saved[0].state, CircuitStateKind::Open);
        assert_eq!(saved[0].reason, Some(OpenReason::Failures));
        assert_eq!(saved[0].open_until, Some(1_000 + OPEN_DURATION_SECS));
    }

    #[test]
    fn success_removes_the_persisted_row() {
        let (_now, breaker, persistence) = persistent_breaker();
        let key = circuit_key("c1", "deepseek-chat");
        for _ in 0..3 {
            breaker.on_failure(&key);
        }
        assert_eq!(lock(&persistence.saved).len(), 1);

        breaker.on_success(&key);
        let removed = lock(&persistence.removed).clone();
        assert_eq!(
            removed,
            vec![("c1".to_string(), "deepseek-chat".to_string())],
            "回到 closed 应删除落库行"
        );
    }

    #[test]
    fn rate_limit_is_persisted_with_its_reason() {
        let (_now, breaker, persistence) = persistent_breaker();
        let key = circuit_key("c1", "deepseek-chat");
        breaker.on_rate_limited(&key, Some(Duration::from_secs(5)));

        let saved = lock(&persistence.saved).clone();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].state, CircuitStateKind::Open);
        assert_eq!(saved[0].reason, Some(OpenReason::RateLimit));
        assert_eq!(saved[0].open_until, Some(1_005));
        assert_eq!(saved[0].rate_limit_hits, 1);
    }

    #[test]
    fn state_is_restored_on_startup() {
        let recorder = Arc::new(RecordingPersistence::default());
        // 模拟上一次运行留下的熔断记录
        lock(&recorder.loaded).push(CircuitSnapshot {
            channel_id: "c1".to_string(),
            upstream_model: "deepseek-chat".to_string(),
            state: CircuitStateKind::Open,
            reason: Some(OpenReason::Failures),
            failures: 3,
            rate_limit_hits: 0,
            open_until: Some(2_000),
        });

        let now = Arc::new(AtomicI64::new(1_000));
        let clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let persistence: Arc<dyn CircuitPersistence> = recorder.clone();
        let breaker = CircuitBreaker::with_persistence_and_clock(persistence, clock);

        let key = circuit_key("c1", "deepseek-chat");
        assert!(
            !breaker.allow(&key),
            "重启后仍在冷却期内，必须继续拦截"
        );
        assert_eq!(
            breaker.state(&key),
            CircuitState::Open { until: 2_000 },
            "恢复后的状态应与落库一致"
        );

        // 冷却到期 → 放行一次探测
        now.store(2_000, Ordering::SeqCst);
        assert!(breaker.allow(&key), "冷却到期应放行探测");
    }

    #[test]
    fn active_snapshots_only_list_non_closed_circuits() {
        let (_now, breaker, _persistence) = persistent_breaker();
        let open_key = circuit_key("c1", "deepseek-chat");
        let ok_key = circuit_key("c2", "kimi-chat");

        breaker.on_success(&ok_key);
        for _ in 0..3 {
            breaker.on_failure(&open_key);
        }

        let active = breaker.active_snapshots();
        assert_eq!(active.len(), 1, "只应列出非 closed 的熔断");
        assert_eq!(active[0].channel_id, "c1");
        assert_eq!(active[0].state, CircuitStateKind::Open);
    }

    #[test]
    fn state_kind_and_reason_round_trip_through_strings() {
        for kind in [
            CircuitStateKind::Closed,
            CircuitStateKind::Open,
            CircuitStateKind::HalfOpen,
        ] {
            assert_eq!(CircuitStateKind::from_str(kind.as_str()), Some(kind));
        }
        for reason in [OpenReason::Failures, OpenReason::RateLimit] {
            assert_eq!(OpenReason::from_str(reason.as_str()), Some(reason));
        }
        assert_eq!(CircuitStateKind::from_str("nope"), None);
        assert_eq!(OpenReason::from_str("nope"), None);
    }

    #[test]
    fn open_duration_is_configurable() {
        let (_now, breaker, _persistence) = persistent_breaker();
        assert_eq!(breaker.open_duration_secs(), OPEN_DURATION_SECS, "默认 60s");

        breaker.set_open_duration_secs(120);
        assert_eq!(breaker.open_duration_secs(), 120);

        let key = circuit_key("c1", "deepseek-chat");
        for _ in 0..3 {
            breaker.on_failure(&key);
        }
        assert_eq!(
            breaker.state(&key),
            CircuitState::Open { until: 1_000 + 120 },
            "熔断时长应按新设置计算"
        );
    }

    #[test]
    fn open_duration_is_clamped_to_one_second() {
        let (_now, breaker, _persistence) = persistent_breaker();
        assert_eq!(breaker.set_open_duration_secs(0), MIN_OPEN_DURATION_SECS);
        assert_eq!(breaker.set_open_duration_secs(-30), MIN_OPEN_DURATION_SECS);
        assert_eq!(breaker.open_duration_secs(), MIN_OPEN_DURATION_SECS);
    }

    #[test]
    fn changing_duration_does_not_retroactively_change_open_circuits() {
        let (_now, breaker, _persistence) = persistent_breaker();
        let key = circuit_key("c1", "deepseek-chat");
        for _ in 0..3 {
            breaker.on_failure(&key);
        }
        assert_eq!(
            breaker.state(&key),
            CircuitState::Open { until: 1_000 + OPEN_DURATION_SECS }
        );

        breaker.set_open_duration_secs(300);
        assert_eq!(
            breaker.state(&key),
            CircuitState::Open { until: 1_000 + OPEN_DURATION_SECS },
            "已在冷却中的熔断不被新设置追溯修改"
        );
    }

    #[test]
    fn three_consecutive_failures_open_the_circuit() {
        let (_now, breaker) = fake_clock();
        assert!(breaker.allow("k1"));
        breaker.on_failure("k1");
        breaker.on_failure("k1");
        assert_eq!(breaker.state("k1"), CircuitState::Closed, "连败 2 次不熔断");
        breaker.on_failure("k1");
        assert_eq!(
            breaker.state("k1"),
            CircuitState::Open { until: 1_000 + OPEN_DURATION_SECS }
        );
        assert!(!breaker.allow("k1"), "熔断期间必须拒绝");
    }

    #[test]
    fn open_circuit_recovers_via_half_open_probe() {
        let (now, breaker) = fake_clock();
        for _ in 0..3 {
            breaker.on_failure("k1");
        }
        // 冷却未到
        now.store(1_000 + OPEN_DURATION_SECS - 1, Ordering::SeqCst);
        assert!(!breaker.allow("k1"));

        // 冷却到期 → 放行一次探测
        now.store(1_000 + OPEN_DURATION_SECS, Ordering::SeqCst);
        assert!(breaker.allow("k1"), "到期后应放行一次探测");
        assert!(
            !breaker.allow("k1"),
            "探测未回结果前不得再放行（防止并发穿透）"
        );

        // 探测成功 → closed
        breaker.on_success("k1");
        assert_eq!(breaker.state("k1"), CircuitState::Closed);
        assert!(breaker.allow("k1"));
    }

    #[test]
    fn failed_probe_reopens_the_circuit() {
        let (now, breaker) = fake_clock();
        for _ in 0..3 {
            breaker.on_failure("k1");
        }
        now.store(1_000 + OPEN_DURATION_SECS, Ordering::SeqCst);
        assert!(breaker.allow("k1"));
        breaker.on_failure("k1");
        assert_eq!(
            breaker.state("k1"),
            CircuitState::Open {
                until: 1_000 + OPEN_DURATION_SECS + OPEN_DURATION_SECS
            },
            "探测失败应重新打开，且冷却重新计时"
        );
    }

    #[test]
    fn success_resets_failure_counter() {
        let (_now, breaker) = fake_clock();
        breaker.on_failure("k1");
        breaker.on_failure("k1");
        breaker.on_success("k1");
        breaker.on_failure("k1");
        assert_eq!(breaker.state("k1"), CircuitState::Closed);
    }

    #[test]
    fn rate_limit_uses_exponential_backoff() {
        assert_eq!(rate_limit_backoff(1), 2);
        assert_eq!(rate_limit_backoff(2), 4);
        assert_eq!(rate_limit_backoff(3), 8);
        assert_eq!(rate_limit_backoff(4), 16);
        assert_eq!(rate_limit_backoff(5), 32);
        assert_eq!(rate_limit_backoff(6), 60);
        assert_eq!(rate_limit_backoff(10), 60, "封顶 60s");
    }

    #[test]
    fn rate_limit_cooldown_respects_retry_after() {
        let (_now, breaker) = fake_clock();
        // Retry-After=30 大于指数退避的 2s → 取 30
        breaker.on_rate_limited("k1", Some(Duration::from_secs(30)));
        assert_eq!(
            breaker.state("k1"),
            CircuitState::Open {
                until: 1_000 + 30
            }
        );
        assert!(!breaker.allow("k1"));
    }

    #[test]
    fn rate_limit_cooldown_takes_larger_of_backoff_and_retry_after() {
        let (_now, breaker) = fake_clock();
        // 第二次命中：指数退避 4s，但 Retry-After=1 → 取 4s
        breaker.on_rate_limited("k1", Some(Duration::from_secs(1)));
        breaker.on_rate_limited("k1", Some(Duration::from_secs(1)));
        assert_eq!(breaker.state("k1"), CircuitState::Open { until: 1_000 + 4 });
    }

    #[test]
    fn keys_are_independent() {
        let (_now, breaker) = fake_clock();
        for _ in 0..3 {
            breaker.on_failure("k1");
        }
        assert!(!breaker.allow("k1"));
        assert!(breaker.allow("k2"), "不同渠道互不影响");
    }

    #[test]
    fn reset_clears_state() {
        let (_now, breaker) = fake_clock();
        for _ in 0..3 {
            breaker.on_failure("k1");
        }
        breaker.reset("k1");
        assert_eq!(breaker.state("k1"), CircuitState::Closed);
        assert!(breaker.allow("k1"));
    }

    #[test]
    fn cooldown_reports_reason_and_remaining_seconds() {
        let (_now, breaker) = fake_clock();
        // 未熔断
        assert_eq!(breaker.cooldown("k1"), None);

        // 连续失败打开 → 原因为 Failures
        for _ in 0..3 {
            breaker.on_failure("k1");
        }
        let (remaining, reason) = breaker.cooldown("k1").expect("应在冷却中");
        assert_eq!(reason, OpenReason::Failures);
        assert_eq!(remaining, OPEN_DURATION_SECS);

        // 429 打开 → 原因为 RateLimit
        breaker.on_rate_limited("k2", Some(Duration::from_secs(7)));
        let (remaining, reason) = breaker.cooldown("k2").expect("应在冷却中");
        assert_eq!(reason, OpenReason::RateLimit);
        assert_eq!(remaining, 7, "剩余秒数应等于冷却时长");
    }

    #[test]
    fn cooldown_expires_after_deadline() {
        let (now, breaker) = fake_clock();
        breaker.on_rate_limited("k1", Some(Duration::from_secs(5)));
        assert!(breaker.cooldown("k1").is_some());
        now.store(1_000 + 5, Ordering::SeqCst);
        assert_eq!(breaker.cooldown("k1"), None, "冷却到期后不再算冷却中");
    }
}
