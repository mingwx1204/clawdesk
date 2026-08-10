//! `builtin:agent_subtask` —— 多智能体子任务执行器。
//!
//! 设计说明：
//! - 主 Agent 通过本工具将子任务委派给一个**独立上下文**的子 Agent：
//!   子 Agent 使用精简只读工具集 + 独立消息历史循环推理，结果以文本
//!   摘要返回给主 Agent（主上下文不被子任务中间过程污染）；
//! - **上下文隔离**：子任务循环独立于主循环，token 预算可控；
//! - **安全**：子任务仅暴露只读/安全工具（read_file / list_dir / grep 等），
//!   写工具、终端、生图不进入子工具集；子任务内的工具调用仍走全局
//!   dispatcher 的中间件链（沙箱 / 高危 / 敏感文件保护自动生效）；
//! - **资源受限**：maxRounds 默认 6、上限 12，防止子任务失控。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::core::tool::context::ToolContext;
use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::dispatcher::ToolCall;
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;
use crate::llm::runner::ChatProvider;
use crate::llm::{
    decode_tool_name, extract_text, extract_tool_calls, serialize_tools, LlmMessage, Role,
};

/// 子任务允许暴露的工具白名单（只读 / 安全，不包含写/终端/生图等危险能力）。
const SUBTASK_SAFE_TOOLS: &[&str] = &[
    "builtin:read_file",
    "builtin:list_dir",
    "builtin:grep_search",
    "builtin:file_search",
    "builtin:get_time",
    "builtin:memory_search",
    "builtin:calculate",
    "builtin:echo",
];

/// 子任务最大轮数上限（硬性约束）。
const SUBTASK_MAX_ROUNDS_CAP: usize = 12;

