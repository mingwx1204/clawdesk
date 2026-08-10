//! 执行器层 —— 具体工具实现所在（DEV_SPEC.md §4.3）。
//!
//! 分层契约：
//! - 每个工具一个执行器文件，通过 `UnifiedToolDef::new(source, name, ...)`
//!   声明自身并注册进 ToolRegistry；
//! - 执行器层**只增不改**，新增工具即新增文件/注册函数；
//! - 本层不修改 core 层任何代码。

pub mod builtin;

use std::sync::Arc;

use crate::core::tool::error::ToolError;
use crate::core::tool::registry::ToolRegistry;

/// 注册全部内置（builtin 源）工具 —— 应用启动时调用一次。
pub fn register_builtin_tools(registry: &Arc<ToolRegistry>) -> Result<(), ToolError> {
    builtin::register_all(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::context::ToolContext;
    use crate::core::tool::dispatcher::{ToolCall, ToolDispatcher};
    use crate::core::tool::result::ToolResult;

    /// 测试辅助：创建已注册全部内置工具的注册表 + 调度器。
    fn setup() -> (Arc<ToolRegistry>, ToolDispatcher) {
        let registry = Arc::new(ToolRegistry::new());
        register_builtin_tools(&registry).unwrap();
        let dispatcher = ToolDispatcher::new(registry.clone());
        (registry, dispatcher)
    }

    /// 辅助：构造一次工具调用。
    fn tool_call(tool_id: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("t-{}", tool_id),
            tool_id: tool_id.into(),
            arguments,
            round: 1,
        }
    }

    // ────────────────────────────────────────────────
    // 注册表级测试
    // ────────────────────────────────────────────────

    #[test]
    fn registers_all_builtin_tools() {
        let (registry, _) = setup();
        // 既有工具 + attachment_save + web_search 等；用下限避免频繁改数字
        assert!(registry.list().len() >= 18, "内置工具数异常: {}", registry.list().len());
        assert!(registry.sources().contains(&"builtin".to_string()));
        // 新增强功能工具必须注册成功
        assert!(registry.get("builtin:web_search").is_some(), "web_search 未注册");
    }

    // ────────────────────────────────────────────────
    // echo：链路自测工具
    // ────────────────────────────────────────────────

    #[tokio::test]
    async fn echo_dispatch_round_trip() {
        let (_, dispatcher) = setup();
        let result = dispatcher
            .dispatch(
                tool_call("builtin:echo", serde_json::json!({"message": "hi"})),
                ToolContext::default(),
            )
            .await
            .unwrap();
        match result {
            ToolResult::Success { output } => assert_eq!(output["echo"], "hi"),
            _ => panic!("期望 success"),
        }
    }

    // ────────────────────────────────────────────────
    // get_time：无参工具
    // ────────────────────────────────────────────────

    #[tokio::test]
    async fn get_time_dispatch_ok() {
        let (_, dispatcher) = setup();
        let result = dispatcher
            .dispatch(tool_call("builtin:get_time", serde_json::json!({})), ToolContext::default())
            .await
            .unwrap();
        match result {
            ToolResult::Success { output } => {
                assert!(!output["iso"].as_str().unwrap().is_empty());
            }
            _ => panic!("期望 success"),
        }
    }

    // ────────────────────────────────────────────────
    // calculate：四则运算
    // ────────────────────────────────────────────────

    #[tokio::test]
    async fn calculate_dispatch_ok() {
        let (_, dispatcher) = setup();
        let result = dispatcher
            .dispatch(
                tool_call("builtin:calculate", serde_json::json!({"expression": "1 + 2 * 3"})),
                ToolContext::default(),
            )
            .await
            .unwrap();
        match result {
            ToolResult::Success { output } => assert_eq!(output["result"], 7.0),
            _ => panic!("期望 success"),
        }
    }

    #[tokio::test]
    async fn calculate_rejects_invalid_expression() {
        let (_, dispatcher) = setup();
        let result = dispatcher
            .dispatch(
                tool_call("builtin:calculate", serde_json::json!({"expression": "1 ++ 2"})),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(matches!(result, ToolResult::Error { .. }));
    }

    // ────────────────────────────────────────────────
    // generate_image：程序化占位图（纯本地计算，无网络）
    // ────────────────────────────────────────────────

    #[tokio::test]
    async fn generate_image_dispatch_ok() {
        let (_, dispatcher) = setup();
        let result = dispatcher
            .dispatch(
                tool_call(
                    "builtin:generate_image",
                    serde_json::json!({"prompt": "测试图", "width": 128, "height": 128}),
                ),
                ToolContext::default(),
            )
            .await
            .unwrap();
        match result {
            ToolResult::Success { output } => {
                assert!(output["dataUrl"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/png;base64,"));
                assert_eq!(output["width"], 128);
                assert_eq!(output["height"], 128);
            }
            _ => panic!("期望 success"),
        }
    }

    // ────────────────────────────────────────────────
    // ocr：离线静态测试（无效 base64 在解码阶段即失败，
    // 不触达 tesseract 外部进程，无网络请求）
    // ────────────────────────────────────────────────

    #[tokio::test]
    async fn ocr_rejects_invalid_base64() {
        let (_, dispatcher) = setup();
        let result = dispatcher
            .dispatch(
                tool_call("builtin:ocr", serde_json::json!({"image_base64": "!!!bad!!!"})),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(matches!(result, ToolResult::Error { .. }));
    }

    #[tokio::test]
    async fn ocr_rejects_empty_base64() {
        let (_, dispatcher) = setup();
        let result = dispatcher
            .dispatch(
                tool_call("builtin:ocr", serde_json::json!({"image_base64": ""})),
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(matches!(result, ToolResult::Error { .. }));
    }
}
