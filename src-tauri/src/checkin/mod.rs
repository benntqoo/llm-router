//! 自动签到框架（T15：HAR 导入与模板化；T16：重放引擎；T17：调度器 + 系统兜底；
//! 02 §7.1.1 / §7.1.2 / §7.1.3 / §7.2.1）。
//!
//! 分层：**导入与模板化**（T15）→ **重放引擎**（T16）→ **调度与兜底**（T17）；
//! 通知属 T19，积分回写属 T18。

pub mod cli;
pub mod cron;
pub mod har;
pub mod replay;
pub mod scheduler;
pub mod system;
