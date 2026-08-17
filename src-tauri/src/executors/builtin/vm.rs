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
    register_click_spot(registry)?;
    register_click(registry)?;
    register_type(registry)?;
    register_paste_utf8(registry)?;
    register_unlock(registry)?;
    register_locate(registry)?;
    register_key(registry)?;
    register_whitelist(registry)?;
    register_send(registry)?;
    register_fetch(registry)?;
    register_readonly(registry)?;
    register_tts(registry)?;
    Ok(())
}

/// AI 语音说话（克隆音色 → VB-Cable → 微信麦克风）。
fn register_tts(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_tts_speak",
        "让 AI 用克隆女声开口说话：合成语音并通过虚拟声卡播放进虚拟机微信的麦克风。用途：1. 微信语音通话中回复对方（对方能听到 AI 的声音）；2. 发语音消息前试听。注意：播放期间请勿同时操作虚拟机键盘（声音走宿主播放设备到 VB-Cable 再到虚拟机麦克风）",
        vec![ToolParamDef {
            name: "text".into(),
            param_type: "string".into(),
            description: "要说出的文本内容".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let text = str_opt(&args, "text").unwrap_or_default();
            match crate::vm_vnc::vm_tts_speak(text).await {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// 只读模式开关。
fn register_readonly(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_set_readonly",
        "设置 AI 微信只读模式（true=只能截图查看，禁止点击/输入/发送；false=恢复操作权限）。操作微信前如不确定用户是否授权操作，先查询 vm_status 的 readonly 字段；处于只读模式时只能 vm_screenshot",
        vec![ToolParamDef {
            name: "enabled".into(),
            param_type: "boolean".into(),
            description: "true=开启只读（只能看），false=关闭只读（可操作）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let enabled = args.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            match crate::vm_vnc::vm_readonly_set(enabled) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// 拉取宿主文件进虚拟机（表情包/图片）。
fn register_fetch(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_fetch_file",
        "把宿主共享目录里的文件（表情包/图片）下载进虚拟机（自动 Win+R + 下载命令）。共享目录：D:\\AI-WeChat\\share（AI 先把图片保存到这里，如用生图工具生成表情包）。返回虚拟机内的图片路径。发送方法：微信聊天框点 + → 文件 → 对话框 Ctrl+L → 输入路径 → 回车 → 发送",
        vec![ToolParamDef {
            name: "name".into(),
            param_type: "string".into(),
            description: "文件名（如 emoji_01.png，只能是文件名，不能含路径）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let name = str_opt(&args, "name").unwrap_or_default();
            match crate::vm_vnc::vm_fetch_file(name) {
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
        "通过虚拟机里的真微信给指定对象发消息。★ 前置条件（必须先做）：vm_screenshot 确认微信主窗口在前台，且【已经打开了 to 的聊天窗口】（用 vm_click_spot(chat1/2/3) 点会话，或 vm_click_spot(search)+vm_paste_utf8+vm_key(ctrl+v)+vm_key(enter) 搜索打开）。然后 vm_send 会：点击输入框 → 打字 → 回车发送。⚠️ 不要在微信主窗口未在前台/未打开聊天窗口时调用，否则发不出去。⚠️ 安全红线：to 必须在白名单（vm_whitelist）内。发送前先 vm_screenshot 确认对象正确，发送后再截图确认成功",
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
        "查看虚拟机（AI-WeChat，内置真微信）的 VNC 连接状态与屏幕尺寸。★ 连接会自动建立（connected=false 时工具内部会自动重连），无需用户手动操作面板；若返回 connected=false 表示虚拟机可能未运行",
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
        "截取虚拟机（内置真微信）当前屏幕，返回【screenText（自动读屏结果：界面文字、联系人名、聊天消息内容）】+【path（完整截图文件）】。★ 截图不依赖 VNC 连接。★★ 你是文本模型看不懂图片——screenText 就是你的眼睛，以它为准判断：是不是微信界面、有没有新消息、聊天内容。⚠️ 不要用 python/ocr/terminal 工具自己分析截图（截图已自带读屏，自己做纯属浪费回合）。操作（vm_send/vm_click_spot/vm_key）前后各截一次确认",
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
            match crate::vm_vnc::vm_screenshot().await {
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
        "在虚拟机屏幕 (x,y) 处鼠标左键点击（连接自动建立）。坐标相对屏幕左上角，必须基于 vm_screenshot 截图判断位置。先发送按下再松开（即单击）；需要双击可连续调用两次",
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
            let x = num(&args, "x", 0);
            let y = num(&args, "y", 0);
            // ★ 按下→110ms→松开：0ms 间隔的按下/松开可能被 VNC 服务器合并丢弃
            let r1 = crate::vm_vnc::vm_pointer(x, y, 1);
            tokio::time::sleep(std::time::Duration::from_millis(110)).await;
            let r2 = crate::vm_vnc::vm_pointer(x, y, 0);
            match (r1, r2) {
                (Ok(_), Ok(_)) => Ok(ToolResult::ok(json!({ "ok": true, "x": x, "y": y }))),
                (Err(e), _) | (_, Err(e)) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// ★ 语义点击（2026-08-16 新增）：AI 不需要自己估算坐标，传语义位置即可。
fn register_click_spot(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_click_spot",
        "按语义位置点击虚拟机屏幕（★ 推荐优先使用，比 vm_click 好用——不用自己算坐标）。支持：input=微信聊天输入框（打字前点它聚焦）、send=发送按钮、search=顶部搜索框、chat1/chat2/chat3=聊天列表第1/2/3条会话、center=屏幕中央。微信操作流程：vm_click_spot(chat1) 打开会话 → vm_click_spot(input) 聚焦输入框 → vm_type 打字 → vm_key(enter) 发送。操作后 vm_screenshot 确认",
        vec![ToolParamDef {
            name: "spot".into(),
            param_type: "string".into(),
            description: "语义位置：input / send / search / chat1 / chat2 / chat3 / center".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let spot = str_opt(&args, "spot").unwrap_or_default();
            match crate::vm_vnc::vm_click_spot(spot) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

fn register_type(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_type",
        "向虚拟机输入文本（Unicode 直接打字，支持中文，不依赖剪贴板）。输入前必须先用 vm_click_spot(input) 或 vm_click 把光标点进输入框。只输入不发送，发送用 vm_key(enter)（微信 PC 端 Enter 发送）",
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
            let text = str_opt(&args, "text").unwrap_or_default();
            match crate::vm_vnc::type_unicode(&text) {
                Ok(()) => Ok(ToolResult::ok(json!({ "ok": true, "chars": text.chars().count() }))),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// ★ vm_paste_utf8：把中文文本正确放进 VM 剪贴板（记事本中转，解决微信不认 Unicode 直接打字的编码问题）。
fn register_paste_utf8(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_paste_utf8",
        "把中文文本放进虚拟机剪贴板（记事本中转，UTF-8 正确不乱码）。用法：先 vm_click_spot(input) 把光标点进微信输入框 → vm_paste_utf8(中文消息) → vm_key(ctrl+v) 粘贴 → vm_key(enter) 发送。★ 注意：vm_type 打中文微信不认（会乱码），发中文必须用本工具 + ctrl+v",
        vec![ToolParamDef {
            name: "text".into(),
            param_type: "string".into(),
            description: "要放入剪贴板的中文文本".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let text = str_opt(&args, "text").unwrap_or_default();
            match crate::vm_vnc::vm_paste_utf8(text) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// ★ vm_unlock：虚拟机锁屏时自动输入密码解锁（用户要求"锁屏让她自己开"）。
fn register_unlock(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_unlock",
        "解锁虚拟机锁屏（自动输入密码 13669403240）。★ 当 vm_screenshot 的 screenDesc 显示【锁屏/登录界面/输入密码】时调用，解锁后继续操作微信。解锁后截图确认桌面，再 Ctrl+Alt+W 打开微信",
        vec![],
    )?;
    let handler: ToolHandler = Arc::new(|_args, _ctx| {
        Box::pin(async move {
            match crate::vm_vnc::vm_unlock() {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });
    registry.register(def, handler)
}

/// ★ vm_locate：用 LocateAnything-3B 视觉定位模型在截图中找目标元素，返回像素坐标。
fn register_locate(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "vm_locate",
        "在虚拟机屏幕截图中定位目标元素（输入框/发送按钮/搜索框/聊天列表项等），返回中心像素坐标。★ 用英文描述目标效果最好，如 'chat input box at the bottom'、'send button'、'search box at the top'。返回 x,y 坐标后用 vm_click(x,y) 点击",
        vec![ToolParamDef {
            name: "target".into(),
            param_type: "string".into(),
            description: "要定位的目标元素描述（英文效果最好）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let target = str_opt(&args, "target").unwrap_or_default();
            match crate::vm_vnc::vm_locate(target).await {
                Ok(v) => Ok(ToolResult::ok(v)),
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
        for name in ["vm_status", "vm_screenshot", "vm_click", "vm_type", "vm_key", "vm_whitelist", "vm_send", "vm_fetch_file", "vm_set_readonly", "vm_tts_speak"] {
            let def = UnifiedToolDef::new("builtin", name, "x", vec![]).unwrap();
            assert_eq!(def.id, format!("builtin:{name}"));
            def.validate_id().unwrap();
        }
    }
}
