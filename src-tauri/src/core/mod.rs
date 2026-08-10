//! ClawDesk 核心层 —— 永久冻结层。
//!
//! 本模块一次成型后禁止任何代码修改（DEV_SPEC.md §4.1）。
//! 核心层只提供统一数据结构、动态注册表、调度器、错误/结果类型与中间件 trait，
//! 不包含任何具体工具实现。
//!
//! 关于 `#![allow(dead_code)]`：核心层全部 API 为后续阶段（阶段 1 起：
//! 执行器注册、中间件挂载、配置熔断阈值等）预置的公共契约，阶段 0 内
//! 无 crate 内消费者属预期状态，故在模块级统一豁免 dead_code 警告。

#![allow(dead_code)]

pub mod tool;

pub mod prelude {
    //! 核心层公共导出，供适配器/执行器/命令层引用。
    //!
    //! 说明：prelude 是面向后续阶段（阶段 1 起）的公共 API 面，
    //! 阶段 0 尚无消费者属预期状态，故抑制未使用警告。

    #[allow(unused_imports)]
    pub use super::tool::context::ToolContext;
    #[allow(unused_imports)]
    pub use super::tool::def::{ToolParamDef, UnifiedToolDef};
    #[allow(unused_imports)]
    pub use super::tool::dispatcher::{BoxFuture, Middleware, ToolCall, ToolDispatcher};
    #[allow(unused_imports)]
    pub use super::tool::error::{ToolError, ToolErrorKind};
    #[allow(unused_imports)]
    pub use super::tool::registry::{ToolHandler, ToolRegistry};
    #[allow(unused_imports)]
    pub use super::tool::result::ToolResult;
}
