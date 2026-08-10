//! `builtin:echo` —— 原样回显参数（链路自测工具）。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// uiPayload 契约：仅前端渲染通道消费（DEV_SPEC.md §8）。
/// 此结构绝不会出现在 ToolContext / 执行结果 / 任何 LLM 上下文构建逻辑中。
const UI_PAYLOAD: &str = r#"{"displayHint":{"icon":"🔁","tone":"info"}}"#;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "echo",
        "原样返回传入的 message 参数（用于工具链路自测）",
        vec![ToolParamDef {
            name: "message".into(),
            param_type: "string".into(),
            description: "要回显的内容".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?
    .with_ui_payload(serde_json::from_str(UI_PAYLOAD).unwrap());

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let message = args
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            Ok(ToolResult::ok(json!({ "echo": message })))
        })
    });

    registry.register(def, handler)
}
