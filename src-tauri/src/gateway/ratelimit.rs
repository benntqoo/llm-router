//! 入站限流（02 §3.1）：`client_key.rpm_limit` 与 `daily_tokens`。
//!
//! 计数存内存（与熔断冷却同理，重启即清零）。
//! `daily_tokens` 按 **UTC 日**切分；token 计数由用量采集（T7）回填，
//! 在 T7 落地前该计数恒为 0，不影响 RPM 限制。

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::gateway::errors::GatewayError;

/// 限流窗口（秒）。
pub const RPM_WINDOW_SECS: i64 = 60;

/// 单个 client key 的用量计数。
#[derive(Debug, Default)]
struct ClientUsage {
    /// 最近 60 秒内的请求时间戳。
    rpm_window: VecDeque<i64>,
    /// 当日累计 token。
    daily_tokens: i64,
    /// 当日计数所属的 UTC 天起点。
    day_start: i64,
}

/// 内存限流器。
#[derive(Debug, Default)]
pub struct RateLimiter {
    clients: Mutex<HashMap<String, ClientUsage>>,
}

impl RateLimiter {
    /// 新建限流器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 检查是否超出限额。
    ///
    /// `rpm_limit` / `daily_tokens` 为 `None` 表示不限。
    pub fn check(
        &self,
        client_id: &str,
        rpm_limit: Option<i64>,
        daily_tokens_limit: Option<i64>,
        now: i64,
    ) -> Result<(), GatewayError> {
        let mut guard = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        let usage = guard.entry(client_id.to_string()).or_default();

        // 清理窗口外的记录
        let cutoff = now - RPM_WINDOW_SECS;
        while usage.rpm_window.front().is_some_and(|ts| *ts <= cutoff) {
            usage.rpm_window.pop_front();
        }

        if let Some(limit) = rpm_limit {
            if limit > 0 && usage.rpm_window.len() as i64 >= limit {
                return Err(GatewayError::rate_limited(format!(
                    "超出速率限制：{limit} 次/分钟"
                )));
            }
        }

        let day_start = utc_day_start(now);
        if usage.day_start != day_start {
            usage.day_start = day_start;
            usage.daily_tokens = 0;
        }
        if let Some(limit) = daily_tokens_limit {
            if limit > 0 && usage.daily_tokens >= limit {
                return Err(GatewayError::rate_limited(format!(
                    "超出每日 token 限制：{limit}"
                )));
            }
        }

        usage.rpm_window.push_back(now);
        Ok(())
    }

    /// 记入已消耗的 token（由 T7 用量采集回填）。
    pub fn record_tokens(&self, client_id: &str, tokens: i64, now: i64) {
        let mut guard = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        let usage = guard.entry(client_id.to_string()).or_default();
        let day_start = utc_day_start(now);
        if usage.day_start != day_start {
            usage.day_start = day_start;
            usage.daily_tokens = 0;
        }
        usage.daily_tokens += tokens;
    }

    /// 读取当日 token 计数（诊断用）。
    pub fn daily_tokens(&self, client_id: &str, now: i64) -> i64 {
        let guard = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get(client_id)
            .filter(|u| u.day_start == utc_day_start(now))
            .map(|u| u.daily_tokens)
            .unwrap_or(0)
    }
}

/// 当前 UTC 日的起点（Unix 秒）。
pub fn utc_day_start(now: i64) -> i64 {
    now.div_euclid(86_400) * 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_limit_blocks_when_exceeded() {
        let limiter = RateLimiter::new();
        assert!(limiter.check("c1", Some(2), None, 1000).is_ok());
        assert!(limiter.check("c1", Some(2), None, 1010).is_ok());
        let err = limiter
            .check("c1", Some(2), None, 1020)
            .expect_err("第三次应被限流");
        assert_eq!(err.error.kind, crate::gateway::errors::ErrorKind::RateLimitError);
    }

    #[test]
    fn rpm_window_slides() {
        let limiter = RateLimiter::new();
        assert!(limiter.check("c1", Some(1), None, 1000).is_ok());
        assert!(limiter.check("c1", Some(1), None, 1001).is_err());
        // 窗口滑过之后恢复
        assert!(limiter.check("c1", Some(1), None, 1000 + RPM_WINDOW_SECS + 1).is_ok());
    }

    #[test]
    fn daily_token_limit_blocks() {
        let limiter = RateLimiter::new();
        limiter.record_tokens("c1", 100, 1000);
        assert!(limiter.check("c1", None, Some(200), 1000).is_ok());
        limiter.record_tokens("c1", 150, 1000);
        let err = limiter
            .check("c1", None, Some(200), 1000)
            .expect_err("超出日 token 限额应被限流");
        assert_eq!(err.error.kind, crate::gateway::errors::ErrorKind::RateLimitError);
    }

    #[test]
    fn daily_tokens_reset_next_utc_day() {
        let limiter = RateLimiter::new();
        let day0 = utc_day_start(1000);
        limiter.record_tokens("c1", 500, day0);
        assert_eq!(limiter.daily_tokens("c1", day0), 500);
        let day1 = day0 + 86_400;
        assert_eq!(limiter.daily_tokens("c1", day1), 0);
        assert!(limiter.check("c1", None, Some(100), day1).is_ok());
    }

    #[test]
    fn no_limit_means_never_blocked() {
        let limiter = RateLimiter::new();
        for _ in 0..100 {
            assert!(limiter.check("c1", None, None, 1000).is_ok());
        }
    }

    #[test]
    fn limits_are_per_client() {
        let limiter = RateLimiter::new();
        assert!(limiter.check("c1", Some(1), None, 1000).is_ok());
        assert!(limiter.check("c2", Some(1), None, 1000).is_ok());
        assert!(limiter.check("c1", Some(1), None, 1001).is_err());
    }
}
