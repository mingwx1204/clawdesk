//! engine —— LLM 引擎（HTTP 客户端 / SSE 解析 / 参数 / 上下文压缩 / 全局配置）。

pub mod client;
pub mod config;
pub mod context;
pub mod param;
pub mod stream;
