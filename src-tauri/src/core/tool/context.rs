use serde::{Deserialize, Serialize};

/// 工具执行上下文。
///
/// 设计约束（DEV_SPEC.md §8）：本结构只承载执行期数据，
/// **ui_payload 绝不进入此处** —— 它是 UnifiedToolDef 渲染通道的专属载荷，
/// 与 LLM 上下文/工具执行上下文完全隔离。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolContext {
    /// 当前工具循环轮次（从 1 开始），由调度器在执行前写入。
    pub round: usize,
    /// 会话标识（阶段 4 MCP 会话等可复用）。
    pub session_id: Option<String>,
    /// 执行超时（秒）。
    pub timeout_secs: Option<u64>,
    /// 内部传递数据（执行器间透传，不序列化业务载荷）。
    pub data: serde_json::Map<String, serde_json::Value>,
}
