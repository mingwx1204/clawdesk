//! `builtin:wechat_ui_*` —— 独立微信（UI 自动化）工具集。
//!
//! 给 AI 准备一个独立的微信账号（本机多开），AI 通过截图 + 鼠标键盘模拟
//! 直接操作微信窗口，与 `wechat.rs` 的 iLink Bot 路线并存。
//! 安全红线：`wechat_ui_send` 强制白名单校验（见 `crate::wechat_ui`）。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

fn num(args: &serde_json::Value, key: &str, default: i32) -> i32 {
    args.get(key)
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .unwrap_or(default)
}

fn str_opt(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// 注册全部独立微信（UI 自动化）工具。
pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    register_list(registry)?;
    register_screenshot(registry)?;
    register_click(registry)?;
    register_type(registry)?;
    register_key(registry)?;
    register_scroll(registry)?;
    register_whitelist(registry)?;
    register_send(registry)?;
    Ok(())
}

/// 列出全部微信窗口（多开识别：标题含"微信"/WeChat）。
fn register_list(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_list",
        "列出当前电脑上所有微信窗口（本机多开时每个微信账号一个窗口）。返回每个窗口的句柄 hwnd、标题、位置尺寸。AI 操作独立微信前先调用本工具确认目标窗口，多开时把独立账号的窗口标题（如「微信」或个人号名称）与 hwnd 记住，后续操作传该 hwnd",
        vec![ToolParamDef {
            name: "unused".into(),
            param_type: "string".into(),
            description: "无需参数".into(),
            required: false,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|_args, _ctx| {
        Box::pin(async move {
            let list = crate::wechat_ui::wechat_ui_list_windows();
            if list.is_empty() {
                Ok(ToolResult::err("未找到任何微信窗口：请先在电脑上打开/登录微信（可多开独立账号）"))
            } else {
                Ok(ToolResult::ok(json!({ "windows": list, "count": list.len() })))
            }
        })
    });
    registry.register(def, handler)
}

/// 截取指定微信窗口画面。
fn register_screenshot(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_screenshot",
        "截取指定微信窗口的实时画面（PNG Data URL），返回图片宽度高度。AI 操作微信前先截图观察界面布局（联系人列表/搜索框/聊天输入框位置），操作后再截图确认效果",
        vec![
            ToolParamDef {
                name: "window_id".into(),
                param_type: "string".into(),
                description: "微信窗口句柄（wechat_ui_list 返回的 hwnd，可传字符串数字）；不传则自动选第一个微信窗口".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "window_title".into(),
                param_type: "string".into(),
                description: "按标题模糊匹配微信窗口（与 window_id 二选一）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            match crate::wechat_ui::wechat_ui_screenshot(str_opt(&args, "window_id"), str_opt(&args, "window_title")) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// 点击。
fn register_click(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_click",
        "在指定微信窗口内 (x,y) 处左键点击。坐标相对窗口左上角（0,0 起），单位像素，必须基于 wechat_ui_screenshot 的截图判断位置。double=true 双击（如双击会话打开聊天）。点击后建议截图确认",
        vec![
            ToolParamDef {
                name: "window_id".into(),
                param_type: "string".into(),
                description: "微信窗口句柄（可省略自动选第一个微信窗口）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "x".into(),
                param_type: "number".into(),
                description: "窗口内横坐标（相对窗口左边缘）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "y".into(),
                param_type: "number".into(),
                description: "窗口内纵坐标（相对窗口上边缘）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "double".into(),
                param_type: "boolean".into(),
                description: "是否双击（默认 false）".into(),
                required: false,
                enum_values: None,
                default: Some(json!(false)),
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let double = args.get("double").and_then(|v| v.as_bool()).unwrap_or(false);
            match crate::wechat_ui::wechat_ui_click(str_opt(&args, "window_id"), num(&args, "x", 0), num(&args, "y", 0), Some(double)) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// 输入文本。
fn register_type(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_type",
        "向指定微信窗口输入文本（Unicode 模拟键盘，支持中文）。输入前必须先用 wechat_ui_click 把光标点进输入框（微信聊天输入框通常在窗口右下区域）。只输入不发送，发送用 wechat_ui_key(enter) 或 wechat_ui_send",
        vec![
            ToolParamDef {
                name: "window_id".into(),
                param_type: "string".into(),
                description: "微信窗口句柄（可省略自动选第一个微信窗口）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "text".into(),
                param_type: "string".into(),
                description: "要输入的文本（支持中文）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let text = str_opt(&args, "text").unwrap_or_default();
            match crate::wechat_ui::wechat_ui_type(str_opt(&args, "window_id"), text) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// 特殊按键。
fn register_key(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_key",
        "向指定微信窗口发送按键。支持：enter(发送)/esc/tab/backspace/up/down/left/right/home/end/delete/space/f1~f12；组合键用 + 连接：ctrl+f(搜索联系人)、ctrl+a(全选)、shift+enter(换行)、alt+1 等。微信 PC 端 Enter 直接发送消息",
        vec![
            ToolParamDef {
                name: "window_id".into(),
                param_type: "string".into(),
                description: "微信窗口句柄（可省略自动选第一个微信窗口）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "key".into(),
                param_type: "string".into(),
                description: "按键名或组合（如 enter、esc、ctrl+f、shift+enter）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let key = str_opt(&args, "key").unwrap_or_default();
            match crate::wechat_ui::wechat_ui_key(str_opt(&args, "window_id"), key) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// 滚动。
fn register_scroll(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_scroll",
        "在指定微信窗口内 (x,y) 处滚动鼠标滚轮。delta 正数=向上滚，负数=向下滚，步长建议 ±120（多次调用实现长滚动）。用于浏览消息列表/联系人列表",
        vec![
            ToolParamDef {
                name: "window_id".into(),
                param_type: "string".into(),
                description: "微信窗口句柄（可省略自动选第一个微信窗口）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "x".into(),
                param_type: "number".into(),
                description: "窗口内横坐标".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "y".into(),
                param_type: "number".into(),
                description: "窗口内纵坐标".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "delta".into(),
                param_type: "number".into(),
                description: "滚动量（正上负下，±120）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            match crate::wechat_ui::wechat_ui_scroll(
                str_opt(&args, "window_id"),
                num(&args, "x", 0),
                num(&args, "y", 0),
                num(&args, "delta", 120),
            ) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// 白名单（可聊天对象）。
fn register_whitelist(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_whitelist",
        "查看或设置指定微信窗口的『可聊天对象』白名单（逗号/顿号分隔的昵称或备注名，如『小明，小红』；传空字符串 = 清空）。这是安全红线：AI 只能给白名单里的人发消息，白名单为空时 wechat_ui_send 会被拒绝。用户在主界面也能设置。调用时不传 users 参数 = 查询当前白名单",
        vec![
            ToolParamDef {
                name: "window_id".into(),
                param_type: "string".into(),
                description: "微信窗口句柄（可省略自动选第一个微信窗口）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "users".into(),
                param_type: "string".into(),
                description: "可聊天对象名单（逗号/顿号分隔）；省略 = 只查询".into(),
                required: false,
                enum_values: None,
                default: None,
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let window_id = str_opt(&args, "window_id");
            if let Some(users) = str_opt(&args, "users") {
                match crate::wechat_ui::wechat_ui_whitelist(window_id, users) {
                    Ok(v) => Ok(ToolResult::ok(v)),
                    Err(e) => Ok(ToolResult::err(e)),
                }
            } else {
                match crate::wechat_ui::wechat_ui_whitelist_get(window_id) {
                    Ok(v) => Ok(ToolResult::ok(v)),
                    Err(e) => Ok(ToolResult::err(e)),
                }
            }
        })
    });
    registry.register(def, handler)
}

/// 高层发送（白名单强制校验）。
fn register_send(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "wechat_ui_send",
        "通过独立微信给指定对象发消息（高级封装，自动完成：Ctrl+F 搜索联系人 → 输入对象名 → 回车打开会话 → 输入内容 → 回车发送）。⚠️ 安全红线：to 必须在白名单（wechat_ui_whitelist 设置）内，否则拒绝发送。发送前先截图确认对象正确，发送后再截图确认成功",
        vec![
            ToolParamDef {
                name: "window_id".into(),
                param_type: "string".into(),
                description: "微信窗口句柄（可省略自动选第一个微信窗口）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "to".into(),
                param_type: "string".into(),
                description: "接收对象（微信昵称/备注名，须在白名单内）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "text".into(),
                param_type: "string".into(),
                description: "要发送的消息内容".into(),
                required: true,
                enum_values: None,
                default: None,
            },
        ],
    )?
    .high_risk(); // ⚠️ 高危：真实发送消息给真人
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let to = str_opt(&args, "to").unwrap_or_default();
            let text = str_opt(&args, "text").unwrap_or_default();
            match crate::wechat_ui::wechat_ui_send(str_opt(&args, "window_id"), to, text) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

#[cfg(test)]
mod tests {
    use crate::core::tool::def::UnifiedToolDef;

    /// 工具定义 ID 合法。
    #[test]
    fn wechat_ui_tool_defs_well_formed() {
        for name in [
            "wechat_ui_list",
            "wechat_ui_screenshot",
            "wechat_ui_click",
            "wechat_ui_type",
            "wechat_ui_key",
            "wechat_ui_scroll",
            "wechat_ui_whitelist",
            "wechat_ui_send",
        ] {
            let def = UnifiedToolDef::new("builtin", name, "x", vec![]).unwrap();
            assert_eq!(def.id, format!("builtin:{name}"));
            def.validate_id().unwrap();
        }
    }

    /// wechat_ui_send 必须标记高危。
    #[test]
    fn wechat_ui_send_is_high_risk() {
        let def = UnifiedToolDef::new("builtin", "wechat_ui_send", "x", vec![])
            .unwrap()
            .high_risk();
        assert!(def.is_high_risk);
    }
}
