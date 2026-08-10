//! `builtin:web_search` —— 联网搜索工具（Bing RSS，无需 API Key）。
//!
//! 对标大厂 AI 客户端的联网能力：模型需要实时/外部信息时调用本工具，
//! 返回最多 8 条搜索结果（标题 / 链接 / 摘要），模型据此回答。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const MAX_RESULTS: usize = 8;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "web_search",
        "联网搜索互联网（Bing），返回最多 8 条结果（标题/链接/摘要）。\
         适合查询实时新闻、最新信息、文档、技术资料等模型知识之外的内容",
        vec![ToolParamDef {
            name: "query".into(),
            param_type: "string".into(),
            description: "搜索关键词（尽量具体，可加引号精确匹配）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if query.is_empty() {
                return Ok(ToolResult::err("query 不能为空"));
            }
            let url = format!(
                "https://www.bing.com/search?q={}&format=rss&count={}",
                urlencode(&query),
                MAX_RESULTS
            );
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                // ★ IPv6 黑洞修复（2026-08-10）
                .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
                .build()
            {
                Ok(c) => c,
                Err(e) => return Ok(ToolResult::err(format!("构建搜索客户端失败: {e}"))),
            };
            let resp = match client
                .get(&url)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36")
                .header("Accept", "application/rss+xml, application/xml, text/xml, */*")
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => return Ok(ToolResult::err(format!("搜索请求失败: {e}"))),
            };
            if !resp.status().is_success() {
                return Ok(ToolResult::err(format!(
                    "搜索服务返回 HTTP {}",
                    resp.status()
                )));
            }
            let body = resp.text().await.unwrap_or_default();
            let results = parse_rss_items(&body);
            if results.is_empty() {
                return Ok(ToolResult::ok(json!({
                    "query": query,
                    "count": 0,
                    "results": [],
                    "note": "未找到相关结果，可尝试更换关键词"
                })));
            }
            Ok(ToolResult::ok(json!({
                "query": query,
                "count": results.len(),
                "results": results,
            })))
        })
    });

    registry.register(def, handler)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 解析 Bing RSS：提取全部 <item> 的 title / link / description。
fn parse_rss_items(xml: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<item>") {
        let body_start = start + "<item>".len();
        let Some(end_rel) = rest[body_start..].find("</item>") else {
            break;
        };
        let body = &rest[body_start..body_start + end_rel];
        rest = &rest[body_start + end_rel + "</item>".len()..];

        let title = extract_tag(body, "title");
        let link = extract_tag(body, "link");
        let desc = strip_html(&extract_tag(body, "description"));
        if title.is_empty() && link.is_empty() {
            continue;
        }
        out.push(json!({
            "title": title,
            "url": link,
            "snippet": truncate(&desc, 400),
        }));
        if out.len() >= MAX_RESULTS {
            break;
        }
    }
    out
}

/// 提取单个 XML 标签内容（含 HTML 反转义）。
fn extract_tag(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(s) = body.find(&open) {
        let from = s + open.len();
        if let Some(e) = body[from..].find(&close) {
            return html_unescape(&body[from..from + e]);
        }
    }
    String::new()
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// 去除 HTML 标签，保留文本。
fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{}…", cut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bing_rss() {
        let xml = r#"<rss><channel>
<item><title>ClawDesk &amp; AI</title><link>https://example.com/a</link><description>&lt;p&gt;桌面 AI 助手&lt;/p&gt;</description></item>
<item><title>第二</title><link>https://example.com/b</link><description>摘要二</description></item>
</channel></rss>"#;
        let items = parse_rss_items(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["title"], "ClawDesk & AI");
        assert_eq!(items[0]["url"], "https://example.com/a");
        assert_eq!(items[0]["snippet"], "桌面 AI 助手");
    }
}
