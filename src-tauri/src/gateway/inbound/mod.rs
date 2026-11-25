//! 入站协议处理。
//!
//! v1 只有 OpenAI Chat Completions（02 §0.1）；Anthropic / Responses 入站是 v2 预留，
//! 不在此实现（AGENT.md §5-1）。

pub mod openai;
