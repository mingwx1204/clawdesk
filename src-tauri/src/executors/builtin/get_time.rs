//! `builtin:get_time` —— 获取当前本地时间。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::UnifiedToolDef;
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// uiPayload 契约：仅前端渲染通道消费（DEV_SPEC.md §8）。
const UI_PAYLOAD: &str = r#"{"displayHint":{"icon":"🕒","tone":"neutral"}}"#;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "get_time",
        "获取当前本地时间与日期",
        vec![],
    )?
    .with_ui_payload(serde_json::from_str(UI_PAYLOAD).unwrap());

    let handler: ToolHandler = Arc::new(|_args, _ctx| {
        Box::pin(async move {
            let now = chrono::Local::now();
            Ok(ToolResult::ok(json!({
                "iso": now.to_rfc3339(),
                "date": now.format("%Y-%m-%d").to_string(),
                "time": now.format("%H:%M:%S").to_string(),
                "weekday": now.format("%A").to_string(),
            })))
        })
    });

    registry.register(def, handler)
}
