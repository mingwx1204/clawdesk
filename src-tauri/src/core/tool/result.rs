use serde::{Deserialize, Serialize};

/// 工具执行结果 —— 三态（success / error / interrupted）。
///
/// 序列化契约：`{"status": "success", "output": ...}` 等，与前端
/// `src/types/tool.ts` 中 `ToolResult` 逐字段镜像。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ToolResult {
    Success { output: serde_json::Value },
    Error { message: String },
    Interrupted { reason: String },
}

impl ToolResult {
    pub fn ok(output: impl Into<serde_json::Value>) -> Self {
        Self::Success {
            output: output.into(),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    pub fn interrupted(reason: impl Into<String>) -> Self {
        Self::Interrupted {
            reason: reason.into(),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }
}
