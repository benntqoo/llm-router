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
//! 状态**只存内存**，重启即清零（02 §2.3：不持久化，避免误伤）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// 连续失败多少次后熔断。
pub const FAILURE_THRESHOLD: u32 = 3;
/// 熔断打开后的冷却时长（秒）。
pub const OPEN_DURATION_SECS: i64 = 60;
/// 429 冷却的指数退避基数（秒）。
pub const RATE_LIMIT_BASE_SECS: i64 = 2;
/// 429 冷却上限（秒）。
pub const RATE_LIMIT_MAX_SECS: i64 = 60;

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

/// 熔断打开的原因（决定冷却期内回给客户端的语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenReason {
    /// 连续失败触发（连接错误 / 5xx）。
    Failures,
    /// 上游限流触发（429）。
    RateLimit,
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

/// 熔断器。
#[derive(Clone)]
pub struct CircuitBreaker {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
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
    /// 构造熔断器（使用真实时钟）。
    pub fn new() -> Self {
        CircuitBreaker {
            inner: Arc::new(Mutex::new(HashMap::new())),
            now: Arc::new(crate::vault::now_secs),
        }
    }

    /// 构造熔断器（注入时钟，仅测试用）。
    pub fn with_clock(now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        CircuitBreaker {
            inner: Arc::new(Mutex::new(HashMap::new())),
            now,
        }
    }

    /// 是否允许请求。
    ///
    /// - closed → 放行；
    /// - open 且未到恢复时间 → 拒绝；
    /// - open 已到期 → 转为 half_open，**放行一次**探测（探测未回结果前不再放行）。
    pub fn allow(&self, key: &str) -> bool {
        let now = (self.now)();
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
                true
            }
        }
    }

    /// 记录一次成功：回到 closed，清空计数。
    pub fn on_success(&self, key: &str) {
        let mut guard = lock(&self.inner);
        let entry = guard.entry(key.to_string()).or_default();
        entry.failures = 0;
        entry.open_until = None;
        entry.open_reason = None;
        entry.probing = false;
        entry.rate_limit_hits = 0;
    }

    /// 记录一次失败（网络错误 / 5xx）。
    ///
    /// 连续失败达到阈值 → 熔断打开 `OPEN_DURATION_SECS`。
    /// 若本次是 half_open 探测失败 → 立即重新打开。
    pub fn on_failure(&self, key: &str) {
        let now = (self.now)();
        let mut guard = lock(&self.inner);
        let entry = guard.entry(key.to_string()).or_default();
        let was_probing = entry.probing;
        entry.failures += 1;
        entry.probing = false;
        if was_probing || entry.failures >= FAILURE_THRESHOLD {
            entry.failures = entry.failures.min(FAILURE_THRESHOLD);
            entry.open_until = Some(now + OPEN_DURATION_SECS);
            entry.open_reason = Some(OpenReason::Failures);
        }
    }

    /// 记录一次 429：进入冷却。
    ///
    /// 冷却时长 = `max(指数退避, Retry-After)`。
    pub fn on_rate_limited(&self, key: &str, retry_after: Option<Duration>) {
        let now = (self.now)();
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
        let mut guard = lock(&self.inner);
        guard.remove(key);
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
