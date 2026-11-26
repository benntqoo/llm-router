//! cron 表达式与"应跑"判定（T17；02 §7.2.1 §1–§2）。
//!
//! - 解析用 `cron` crate（02 §7.2 指定），**按本地时区**解释（`chrono::Local`）；
//! - `last_due_at` = **最近一次应触发时刻（≤ now）**——调度与补签都用它；
//! - `CronShape` 只用于**系统兜底任务的频率映射**（Windows 计划任务 / macOS launchd），
//!   按**原始表达式字段**分类，避免为了映射再实现一遍匹配语义。

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Local, Timelike};
use cron::Schedule;

/// 解析 cron 表达式。
///
/// **字段口径**：用户写**经典 5 字段**（分 时 日 月 周）；`cron` crate 0.12 用的是
/// **6 字段**（秒 分 时 日 月 周 [年]）——这里做一层归一化：
/// 5 字段 → 前面补一个 `0`（秒）；6 / 7 字段原样交给 crate 解析。
pub fn parse(expr: &str) -> Result<Schedule, String> {
    let normalized = normalize(expr)?;
    Schedule::from_str(&normalized).map_err(|err| format!("cron 表达式不合法：{err}"))
}

/// 归一化：5 字段补秒位；字段数不对直接报错。
fn normalize(expr: &str) -> Result<String, String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    match fields.len() {
        5 => Ok(format!("0 {}", fields.join(" "))),
        6 | 7 => Ok(fields.join(" ")),
        other => Err(format!(
            "cron 表达式不合法：必须是 5~7 个字段（分 时 日 月 周），实际 {other} 个"
        )),
    }
}

/// 最近一次应触发时刻（≤ `now`）。
///
/// 算法：从 `now` 往回扫窗口，窗口内**没有**命中点就把窗口放大（1 小时 → 1 天 → 7 天 → …），
/// 找到命中点后取**最后一个 ≤ now** 的命中（迭代器有序，撞到 > now 就停）。
///
/// 这样既不依赖 `cron` crate 的"上一次"语义（它只给未来的），也不用逐分钟暴力回溯。
pub fn last_due_at(schedule: &Schedule, now: DateTime<Local>) -> Option<DateTime<Local>> {
    const WINDOWS_MINUTES: [i64; 6] = [
        60,        // 1 小时
        60 * 24,   // 1 天
        60 * 7 * 24,
        60 * 31 * 24,
        60 * 366 * 24,
        60 * 366 * 24 * 10,
    ];
    for minutes in WINDOWS_MINUTES {
        let start = now - Duration::from_secs((minutes * 60) as u64);
        let mut last: Option<DateTime<Local>> = None;
        for occurrence in schedule.after(&start) {
            if occurrence > now {
                break;
            }
            last = Some(occurrence);
        }
        if last.is_some() {
            return last;
        }
    }
    None
}

/// 下一次应触发时刻（> `now`）。
pub fn next_due_at(schedule: &Schedule, now: DateTime<Local>) -> Option<DateTime<Local>> {
    schedule.after(&now).next()
}

/// 当前分钟是否命中（用于"到点就跑"的精确判断）。
pub fn matches_now(schedule: &Schedule, now: DateTime<Local>) -> bool {
    let truncated = now
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0));
    match truncated {
        Some(minute) => schedule.includes(minute),
        None => false,
    }
}

// ────────────────────────────── 系统兜底任务的频率映射 ──────────────────────────────

/// cron 表达式的形状（**只用于**把频率映射到 Windows 计划任务 / macOS launchd）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronShape {
    /// 每 N 分钟一次（`*/N * * * *`）。
    EveryMinutes(u32),
    /// 每小时第 M 分钟（`M * * * *`）。
    Hourly(u32),
    /// 每天 H:M（`M H * * *`）。
    Daily { hour: u32, minute: u32 },
    /// 每周的 H:M（`M H * * D`）。
    Weekly {
        hour: u32,
        minute: u32,
        /// 星期几（1 = 周一 … 7 = 周日）。
        days: Vec<u32>,
    },
    /// 每月的 H:M（`M H D * *`）。
    Monthly { hour: u32, minute: u32, day: u32 },
    /// 其它（映射成"每天一次"——宁可多跑，不可漏跑）。
    Other,
}