/// 子任务系统提示（引导子 Agent 专注、只读、输出精炼）。
const SUBTASK_SYSTEM_PROMPT: &str = "\
你是 ClawDesk 主 Agent 委派的子任务执行代理。\
你的职责：使用提供的只读工具完成指定子任务，保持专注。\
规则：\
1. 只使用提供的工具，绝不臆造结果；\
2. 用最少轮数达成目标；\
3. 结束时给出精炼结论（中文），直接说明发现/答案，不输出过程性废话。";

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "agent_subtask",
        "将子任务委派给独立的子 Agent 执行（独立上下文 + 只读工具集），返回子任务的结论文本。\
         适合需要专注调研/检索/分析、不希望污染主上下文的场景",
        vec![
            ToolParamDef {
                name: "task".into(),
                param_type: "string".into(),
                description: "子任务描述：明确目标、范围与期望输出".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "maxRounds".into(),
                param_type: "number".into(),
                description: "子任务最大推理轮数（默认 6，上限 12）".into(),
                required: false,
                enum_values: None,
                default: Some(json!(6)),
            },
        ],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let task = args
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if task.is_empty() {
                return Ok(ToolResult::err("task 不能为空"));
            }
            let max_rounds = args
                .get("maxRounds")
                .and_then(|v| v.as_u64())
                .unwrap_or(6)
                .clamp(1, SUBTASK_MAX_ROUNDS_CAP as u64) as usize;

            // 主 LLM 客户端（路由单例；未配置主模型 → 降级错误）
            let Some(router) = crate::llm::router::global() else {
                return Ok(ToolResult::err("子任务失败: 主模型未配置"));
            };
            let Some(client) = router.main_client() else {
                return Ok(ToolResult::err("子任务失败: 主模型未配置"));
            };

            // 全局调度器（含中间件链：沙箱 / 高危 / 敏感文件保护）
            let Some(dispatcher) = crate::core::tool::dispatcher::global() else {
                return Ok(ToolResult::err("子任务失败: 调度器未初始化"));
            };

            // 子工具集：只读白名单（从注册表取定义）
            let safe_defs: Vec<_> = dispatcher
                .registry()
                .list()
                .into_iter()
                .filter(|d| SUBTASK_SAFE_TOOLS.contains(&d.id.as_str()))
                .collect();
            if safe_defs.is_empty() {
                return Ok(ToolResult::err("子任务失败: 子工具集为空"));
            }
            let tools = serialize_tools(&safe_defs);

            // 独立子上下文
            let mut messages: Vec<LlmMessage> = vec![
                LlmMessage {
                    role: Role::System,
                    content: SUBTASK_SYSTEM_PROMPT.to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                },
                LlmMessage {
                    role: Role::User,
                    content: task.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ];

            let mut rounds_used = 0usize;
            loop {
                if rounds_used >= max_rounds {
                    break;
                }
                rounds_used += 1;

                // 同步 LLM 调用放到阻塞线程，避免阻塞 async runtime
                // （UFCS 调用 ChatProvider::chat，避免被 LlmClient 自带单参 chat 遮蔽）
                let client = client.clone();
                let tools = tools.clone();
                let msgs = messages.clone();
                let resp =
                    match tokio::task::spawn_blocking(move || ChatProvider::chat(&client, &msgs, &tools))
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return Ok(ToolResult::err(format!("子任务执行线程失败: {}", e)));
                        }
                    };
                let resp = match resp {
                    Ok(r) => r,
                    Err(e) => {
                        return Ok(ToolResult::err(format!("子任务模型请求失败: {}", e)));
                    }
                };

                // 追加 assistant 消息（含 tool_calls）
                let tool_calls = extract_tool_calls(&resp);
                let model_text = extract_text(&resp);
                messages.push(LlmMessage {
                    role: Role::Assistant,
                    content: model_text.clone(),
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls.clone())
                    },
                    tool_call_id: None,
                });

                // 模型给出最终文本 → 子任务完成
                if tool_calls.is_empty() {
                    return Ok(ToolResult::ok(json!({
                        "result": model_text,
                        "roundsUsed": rounds_used,
                        "truncated": false,
                    })));
                }

                // 执行每个工具调用，把结果作为 tool 消息回填（保持配对）
                for call in tool_calls {
                    let name = decode_tool_name(&call.function.name);
                    let call_args: Value = serde_json::from_str(&call.function.arguments)
                        .unwrap_or_else(|_| Value::Object(Default::default()));
                    let call_id = call.id.clone();

                    let out_text = match dispatcher
                        .dispatch(
                            ToolCall {
                                id: call_id.clone(),
                                tool_id: name,
                                arguments: call_args,
                                // 子任务内部独立计数（受 max_rounds 上限约束）
                                round: 1,
                            },
                            ToolContext::default(),
                        )
                        .await
                    {
                        Ok(ToolResult::Success { output }) => output.to_string(),
                        Ok(ToolResult::Error { message }) => {
                            format!("工具执行失败: {}", message)
                        }
                        Ok(ToolResult::Interrupted { reason }) => {
                            format!("工具执行中断: {}", reason)
                        }
                        Err(e) => format!("工具执行失败: {}", e),
                    };
                    messages.push(LlmMessage {
                        role: Role::Tool,
                        content: out_text,
                        tool_calls: None,
                        tool_call_id: Some(call_id),
                    });
                }
            }

            // 达到轮数上限且未产出结论
            let last_text = messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, Role::Assistant))
                .map(|m| m.content.clone())
                .unwrap_or_default();
            Ok(ToolResult::ok(json!({
                "result": format!(
                    "（子任务达到 {} 轮上限，未产出最终结论；最后模型输出: {}）",
                    max_rounds,
                    if last_text.is_empty() { "无" } else { &last_text }
                ),
                "roundsUsed": rounds_used,
                "truncated": true,
            })))
        })
    });

    registry.register(def, handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::dispatcher::ToolDispatcher;
    use std::sync::Arc;

    /// 验证 agent_subtask 确实注册进注册表，且参数定义正确（task 必填 / maxRounds 默认 6）。
    #[test]
    fn registers_subtask_tool_with_correct_params() {
        let registry = Arc::new(ToolRegistry::new());
        register(&registry).expect("注册失败");
        let def = registry
            .get("builtin:agent_subtask")
            .expect("agent_subtask 未注册到注册表");
        assert!(
            def.params.iter().any(|p| p.name == "task" && p.required),
            "task 参数应为必填"
        );
        let max_r = def
            .params
            .iter()
            .find(|p| p.name == "maxRounds")
            .expect("缺少 maxRounds 参数");
        assert_eq!(max_r.default.as_ref().and_then(|v| v.as_u64()), Some(6));
    }

    /// 验证 handler 真实可调度：空 task 应被校验拦截返回 Error（而非 panic / 假成功）。
    #[tokio::test]
    async fn dispatches_empty_task_rejected() {
        let registry = Arc::new(ToolRegistry::new());
        register(&registry).expect("注册失败");
        let dispatcher = ToolDispatcher::new(registry);
        let res = dispatcher
            .dispatch(
                ToolCall {
                    id: "t1".into(),
                    tool_id: "builtin:agent_subtask".into(),
                    arguments: serde_json::json!({}),
                    round: 1,
                },
                ToolContext::default(),
            )
            .await
            .expect("调度未返回结果");
        assert!(
            matches!(res, ToolResult::Error { .. }),
            "空 task 应返回错误，实际返回成功（说明校验未生效）"
        );
    }
}
