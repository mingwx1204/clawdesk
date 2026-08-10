//! MCP JSON-RPC 2.0 协议层 —— 纯函数，无 IO，可离线单测。
//!
//! MCP（Model Context Protocol）基于 JSON-RPC 2.0，stdio transport
//! 使用 `Content-Length` 头帧封装（与 LSP 相同）：
//! ```text
//! Content-Length: <N>\r\n\r\n<JSON 消息体（N 字节）>
//! ```

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// JSON-RPC 2.0 版本号。
pub const JSONRPC_VERSION: &str = "2.0";

// ────────────────────────────────────────────────
// 消息类型
// ────────────────────────────────────────────────

/// JSON-RPC 请求（带 id，需响应）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC 通知（无 id，不需响应）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

// ────────────────────────────────────────────────
// 帧编解码
// ────────────────────────────────────────────────

/// 将 JSON 文本封装为 stdio 帧（`Content-Length` 头 + 消息体）。
pub fn encode_frame(message: &str) -> Vec<u8> {
    let body = message.as_bytes();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut out = Vec::with_capacity(header.len() + body.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body);
    out
}

/// 从缓冲中解析出全部完整帧，返回 (解析出的 JSON 文本, 已消费字节数)。
///
/// 设计说明：stdio 管道可能一次到达多个帧或半个帧，
/// 调用方需将剩余字节保留在缓冲中供下次解析。
pub fn decode_frames(buf: &[u8]) -> (Vec<String>, usize) {
    let mut messages = Vec::new();
    let mut pos = 0usize;
    let len = buf.len();

    loop {
        // 找头部结束位置 \r\n\r\n
        let header_end = find_subslice(&buf[pos..len], b"\r\n\r\n");
        let Some(header_end) = header_end else {
            break; // 头部不完整，等待更多数据
        };
        let header_abs_end = pos + header_end + 4;

        // 解析 Content-Length
        let header_text = std::str::from_utf8(&buf[pos..pos + header_end]).unwrap_or("");
        let Some(content_length) = parse_content_length(header_text) else {
            break; // 头部格式异常，放弃该帧
        };

        // 检查消息体是否完整
        if header_abs_end + content_length > len {
            break; // 消息体不完整，等待更多数据
        }

        let body = &buf[header_abs_end..header_abs_end + content_length];
        let text = String::from_utf8_lossy(body).into_owned();
        messages.push(text);
        pos = header_abs_end + content_length;
    }

    (messages, pos)
}

/// 查找子串位置（朴素匹配，帧头短小足够用）。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// 从帧头文本解析 Content-Length 值。
fn parse_content_length(header: &str) -> Option<usize> {
    header.lines().find_map(|line| {
        let lower = line.trim().to_ascii_lowercase();
        lower
            .strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    })
}

// ────────────────────────────────────────────────
// MCP 方法消息构造
// ────────────────────────────────────────────────

/// 构造 `initialize` 请求。
pub fn build_initialize(id: u64, client_name: &str, client_version: &str) -> String {
    let msg = JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        method: "initialize".into(),
        params: json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": client_name, "version": client_version }
        }),
    };
    serde_json::to_string(&msg).unwrap()
}

/// 构造 `notifications/initialized` 通知。
pub fn build_initialized_notification() -> String {
    let msg = JsonRpcNotification {
        jsonrpc: JSONRPC_VERSION.into(),
        method: "notifications/initialized".into(),
        params: json!({}),
    };
    serde_json::to_string(&msg).unwrap()
}

/// 构造 `tools/list` 请求。
pub fn build_tools_list(id: u64) -> String {
    let msg = JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        method: "tools/list".into(),
        params: json!({}),
    };
    serde_json::to_string(&msg).unwrap()
}

/// 构造 `tools/call` 请求。
pub fn build_tools_call(id: u64, name: &str, arguments: &Value) -> String {
    let msg = JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        method: "tools/call".into(),
        params: json!({
            "name": name,
            "arguments": arguments,
        }),
    };
    serde_json::to_string(&msg).unwrap()
}

// ────────────────────────────────────────────────
// MCP 工具定义（tools/list 返回的 schema 子集）
// ────────────────────────────────────────────────

/// MCP 工具定义（仅解析我们关心的字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

impl McpTool {
    /// 从 `tools/list` 结果中提取工具数组。
    pub fn parse_tools(result: &Value) -> Vec<McpTool> {
        result
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| serde_json::from_value::<McpTool>(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// 解析 `tools/call` 结果，提取结构化文本内容。
///
/// MCP 约定：content 数组元素为 `{type: "text", text: "..."}`。
pub fn extract_call_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| result.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_then_decode_roundtrip() {
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let frame = encode_frame(msg);
        // 动态断言：Content-Length 必须等于消息体字节数
        let expected_header = format!("Content-Length: {}\r\n\r\n", msg.len());
        assert!(frame.starts_with(expected_header.as_bytes()));

        let (messages, consumed) = decode_frames(&frame);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], msg);
        assert_eq!(consumed, frame.len());
    }

    #[test]
    fn decode_multiple_frames_in_one_buffer() {
        let msg1 = r#"{"jsonrpc":"2.0","id":1,"method":"a"}"#;
        let msg2 = r#"{"jsonrpc":"2.0","id":2,"method":"b"}"#;
        let mut buf = encode_frame(msg1);
        buf.extend_from_slice(&encode_frame(msg2));

        let (messages, consumed) = decode_frames(&buf);
        assert_eq!(messages.len(), 2);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn decode_partial_frame_keeps_remainder() {
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let frame = encode_frame(msg);
        // 只给一半数据
        let half = &frame[..frame.len() - 10];
        let (messages, consumed) = decode_frames(half);
        assert!(messages.is_empty());
        assert_eq!(consumed, 0);
    }

    #[test]
    fn build_messages_produce_valid_json() {
        let req = build_initialize(1, "clawdesk", "0.1.0");
        let v: Value = serde_json::from_str(&req).unwrap();
        assert_eq!(v["method"], "initialize");
        assert_eq!(v["id"], 1);

        let call = build_tools_call(2, "echo", &json!({"x": 1}));
        let v: Value = serde_json::from_str(&call).unwrap();
        assert_eq!(v["params"]["name"], "echo");
    }

    #[test]
    fn parse_tools_extracts_schema() {
        let result = json!({
            "tools": [
                {
                    "name": "fs_read",
                    "description": "读取文件",
                    "input_schema": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }
                }
            ]
        });
        let tools = McpTool::parse_tools(&result);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "fs_read");
    }

    #[test]
    fn extract_text_content() {
        let result = json!({
            "content": [
                { "type": "text", "text": "第一行" },
                { "type": "text", "text": "第二行" }
            ]
        });
        assert_eq!(extract_call_text(&result), "第一行\n第二行");
    }
}