/// 按**原始表达式字段**分类（归一化成 6 字段：秒 分 时 日 月 周）。
pub fn classify(expr: &str) -> CronShape {
    let normalized = match normalize(expr) {
        Ok(normalized) => normalized,
        Err(_) => return CronShape::Other,
    };
    let fields: Vec<&str> = normalized.split_whitespace().collect();
    if fields.len() < 6 {
        return CronShape::Other;
    }
    // [秒, 分, 时, 日, 月, 周]
    let (minute, hour, dom, month, dow) = (fields[1], fields[2], fields[3], fields[4], fields[5]);

    let minute_value = parse_single_number(minute);
    let hour_value = parse_single_number(hour);

    // 每 N 分钟：分钟字段是 */N
    if let Some(step) = parse_step(minute) {
        if hour == "*" && dom == "*" && month == "*" && dow == "*" {
            return CronShape::EveryMinutes(step);
        }
        return CronShape::Other;
    }
    // 每分钟：分钟字段是 `*`（`*/1` 的等价写法）
    // —— **不能**落到 `Other`（那是"每天一次"）：会把"每分钟"降级成"每天"，漏跑。
    if minute == "*" && hour == "*" && dom == "*" && month == "*" && dow == "*" {
        return CronShape::EveryMinutes(1);
    }
    let minute_no = match minute_value {
        Some(value) => value,
        None => return CronShape::Other,
    };
    // 每小时第 M 分：`* * * * *` 形态（小时字段是 `*`）
    if hour == "*" && dom == "*" && month == "*" && dow == "*" {
        return CronShape::Hourly(minute_no);
    }
    let hour_no = match hour_value {
        Some(value) => value,
        None => return CronShape::Other,
    };
    if dom == "*" && month == "*" && dow == "*" {
        return CronShape::Daily {
            hour: hour_no,
            minute: minute_no,
        };
    }
    if dom == "*" && month == "*" && dow != "*" {
        let days = parse_weekdays(dow);
        if !days.is_empty() {
            return CronShape::Weekly {
                hour: hour_no,
                minute: minute_no,
                days,
            };
        }
        return CronShape::Other;
    }
    if dom != "*" && month == "*" && dow == "*" {
        if let Some(day) = parse_single_number(dom) {
            return CronShape::Monthly {
                hour: hour_no,
                minute: minute_no,
                day,
            };
        }
        return CronShape::Other;
    }
    CronShape::Other
}

/// 单个数字字段（`5` → 5）。
fn parse_single_number(field: &str) -> Option<u32> {
    field.parse::<u32>().ok()
}

/// 步长字段（`*/15` → 15；`5/15` 这种写法不认）。
fn parse_step(field: &str) -> Option<u32> {
    let rest = field.strip_prefix("*/")?;
    rest.parse::<u32>().ok().filter(|value| *value > 0)
}

/// 星期字段（`1-5` / `1,3,5` / `0`（周日））→ 统一成 1=周一 … 7=周日。
fn parse_weekdays(field: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in field.split(',') {
        let part = part.trim();
        let value = if let Some((from, to)) = part.split_once('-') {
            match (from.parse::<u32>(), to.parse::<u32>()) {
                (Ok(from), Ok(to)) if from <= to => Some((from, to)),
                (Ok(from), Ok(to)) if from > to => {
                    // 跨周末的区间（如 5-1 = 周五到周一）
                    let mut days = Vec::new();
                    let mut cursor = from;
                    while cursor <= 7 {
                        days.push(cursor);
                        cursor += 1;
                    }
                    let mut cursor = 1;
                    while cursor <= to {
                        days.push(cursor);
                        cursor += 1;
                    }
                    return normalize_days(days);
                }
                _ => None,
            }
        } else {
            part.parse::<u32>().ok().map(|value| (value, value))
        };
        if let Some((from, to)) = value {
            let mut cursor = from;
            while cursor <= to {
                out.push(cursor);
                cursor += 1;
            }
        }
    }
    normalize_days(out)
}

