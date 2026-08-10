//! MCP 适配器 —— 管理外部 MCP server 连接，将远端工具注册进统一注册表。
//!
//! 流程（DEV_SPEC.md §4.2）：
//! 1. `add_server(config)` 登记 server 配置；
//! 2. `register_tools(registry)` 对每个 server：spawn → initialize →
//!    tools/list → 转换为 UnifiedToolDef（source: "mcp"）注册；
//! 3. 调用时 handler 转发 `tools/call`，返回文本结果。

pub mod client;
pub mod protocol;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

use self::client::{McpClient, McpServerConfig};
use self::protocol::McpTool;

/// 适配器固定来源标识（动态注册表的 source 值之一）。
pub const SOURCE: &str = "mcp";

/// MCP 适配器：server 配置 + 活动连接的管理器。
#[derive(Default)]
pub struct McpAdapter {
    servers: RwLock<Vec<McpServerConfig>>,
    /// server name → 已连接客户端（Mutex 包裹以满足 handler Fn 约束）。
    clients: RwLock<HashMap<String, Arc<Mutex<McpClient>>>>,
}

impl McpAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个 server 配置（name 重复则拒绝）。
    pub fn add_server(&self, config: McpServerConfig) -> Result<(), ToolError> {
        if config.name.trim().is_empty() || config.command.trim().is_empty() {
            return Err(ToolError::invalid_def("MCP server 的 name 与 command 不能为空"));
        }
        let mut servers = self.servers.write().unwrap();
        if servers.iter().any(|s| s.name == config.name) {
            return Err(ToolError::already_registered(&format!("mcp:{}", config.name)));
        }
        servers.push(config);
        Ok(())
    }

    /// 列出已登记 server。
    pub fn list_servers(&self) -> Vec<McpServerConfig> {
        self.servers.read().unwrap().clone()
    }

    /// 移除一个 server 配置并断开其客户端连接（返回是否找到）。
    pub fn remove_server(&self, name: &str) -> bool {
        let mut servers = self.servers.write().unwrap();
        let before = servers.len();
        servers.retain(|s| s.name != name);
        let removed = servers.len() != before;
        if removed {
            // 断开并移除活动客户端（Drop 时终止子进程）
            self.clients.write().unwrap().remove(name);
        }
        removed
    }

    /// 连接全部已登记 server 并将工具注册进 registry。
    ///
    /// 返回注册的工具数量。单个 server 失败不影响其它 server
    /// （失败记录在日志中），已连接部分照常注册。
    pub fn register_tools(&self, registry: &ToolRegistry) -> Result<usize, ToolError> {
        let servers = self.servers.read().unwrap().clone();
        let mut total = 0usize;

        for config in &servers {
            let mut client = match McpClient::spawn(config) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[MCP] 连接 `{}` 失败: {}", config.name, e);
                    continue;
                }
            };
            if let Err(e) = client.initialize("clawdesk", env!("CARGO_PKG_VERSION")) {
                eprintln!("[MCP] 初始化 `{}` 失败: {}", config.name, e);
                continue;
            }
            let tools = match client.list_tools() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[MCP] `{}` tools/list 失败: {}", config.name, e);
                    continue;
                }
            };

            let shared = Arc::new(Mutex::new(client));
            let registered = self.register_tools_from(config, &tools, shared, registry)?;
            total += registered;
        }

        Ok(total)
    }

    /// 将单个 server 的工具列表转换为 UnifiedToolDef 并注册。
    fn register_tools_from(
        &self,
        config: &McpServerConfig,
        tools: &[McpTool],
        shared: Arc<Mutex<McpClient>>,
        registry: &ToolRegistry,
    ) -> Result<usize, ToolError> {
        let mut count = 0usize;
        for tool in tools {
            let tool_id = format!("{}.{}", config.name, tool.name);
            let params = schema_to_params(&tool.input_schema);

            let description = if tool.description.is_empty() {
                format!("MCP 工具（server: {}）", config.name)
            } else {
                tool.description.clone()
            };
            let def = UnifiedToolDef::new(
                SOURCE,
                &tool_id,
                &description,
                params,
            )?
            .with_metadata("server", serde_json::json!(config.name))
            .with_metadata("mcp_tool", serde_json::json!(tool.name));

            // handler 转发 tools/call；阻塞 IO 在 async handler 内执行
            let shared_for_handler = shared.clone();
            let tool_name = tool.name.clone();
            let server_name = config.name.clone();
            let handler: ToolHandler = Arc::new(move |args, _ctx| {
                let shared = shared_for_handler.clone();
                let tool_name = tool_name.clone();
                let server_name = server_name.clone();
                Box::pin(async move {
                    match shared.lock().unwrap().call_tool(&tool_name, &args) {
                        Ok(value) => Ok(ToolResult::ok(value)),
                        Err(e) => Ok(ToolResult::err(format!("MCP `{}`: {}", server_name, e))),
                    }
                })
            });

            registry.register(def, handler)?;
            count += 1;
        }
        self.clients.write().unwrap().insert(config.name.clone(), shared);
        Ok(count)
    }
}

/// 将 MCP 的 JSON Schema 子集转换为 ToolParamDef 列表。
fn schema_to_params(schema: &serde_json::Value) -> Vec<ToolParamDef> {
    let mut params = Vec::new();
    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return params;
    };

    let required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for (name, prop) in properties {
        let param_type = prop
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("string")
            .to_string();
        let description = prop
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .to_string();
        let is_required = required.contains(&name.as_str());

        params.push(ToolParamDef {
            name: name.clone(),
            param_type,
            description,
            required: is_required,
            enum_values: None,
            default: None,
        });
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_to_params_parses_required() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文件路径" },
                "recursive": { "type": "boolean" }
            },
            "required": ["path"]
        });
        let params = schema_to_params(&schema);
        assert_eq!(params.len(), 2);
        let path = params.iter().find(|p| p.name == "path").unwrap();
        assert!(path.required);
        assert_eq!(path.param_type, "string");
        let recursive = params.iter().find(|p| p.name == "recursive").unwrap();
        assert!(!recursive.required);
    }

    #[test]
    fn empty_schema_yields_no_params() {
        assert!(schema_to_params(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn add_server_rejects_duplicate_name() {
        let adapter = McpAdapter::new();
        let config = McpServerConfig {
            name: "fs".into(),
            command: "npx".into(),
            args: vec![],
        };
        adapter.add_server(config.clone()).unwrap();
        let err = adapter.add_server(config).unwrap_err();
        assert_eq!(err.kind, crate::core::tool::error::ToolErrorKind::AlreadyRegistered);
    }

    /// 离线静态：空命令配置被拒绝（不触达外部进程）。
    #[test]
    fn add_server_rejects_empty_command() {
        let adapter = McpAdapter::new();
        let config = McpServerConfig {
            name: "bad".into(),
            command: "".into(),
            args: vec![],
        };
        assert!(adapter.add_server(config).is_err());
    }

    /// 离线静态：register_tools 遇到不可启动 server 时静默跳过，
    /// 不 panic、不阻断（返回注册数量 0）。
    #[test]
    fn register_tools_skips_unreachable_server() {
        let adapter = McpAdapter::new();
        let config = McpServerConfig {
            name: "ghost".into(),
            command: "definitely-not-a-real-cmd-xyz".into(),
            args: vec![],
        };
        adapter.add_server(config).unwrap();

        let registry = crate::core::tool::registry::ToolRegistry::new();
        let count = adapter.register_tools(&registry).unwrap();
        assert_eq!(count, 0);
        assert!(registry.is_empty());
    }
}
