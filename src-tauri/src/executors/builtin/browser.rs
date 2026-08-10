//! `builtin:browser` —— 浏览器自动化工具（对标大厂 Agent 的 Web 操作能力）。
//!
//! 提供 HTTP 请求（GET/POST）+ headless Chrome 页面操作（打开/截图/点击/填表），
//! 让你可以通过 AI 自动操作网页——查资料、填表单、签到、抢票、爬数据。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

// ═══════════════════════════════════════════
// browser_fetch —— HTTP GET/POST 网页内容
// ═══════════════════════════════════════════

fn http_fetch(def: &UnifiedToolDef, tool_id: &str) -> (UnifiedToolDef, ToolHandler) {
    let def = def.clone();
    let tid = tool_id.to_string();
    let handler: ToolHandler = Arc::new(move |args, _ctx| {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if url.is_empty() {
            return Box::pin(async { Ok(ToolResult::err("url 参数不能为空")) });
        }
        let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_uppercase();
        let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let headers_str = args.get("headers").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let tid = tid.clone();
        Box::pin(async move {
            match do_http_fetch(&url, &method, &body, &headers_str).await {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("[{}] {}", tid, e))),
            }
        })
    });
    (def, handler)
}

async fn do_http_fetch(url: &str, method: &str, body: &str, headers_str: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("ClawDesk/1.0 (Agent Browser)")
        // ★ IPv6 黑洞修复（2026-08-10）
        .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
        .build()
        .map_err(|e| format!("构建客户端失败: {e}"))?;

    // 解析自定义头部
    let mut req = match method {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        _ => client.get(url),
    };
    if !headers_str.is_empty() {
        for line in headers_str.lines() {
            if let Some((k, v)) = line.split_once(':') {
                req = req.header(k.trim(), v.trim());
            }
        }
    }
    if !body.is_empty() && (method == "POST" || method == "PUT") {
        req = req.header("Content-Type", "application/json").body(body.to_string());
    }

    let resp = req.send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status().as_u16();
    let ct_header = resp.headers().get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("").to_string();
    let text = resp.text().await.map_err(|e| format!("读取响应体失败: {e}"))?;

    // HTML 响应：提取文本摘要（去掉标签，最多 8000 字符）
    let body_display = if ct_header.contains("text/html") || text.trim_start().starts_with("<!") || text.trim_start().starts_with("<") {
        extract_html_text(&text)
    } else {
        text.chars().take(8000).collect::<String>()
    };

    Ok(json!({
        "status": status,
        "contentType": ct_header,
        "body": body_display,
        "rawLength": text.len(),
    }))
}

/// 极简 HTML → 纯文本提取（去掉标签，保留正文）。
fn extract_html_text(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if !in_tag && i + 6 < len && chars[i] == '<' {
            let tag_start = &html[i..];
            if tag_start.to_lowercase().starts_with("<script") {
                in_script = true; in_tag = true; i += 1; continue;
            }
            if tag_start.to_lowercase().starts_with("</script") {
                in_script = false; in_tag = true; i += 1; continue;
            }
            if tag_start.to_lowercase().starts_with("<style") {
                in_style = true; in_tag = true; i += 1; continue;
            }
            if tag_start.to_lowercase().starts_with("</style") {
                in_style = false; in_tag = true; i += 1; continue;
            }
        }
        if chars[i] == '<' { in_tag = true; i += 1; continue; }
        if chars[i] == '>' { in_tag = false; i += 1; continue; }
        if !in_tag && !in_script && !in_style {
            result.push(chars[i]);
        }
        i += 1;
    }
    // 实体解码（常见 HTML 实体）
    let s = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    // 压缩空白
    let condensed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    condensed.chars().take(6000).collect()
}

// ═══════════════════════════════════════════
// browser_screenshot —— headless Chrome 截图
// ═══════════════════════════════════════════

fn browser_screenshot_handler() -> ToolHandler {
    Arc::new(|args, _ctx| {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            return Box::pin(async { Ok(ToolResult::err("url 不能为空")) });
        }
        Box::pin(async move {
            match do_screenshot(&url, "") {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("截图失败: {e}"))),
            }
        })
    })
}

fn do_screenshot(url: &str, _selector: &str) -> Result<serde_json::Value, String> {
    use headless_chrome::{Browser, LaunchOptions};
    let opts = LaunchOptions {
        headless: true,
        window_size: Some((1280, 900)),
        sandbox: false,
        ..LaunchOptions::default()
    };

    let browser = Browser::new(opts).map_err(|e| format!("浏览器初始化失败: {e}（需要已安装 Chrome/Edge）"))?;
    let tab = browser.new_tab().map_err(|e| format!("创建标签页失败: {e}"))?;
    tab.navigate_to(url).map_err(|e| format!("导航失败: {e}"))?;
    tab.wait_until_navigated().map_err(|e| format!("等待加载失败: {e}"))?;
    std::thread::sleep(std::time::Duration::from_secs(2));

    let png = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None,
        None,
        true,
    ).map_err(|e| format!("截图失败: {e}"))?;

    // 保存到附件目录
    let dir = crate::executors::builtin::attachment::attach_dir()
        .map_err(|e| format!("获取附件目录失败: {e}"))?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("browser_screenshot_{}.png", ts));
    std::fs::write(&path, &png).map_err(|e| format!("保存截图失败: {e}"))?;

    Ok(json!({
        "saved": path.to_string_lossy().to_string(),
        "size": png.len(),
        "width": 1280,
        "height": 900,
    }))
}

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    // browser_fetch
    let def_fetch = UnifiedToolDef::new(
        "builtin",
        "browser_fetch",
        "抓取网页内容（HTTP GET/POST）。返回状态码、类型、文本摘要（自动提取 HTML 纯文本）。",
        vec![
            ToolParamDef {
                name: "url".into(), param_type: "string".into(),
                description: "网页 URL（http/https）".into(), required: true,
                enum_values: None, default: None,
            },
            ToolParamDef {
                name: "method".into(), param_type: "string".into(),
                description: "HTTP 方法：GET 或 POST，默认 GET".into(), required: false,
                enum_values: Some(vec!["GET".into(), "POST".into()]), default: Some(json!("GET")),
            },
            ToolParamDef {
                name: "body".into(), param_type: "string".into(),
                description: "POST 请求体（JSON 字符串）".into(), required: false,
                enum_values: None, default: None,
            },
            ToolParamDef {
                name: "headers".into(), param_type: "string".into(),
                description: "自定义 Header（每行 Key: Value）".into(), required: false,
                enum_values: None, default: None,
            },
        ],
    )?;
    let (def_fetch, handler_fetch) = http_fetch(&def_fetch, "browser_fetch");
    registry.register(def_fetch, handler_fetch)?;

    // browser_screenshot
    let def_shot = UnifiedToolDef::new(
        "builtin",
        "browser_screenshot",
        "用 headless Chrome 打开网页并全页截图（保存到附件目录）。",
        vec![ToolParamDef {
            name: "url".into(), param_type: "string".into(),
            description: "网页 URL".into(), required: true,
            enum_values: None, default: None,
        }],
    )?;
    registry.register(def_shot, browser_screenshot_handler())?;

    Ok(())
}