/// 星期归一：cron 里 0 和 7 都是周日；统一成 1=周一 … 7=周日。
fn normalize_days(mut days: Vec<u32>) -> Vec<u32> {
    days = days
        .into_iter()
        .map(|day| match day {
            0 => 7,
            other => other,
        })
        .filter(|day| (1..=7).contains(day))
        .collect();
    days.sort_unstable();
    days.dedup();
    days
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// 构造一个本地时区的 DateTime（测试用）。
    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        chrono::Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("时间应合法")
    }

    #[test]
    fn daily_expression_has_a_due_time_today() {
        let schedule = parse("0 9 * * *").expect("应能解析");
        let now = at(2026, 11, 27, 10, 30);
        let due = last_due_at(&schedule, now).expect("应有应触发时刻");
        assert_eq!(due, at(2026, 11, 27, 9, 0), "今天 09:00 已过");
    }

    #[test]
    fn due_time_is_yesterday_when_today_has_not_reached_it() {
        let schedule = parse("0 9 * * *").expect("应能解析");
        let now = at(2026, 11, 27, 8, 30);
        let due = last_due_at(&schedule, now).expect("应有应触发时刻");
        assert_eq!(due, at(2026, 11, 26, 9, 0), "今天还没到 09:00 → 昨天");
    }

    #[test]
    fn monthly_expression_expands_the_lookback_window() {
        // 每月 1 号 09:00：11 月 27 日的最近一次是 11 月 1 日
        let schedule = parse("0 9 1 * *").expect("应能解析");
        let now = at(2026, 11, 27, 10, 0);
        let due = last_due_at(&schedule, now).expect("应有应触发时刻");
        assert_eq!(due, at(2026, 11, 1, 9, 0));
    }

    #[test]
    fn every_minute_expression_hits_the_current_minute() {
        let schedule = parse("* * * * *").expect("应能解析");
        let now = at(2026, 11, 27, 10, 30);
        let due = last_due_at(&schedule, now).expect("应有应触发时刻");
        assert_eq!(due, at(2026, 11, 27, 10, 30));
        assert!(matches_now(&schedule, now));
    }

    #[test]
    fn next_due_time_is_in_the_future() {
        let schedule = parse("0 9 * * *").expect("应能解析");
        let now = at(2026, 11, 27, 8, 30);
        let next = next_due_at(&schedule, now).expect("应有下次");
        assert_eq!(next, at(2026, 11, 27, 9, 0));
    }

    #[test]
    fn invalid_expressions_are_rejected_with_a_readable_message() {
        assert!(parse("").is_err());
        let err = parse("not a cron").unwrap_err();
        assert!(err.contains("不合法"), "{err}");
    }

    #[test]
    fn shapes_map_to_system_task_frequencies() {
        assert_eq!(classify("*/15 * * * *"), CronShape::EveryMinutes(15));
        assert_eq!(classify("30 * * * *"), CronShape::Hourly(30));
        assert_eq!(
            classify("0 9 * * *"),
            CronShape::Daily {
                hour: 9,
                minute: 0
            }
        );
        assert_eq!(
            classify("15 8 * * 1-5"),
            CronShape::Weekly {
                hour: 8,
                minute: 15,
                days: vec![1, 2, 3, 4, 5]
            }
        );
        assert_eq!(
            classify("0 9 1 * *"),
            CronShape::Monthly {
                hour: 9,
                minute: 0,
                day: 1
            }
        );
        // 不认识的表达式 → 退回"每天一次"（宁可多跑不可漏跑）
        assert_eq!(classify("0 9 1 11 *"), CronShape::Other);
        assert_eq!(classify("0 9 * * 0"), CronShape::Weekly {
            hour: 9,
            minute: 0,
            days: vec![7]
        });
    }

    #[test]
    fn every_minute_expression_maps_to_every_one_minute() {
        // **回归**（真机验收时发现）：`* * * * *` 曾经落进 `Other`（= 每天一次），
        // 会把"每分钟"降级成"每天"→ 系统兜底漏跑。
        assert_eq!(classify("* * * * *"), CronShape::EveryMinutes(1));
        assert_eq!(classify("*/1 * * * *"), CronShape::EveryMinutes(1));
        // 分钟是 `*` 但小时不是 `*` → 不是"每分钟"
        assert_eq!(classify("* 9 * * *"), CronShape::Other);
    }

    #[test]
    fn six_field_expressions_are_accepted_with_seconds_first() {
        // 6 字段 = 秒 分 时 日 月 周（`cron` crate 0.12 的口径）
        assert_eq!(
            classify("0 0 9 * * *"),
            CronShape::Daily {
                hour: 9,
                minute: 0
            }
        );
        assert_eq!(
            classify("0 30 8 * * 1-5"),
            CronShape::Weekly {
                hour: 8,
                minute: 30,
                days: vec![1, 2, 3, 4, 5]
            }
        );
    }

    #[test]
    fn five_field_expressions_are_normalized_with_zero_seconds() {
        assert!(parse("0 9 * * *").is_ok());
        assert!(parse("*/15 * * * *").is_ok());
        // 字段数不对 → 明确报错
        let err = parse("0 9 *").unwrap_err();
        assert!(err.contains("5~7"), "{err}");
    }
}
