//! `builtin:email` —— 邮件发送工具（对标大厂 Agent 的通信能力）。
//!
//! 通过 SMTP 发送邮件。支持常见邮箱（QQ/Gmail/163 等），
//! SMTP 配置从参数传入（仅内存态）。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// 常见 SMTP 服务器预设（端口 + TLS）。
const SMTP_PRESETS: &[(&str, u16, &str)] = &[
    ("qq.com", 587, "STARTTLS"),
    ("gmail.com", 587, "STARTTLS"),
    ("163.com", 465, "TLS"),
    ("126.com", 465, "TLS"),
    ("outlook.com", 587, "STARTTLS"),
    ("office365.com", 587, "STARTTLS"),
];

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "email",
        "通过 SMTP 发送邮件（支持 QQ/Gmail/163 等）。自动推断服务器配置，也可手动指定。",
        vec![
            ToolParamDef {
                name: "to".into(), param_type: "string".into(),
                description: "收件人邮箱".into(), required: true, enum_values: None, default: None,
            },
            ToolParamDef {
                name: "subject".into(), param_type: "string".into(),
                description: "邮件主题".into(), required: true, enum_values: None, default: None,
            },
            ToolParamDef {
                name: "body".into(), param_type: "string".into(),
                description: "邮件正文".into(), required: true, enum_values: None, default: None,
            },
            ToolParamDef {
                name: "from".into(), param_type: "string".into(),
                description: "发件人邮箱（完整地址）".into(), required: true, enum_values: None, default: None,
            },
            ToolParamDef {
                name: "password".into(), param_type: "string".into(),
                description: "SMTP 授权码/密码（QQ/163 需开启 SMTP 并使用授权码）".into(),
                required: true, enum_values: None, default: None,
            },
            ToolParamDef {
                name: "smtp_host".into(), param_type: "string".into(),
                description: "SMTP 服务器（可选，留空自动根据发件人域名推断）".into(),
                required: false, enum_values: None, default: None,
            },
        ],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        let to = args.get("to").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let subject = args.get("subject").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let from = args.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let password = args.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let smtp_host = args.get("smtp_host").and_then(|v| v.as_str()).unwrap_or("").to_string();

        if to.is_empty() || subject.is_empty() || body.is_empty() || from.is_empty() || password.is_empty() {
            return Box::pin(async { Ok(ToolResult::err("参数不完整：to/subject/body/from/password 均为必填")) });
        }
        Box::pin(async move {
            match send_email_sync(&to, &subject, &body, &from, &password, &smtp_host) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("发送失败: {e}"))),
            }
        })
    });

    registry.register(def, handler)
}

fn send_email_sync(to: &str, subject: &str, body: &str, from: &str, password: &str, smtp_host: &str) -> Result<serde_json::Value, String> {
    use lettre::{Message, Transport, transport::smtp::authentication::Credentials};

    let (host, port) = if !smtp_host.is_empty() {
        (smtp_host.to_string(), 587u16)
    } else {
        let domain = from.split('@').nth(1).unwrap_or("qq.com");
        SMTP_PRESETS.iter()
            .find(|(d, _, _)| domain.contains(d))
            .map(|(d, p, _)| (d.to_string(), *p))
            .unwrap_or(("smtp.qq.com".into(), 587))
    };

    let email = Message::builder()
        .from(from.parse().map_err(|e| format!("发件人格式错误: {e}"))?)
        .to(to.parse().map_err(|e| format!("收件人格式错误: {e}"))?)
        .subject(subject)
        .body(body.to_string())
        .map_err(|e| format!("构造邮件失败: {e}"))?;

    let creds = Credentials::new(from.to_string(), password.to_string());
    let mailer = lettre::SmtpTransport::relay(&host)
        .map_err(|e| format!("SMTP 连接 {}:{} 失败: {}. QQ/163 需先开启 SMTP 服务获取授权码(不是邮箱密码)", host, port, e))?
        .port(port)
        .credentials(creds)
        .build();

    mailer.send(&email)
        .map_err(|e| format!("发送失败: {}. 请检查授权码和收件人地址", e))?;

    Ok(json!({
        "ok": true,
        "to": to,
        "subject": subject,
        "from": from,
        "server": format!("{}:{}", host, port),
    }))
}
