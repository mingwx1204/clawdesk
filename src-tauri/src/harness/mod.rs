//! harness —— CodeWhale 后端移植模块根（方案B：作为 llm/runner 的引擎底座）。
//!
//! 说明：复制自 CodeWhale 的 crate 子模块文件名为 `lib.rs`（原 crate 根），
//! 作为子模块引入时必须用 `#[path]` 显式指定，否则 Rust 2018 模块解析
//! 只查找 `xxx.rs` / `xxx/mod.rs`。

#[path = "core/lib.rs"]
pub mod core;
#[path = "agent/lib.rs"]
pub mod agent;
#[path = "tools/lib.rs"]
pub mod tools;
#[path = "paths/lib.rs"]
pub mod paths;
#[path = "protocol/lib.rs"]
pub mod protocol;
#[path = "hooks/lib.rs"]
pub mod hooks;
#[path = "state/lib.rs"]
pub mod state;
pub mod engine;

pub use core::turn_loop::ENGINE_RT;
