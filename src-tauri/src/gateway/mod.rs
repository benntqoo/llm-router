//! 网关模块：入站协议解析、路由、出站适配、流式管道（按 02 §1.2 目录规划逐步实现）。
//!
//! T3 已包含：`server`（axum 路由与监听）、`auth`（入站鉴权）、`errors`（统一错误结构）、
//! `ratelimit`（限流）、`inbound/openai`（入站解析）。
//! T5 已包含：`router`（别名解析与路由策略）。
//! T6 已包含：`circuit`（熔断）、`circuit_store`（熔断状态落库）、`outbound`（出站发送）、
//! `stream`（SSE 直通）、`fallback`（候选链与首字节前重试）。
//! T7 已包含：`usage`（用量采集：SSE 旁路 tee、usage 解析、估算入口）。

pub mod auth;
pub mod circuit;
pub mod circuit_store;
pub mod errors;
pub mod fallback;
pub mod inbound;
pub mod outbound;
pub mod ratelimit;
pub mod router;
pub mod server;
pub mod stream;
pub mod usage;
