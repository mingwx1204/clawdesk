use serde::{Deserialize, Serialize};

/// 工具错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorKind {
    /// 工具定义非法（id 不符合 source:name 规范、空 source/name 等）
    InvalidDef,
    /// 工具重复注册
    AlreadyRegistered,
    /// 工具未找到
    NotFound,
    /// 工具循环轮次超过熔断上限
    MaxRoundsExceeded,
    /// 安全中间件拦截
    MiddlewareRejected,
    /// 执行器执行失败
    ExecutionFailed,
    /// 内部错误
    Internal,
}

/// 工具层统一错误 —— 可跨 IPC 序列化传输（DEV_SPEC.md §5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub kind: ToolErrorKind,
    pub message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn invalid_def(message: impl Into<String>) -> Self {
        Self::new(ToolErrorKind::InvalidDef, message)
    }

    pub fn already_registered(id: &str) -> Self {
        Self::new(ToolErrorKind::AlreadyRegistered, format!("工具已注册: {}", id))
    }

    pub fn not_found(id: &str) -> Self {
        Self::new(ToolErrorKind::NotFound, format!("工具未找到: {}", id))
    }

    pub fn max_rounds(round: usize, max: usize) -> Self {
        Self::new(
            ToolErrorKind::MaxRoundsExceeded,
            format!("工具循环轮次 {} 超过熔断上限 {}", round, max),
        )
    }

    pub fn middleware_rejected(name: &str, message: impl Into<String>) -> Self {
        Self::new(
            ToolErrorKind::MiddlewareRejected,
            format!("中间件 {} 拦截: {}", name, message.into()),
        )
    }

    pub fn execution(message: impl Into<String>) -> Self {
        Self::new(ToolErrorKind::ExecutionFailed, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ToolErrorKind::Internal, message)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.kind, self.message)
    }
}

impl std::error::Error for ToolError {}
