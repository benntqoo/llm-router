//! 网关模块：入站协议解析、路由、出站适配、流式管道（按 02 §1.2 目录规划逐步实现）。
//!
//! T3 已包含：`server`（axum 路由与监听）、`auth`（入站鉴权）、`errors`（统一错误结构）、
//! `ratelimit`（限流）、`inbound/openai`（入站解析）。
//! T4/T5 起补：`router`（别名解析）、`fallback`（熔断重试）、`sse`（流式直通）。

pub mod auth;
pub mod errors;
pub mod inbound;
pub mod ratelimit;
pub mod server;
