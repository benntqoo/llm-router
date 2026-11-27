//! Tauri IPC 命令层（按域拆分：channel / vault / usage / checkin / notify / copilot / account / settings）。
//!
//! T0 只有 `gateway` 一个域。

pub mod channel;
pub mod checkin;
pub mod circuit;
pub mod client_key;
pub mod copilot;
pub mod gateway;
pub mod notify;
pub mod price;
pub mod usage;
pub mod vault;
