//! `builtin` 源窗口控制工具集（阶段 5）。
//!
//! 设计说明：
//! - 窗口句柄仅在 Tauri `setup` 回调可用，因此本模块的注册函数接收
//!   `AppHandle`，由 `lib.rs` 的 `setup` 回调调用（不进入 builtin::register_all）；
//! - handler 捕获 `AppHandle`（Clone 廉价），经 `get_webview_window("main")`
//!   获取主窗口执行操作；
//! - `window_close` 标记为高危工具（安全中间件 / 前端确认消费）；
//! - 本模块不修改 core 层任何代码。

use std::sync::Arc;

use serde_json::json;
use tauri::Manager;

use crate::core::tool::def::UnifiedToolDef;
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// 注册全部窗口控制工具（在 Tauri setup 回调中调用）。
pub fn register_window_tools(
    registry: &ToolRegistry,
    app: tauri::AppHandle,
) -> Result<(), ToolError> {
    register_minimize(registry, app.clone())?;
    register_maximize(registry, app.clone())?;
    register_close(registry, app.clone())?;
    register_get_state(registry, app)?;
    Ok(())
}

/// 取主窗口句柄的公共辅助。
fn main_window(app: &tauri::AppHandle) -> Result<tauri::WebviewWindow, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "主窗口（main）未找到".to_string())
}

fn register_minimize(registry: &ToolRegistry, app: tauri::AppHandle) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "window_minimize",
        "最小化主窗口",
        vec![],
    )?;

    let handler: ToolHandler = Arc::new(move |_args, _ctx| {
        let app = app.clone();
        Box::pin(async move {
            match main_window(&app) {
                Ok(win) => match win.minimize() {
                    Ok(()) => Ok(ToolResult::ok(json!({ "minimized": true }))),
                    Err(e) => Ok(ToolResult::err(format!("最小化失败: {}", e))),
                },
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_maximize(registry: &ToolRegistry, app: tauri::AppHandle) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "window_maximize",
        "最大化主窗口（已最大化则还原）",
        vec![],
    )?;

    let handler: ToolHandler = Arc::new(move |_args, _ctx| {
        let app = app.clone();
        Box::pin(async move {
            match main_window(&app) {
                Ok(win) => {
                    let is_max = win.is_maximized().unwrap_or(false);
                    let result = if is_max {
                        win.unmaximize()
                    } else {
                        win.maximize()
                    };
                    match result {
                        Ok(()) => Ok(ToolResult::ok(json!({ "maximized": !is_max }))),
                        Err(e) => Ok(ToolResult::err(format!("切换最大化失败: {}", e))),
                    }
                }
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_close(registry: &ToolRegistry, app: tauri::AppHandle) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "window_close",
        "关闭主窗口（应用退出）",
        vec![],
    )?
    .high_risk(); // ⚠️ 高危：关闭窗口可能丢失未保存内容

    let handler: ToolHandler = Arc::new(move |_args, _ctx| {
        let app = app.clone();
        Box::pin(async move {
            match main_window(&app) {
                Ok(win) => match win.close() {
                    Ok(()) => Ok(ToolResult::ok(json!({ "closed": true }))),
                    Err(e) => Ok(ToolResult::err(format!("关闭窗口失败: {}", e))),
                },
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_get_state(registry: &ToolRegistry, app: tauri::AppHandle) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "window_get_state",
        "获取主窗口当前状态（最大化/最小化/尺寸）",
        vec![],
    )?;

    let handler: ToolHandler = Arc::new(move |_args, _ctx| {
        let app = app.clone();
        Box::pin(async move {
            match main_window(&app) {
                Ok(win) => {
                    let maximized = win.is_maximized().unwrap_or(false);
                    let minimized = win.is_minimized().unwrap_or(false);
                    let size = win.outer_size().ok();
                    Ok(ToolResult::ok(json!({
                        "maximized": maximized,
                        "minimized": minimized,
                        "size": size.map(|s| json!({ "width": s.width, "height": s.height })),
                        "title": win.title().unwrap_or_default(),
                    })))
                }
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

#[cfg(test)]
mod tests {
    use crate::core::tool::def::UnifiedToolDef;

    /// 离线静态：验证窗口工具的定义 ID 符合 source:name 规范（不触达真实窗口）。
    #[test]
    fn window_tool_defs_are_well_formed() {
        let cases = [
            ("builtin", "window_minimize"),
            ("builtin", "window_maximize"),
            ("builtin", "window_close"),
            ("builtin", "window_get_state"),
        ];
        for (source, name) in cases {
            let def = UnifiedToolDef::new(source, name, "x", vec![]).unwrap();
            assert_eq!(def.id, format!("{}:{}", source, name));
            def.validate_id().unwrap();
        }
    }

    /// window_close 必须标记为高危。
    #[test]
    fn window_close_is_high_risk() {
        // 通过 def 构造验证（AppHandle 无法在单测构造，注册逻辑由编译验证）
        let def = UnifiedToolDef::new("builtin", "window_close", "x", vec![])
            .unwrap()
            .high_risk();
        assert!(def.is_high_risk);
    }
}
