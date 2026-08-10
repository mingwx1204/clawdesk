//! 适配器层 —— 对接外部能力（DEV_SPEC.md §4.2）。
//!
//! 分层契约：
//! - 适配器负责将外部协议（MCP JSON-RPC / SkillHub 技能文件）转换为
//!   统一数据结构（UnifiedToolDef），注册进 ToolRegistry；
//! - 新增外部能力即新增适配器子模块，**不得修改 core**；
//! - 阶段 4 交付：mcp（外置 MCP 客户端）、skillhub（技能中心）。

pub mod mcp;
pub mod skillhub;

use std::sync::Arc;

use crate::core::tool::error::ToolError;
use crate::core::tool::registry::ToolRegistry;

/// 注册全部适配器提供的**内置**工具（无需外部配置的部分）。
///
/// - skillhub：内置示例技能（目录扫描由运行时按需调用）；
/// - mcp：不内置注册，需通过 `McpAdapter::add_server` + `register_tools`
///   按配置动态接入。
pub fn register_builtin_adapters(registry: &Arc<ToolRegistry>) -> Result<(), ToolError> {
    skillhub::register_builtin_skills(registry)?;
    Ok(())
}
