//! MCP stdio 客户端 —— 通过子进程 stdin/stdout 与 MCP server 通信。
//!
//! 设计说明：
//! - 同步阻塞 IO（子进程管道），调用方（工具 handler）负责放在合适线程；
//! - 客户端有 `&mut self` 语义（写入 stdin），跨线程共享需 `Mutex` 包裹；
//! - `spawn` 只做进程启动；协议握手由 `initialize` 完成。

use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

use super::protocol::{
    build_initialized_notification, build_initialize, build_tools_call, build_tools_list,
    decode_frames, encode_frame, extract_call_text, JsonRpcResponse, McpTool,
};

/// MCP server 进程配置。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// server 唯一名称（用于工具命名空间与去重）。
    pub name: String,
    /// 可执行程序（如 `npx` / `node` / 二进制路径）。
    pub command: String,
    /// 启动参数（如 `["@modelcontextprotocol/server-filesystem", "./"]`）。
    pub args: Vec<String>,
}

/// 已连接（spawn 成功）的 MCP 客户端。
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpClient {
    /// 启动 server 子进程（不做协议握手）。
    pub fn spawn(config: &McpServerConfig) -> Result<Self, String> {
        let mut cmd = Command::new(&config.command);
        crate::executors::builtin::terminal::hide_console(&mut cmd);
        let mut child = cmd
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("无法启动 MCP server `{}`: {}", config.command, e))?;

        let stdin = child.stdin.take().ok_or("无法获取 stdin")?;
        let stdout = child.stdout.take().ok_or("无法获取 stdout")?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    /// 协议握手：initialize + initialized 通知。
    pub fn initialize(&mut self, client_name: &str, client_version: &str) -> Result<(), String> {
        let req = build_initialize(self.next_id, client_name, client_version);
        self.next_id += 1;
        self.send(&req)?;

        // 等待 initialize 响应（可能先收到其它通知，忽略）
        let _resp = self.read_response()?;

        // 发送 initialized 通知
        let notify = build_initialized_notification();
        self.send(&notify)?;
        Ok(())
    }

    /// 拉取工具列表。
    pub fn list_tools(&mut self) -> Result<Vec<McpTool>, String> {
        let req = build_tools_list(self.next_id);
        self.next_id += 1;
        self.send(&req)?;
        let resp = self.read_response()?;
        let result = resp.result.ok_or_else(|| "tools/list 无 result".to_string())?;
        Ok(McpTool::parse_tools(&result))
    }

    /// 调用工具，返回提取后的文本结果。
    pub fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let req = build_tools_call(self.next_id, name, arguments);
        self.next_id += 1;
        self.send(&req)?;
        let resp = self.read_response()?;
        if let Some(err) = resp.error {
            return Err(format!("MCP 工具 `{}` 调用错误: {}", name, err));
        }
        let result = resp.result.unwrap_or_default();
        let text = extract_call_text(&result);
        Ok(serde_json::json!({ "text": text }))
    }

    /// 终止子进程。
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    // ── 内部：帧读写 ──

    fn send(&mut self, message: &str) -> Result<(), String> {
        let frame = encode_frame(message);
        self.stdin
            .write_all(&frame)
            .map_err(|e| format!("写入 MCP server 失败: {}", e))?;
        self.stdin.flush().map_err(|e| format!("刷新 MCP stdin 失败: {}", e))
    }

    /// 读取一帧响应（跳过通知帧，直到找到与请求 id 匹配的响应）。
    fn read_response(&mut self) -> Result<JsonRpcResponse, String> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            // 尝试从缓冲解析
            let (messages, consumed) = decode_frames(&buf);
            if consumed > 0 {
                buf.drain(..consumed);
            }
            for msg in messages {
                let value: Value = serde_json::from_str(&msg)
                    .map_err(|e| format!("解析 MCP 响应失败: {}", e))?;
                // 跳过通知（无 id）
                if value.get("id").is_none() {
                    continue;
                }
                let resp: JsonRpcResponse = serde_json::from_value(value)
                    .map_err(|e| format!("响应结构异常: {}", e))?;
                return Ok(resp);
            }
            // 需要更多数据：阻塞读块
            let mut chunk = [0u8; 8192];
            let n = self
                .stdout
                .read(&mut chunk)
                .map_err(|e| format!("读取 MCP server 失败: {}", e))?;
            if n == 0 {
                return Err("MCP server 进程已退出（EOF）".into());
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 离线静态测试：spawn 无效命令必须返回 Err（不触达真实 server）。
    #[test]
    fn spawn_invalid_command_returns_err() {
        let config = McpServerConfig {
            name: "ghost".into(),
            command: "definitely-not-a-real-cmd-xyz".into(),
            args: vec![],
        };
        let result = McpClient::spawn(&config);
        assert!(result.is_err());
    }
}
