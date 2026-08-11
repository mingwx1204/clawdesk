//! `builtin:vm_*` —— 虚拟机内置微信工具集。
//!
//! AI 通过 VNC 屏幕流查看并操作虚拟机（AI-WeChat）里运行的真微信：
//! 截图看界面 → 点击 → 输入/粘贴 → 发送。与本机微信完全隔离。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

fn num(args: &serde_json::Value, key: &str, default: u16) -> u16 {
    args.get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n.min(65535) as u16)
        .unwrap_or(default)
}

fn str_opt(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// 注册全部虚拟机（VNC）工具。
pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    register_status(registry)?;
    register_screenshot(registry)?;
    register_click(registry)?;
    register_type(registry)?;
    register_key(registry)?;
    register_whitelist(registry)?;
    register_send(registry)?;
    Ok(())
}

/// 白名单（可聊天对象）。
fn register_whitelist(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_whitelist",
        "查看或设置虚拟机微信的『可聊天对象』白名单（逗号/顿号分隔的微信昵称，如『小明，小红』；传空字符串 = 清空）。安全红线：AI 只能给白名单里的人发消息，白名单为空时 vm_send 会被拒绝。不传 users 参数 = 查询当前白名单",
        vec![ToolParamDef {
            name: "users".into(),
            param_type: "string".into(),
            description: "可聊天对象名单（逗号/顿号分隔）；省略 = 只查询".into(),
            required: false,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            if let Some(users) = str_opt(&args, "users") {
                match crate::vm_vnc::vm_whitelist_set(users) {
                    Ok(v) => Ok(ToolResult::ok(v)),
                    Err(e) => Ok(ToolResult::err(e)),
                }
            } else {
                match crate::vm_vnc::vm_whitelist_get() {
                    Ok(v) => Ok(ToolResult::ok(v)),
                    Err(e) => Ok(ToolResult::err(e)),
                }
            }
        })
    });
    registry.register(def, handler)
}

/// 高级发送（白名单强制校验）。
fn register_send(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_send",
        "通过虚拟机里的真微信给指定对象发消息（自动完成：Ctrl+F 搜索联系人 → 粘贴名称 → 回车打开会话 → 粘贴内容 → 回车发送）。⚠️ 安全红线：to 必须在白名单（vm_whitelist）内，否则拒绝发送。发送前先 vm_screenshot 确认对象正确，发送后再截图确认成功",
        vec![
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
            match crate::vm_vnc::vm_send(to, text) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_status(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_status",
        "查看虚拟机（AI-WeChat，内置真微信）的 VNC 连接状态与屏幕尺寸。操作虚拟机前先调用确认已连接",
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
            match crate::vm_vnc::vm_status() {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_screenshot(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_screenshot",
        "截取虚拟机（内置真微信）当前屏幕，返回 PNG Data URL。这是 AI 查看微信界面的唯一途径：先截图观察（会话列表/聊天窗口/输入框位置），操作后再截图确认效果",
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
            if !crate::vm_vnc::is_connected() {
                return Ok(ToolResult::err("虚拟机 VNC 未连接：请在主界面「虚拟机微信」面板点连接"));
            }
            match crate::vm_vnc::vm_screenshot() {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_click(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_click",
        "在虚拟机屏幕 (x,y) 处鼠标左键点击。坐标相对屏幕左上角，必须基于 vm_screenshot 截图判断位置。先发送按下再松开（即单击）；需要双击可连续调用两次",
        vec![
            ToolParamDef {
                name: "x".into(),
                param_type: "number".into(),
                description: "屏幕横坐标".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "y".into(),
                param_type: "number".into(),
                description: "屏幕纵坐标".into(),
                required: true,
                enum_values: None,
                default: None,
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            if !crate::vm_vnc::is_connected() {
                return Ok(ToolResult::err("虚拟机 VNC 未连接：请在主界面「虚拟机微信」面板点连接"));
            }
            let x = num(&args, "x", 0);
            let y = num(&args, "y", 0);
            let r1 = crate::vm_vnc::vm_pointer(x, y, 1);
            let r2 = crate::vm_vnc::vm_pointer(x, y, 0);
            match (r1, r2) {
                (Ok(_), Ok(_)) => Ok(ToolResult::ok(json!({ "ok": true, "x": x, "y": y }))),
                (Err(e), _) | (_, Err(e)) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_type(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_type",
        "向虚拟机输入文本（写入客机剪贴板后 Ctrl+V，支持中文）。输入前必须先用 vm_click 把光标点进输入框。只输入不发送，发送用 vm_key(enter)（微信 PC 端 Enter 发送）",
        vec![ToolParamDef {
            name: "text".into(),
            param_type: "string".into(),
            description: "要输入的文本（支持中文）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            if !crate::vm_vnc::is_connected() {
                return Ok(ToolResult::err("虚拟机 VNC 未连接：请在主界面「虚拟机微信」面板点连接"));
            }
            let text = str_opt(&args, "text").unwrap_or_default();
            match crate::vm_vnc::paste_and_send(&text) {
                Ok(()) => Ok(ToolResult::ok(json!({ "ok": true, "chars": text.chars().count() }))),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_key(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_key",
        "向虚拟机发送按键。支持：enter(发送)/esc/tab/backspace/up/down/left/right/home/end/delete/space/f1~f12/单个字符；组合键用 +：ctrl+f、shift+enter、alt+1 等。微信 PC 端 Enter 直接发送消息",
        vec![ToolParamDef {
            name: "key".into(),
            param_type: "string".into(),
            description: "按键名或组合（如 enter、ctrl+f、shift+enter）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            if !crate::vm_vnc::is_connected() {
                return Ok(ToolResult::err("虚拟机 VNC 未连接：请在主界面「虚拟机微信」面板点连接"));
            }
            let key = str_opt(&args, "key").unwrap_or_default();
            match crate::vm_vnc::press_combo(&key) {
                Ok(()) => Ok(ToolResult::ok(json!({ "ok": true, "key": key }))),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

#[cfg(test)]
mod tests {
    use crate::core::tool::def::UnifiedToolDef;

    #[test]
    fn vm_tool_defs_well_formed() {
        for name in ["vm_status", "vm_screenshot", "vm_click", "vm_type", "vm_key", "vm_whitelist", "vm_send"] {
            let def = UnifiedToolDef::new("builtin", name, "x", vec![]).unwrap();
            assert_eq!(def.id, format!("builtin:{name}"));
            def.validate_id().unwrap();
        }
    }
}
