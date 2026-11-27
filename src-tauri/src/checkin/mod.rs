//! 自动签到框架（T15：HAR 导入与模板化；T16：重放引擎；T17：调度器 + 系统兜底；
//! T18：站点插件 + 浏览器辅轨；T19：结果通知；02 §7.1.1 / §7.1.2 / §7.1.3 / §7.2.1 / §7.4 / §7.5）。
//!
//! 分层：**导入与模板化**（T15）→ **重放引擎**（T16）→ **调度与兜底**（T17）；
//! **站点插件**（T18）在调度层之下分派：`har` 走重放引擎，内置插件走
//! `checkin::sites::*`（ModelScope 接口型 / MiniMax Code 辅轨型）；
//! **结果通知**（T19）在调度层之上：`checkin::notify` 覆盖全部执行路径。

pub mod cli;
pub mod cron;
pub mod har;
pub mod notify;
pub mod plugin;
pub mod replay;
pub mod reward;
pub mod scheduler;
pub mod sidecar;
pub mod sites;
pub mod system;
pub mod system_notify;
