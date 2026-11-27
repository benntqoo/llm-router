//! OAuth 型渠道的凭证获取层（T20：GitHub Copilot；02 §6.2 / §6.2.1）。
//!
//! 本目录**只取凭证，不出站**——出站转发与 R-6 伪装头属 T21。

pub mod copilot;
