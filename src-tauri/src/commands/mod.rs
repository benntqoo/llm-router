//! Tauri IPC 命令层（按域拆分：channel / vault / usage / checkin / account / settings）。
//!
//! T0 只有 `gateway` 一个域。

pub mod gateway;
