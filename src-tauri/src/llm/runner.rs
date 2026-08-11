//! Agent 执行内核（方案B替换版）—— CodeWhale 引擎驱动。
//!
//! 与旧版差异：
//! - 模型请求从「ureq 同步非流式」替换为「reqwest HTTP/1.1 SSE 流式」；
//! - 上下文压缩从「阻塞内联」替换为「独立超时 120s + 后台协程 + 降级截断」；
//! - 调度从「单循环无守卫」替换为「独立 ENGINE_RT + 600s 看门狗 + 心跳 abort + 工具 480s timeout」。
//!
//! 对外契约不变：`run_agent_loop(...) -> Result<ToolLoopOutcome, String>`。
//! 引擎配置经 `harness::engine::config::engine_config()` 读取（harness_set_model_config 写入）。

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::core::tool::context::ToolContext;
use crate::core::tool::dispatcher::{ToolCall, ToolDispatcher};
use crate::core::tool::registry::ToolRegistry;
use crate::core::tool::result::ToolResult;

use super::planner::{build_plan_prompt, parse_plan};
use super::progress::{
    AgentProgress, CancelRegistry, CancellationToken, ProgressSink, ToolCallProgress,
};
use super::session::SessionManager;
use super::{extract_text, extract_usage, AgentMode, ChatResponse, LlmMessage, Role};
use super::Usage;
use crate::middleware::risk::RiskLevel;

use crate::harness::core::turn_loop::{
    self, EngineEvent, ToolExecutor, TurnConfig, TurnResult, ENGINE_RT,
};
use crate::harness::engine::client::LlmClient;
use crate::harness::engine::config::{EngineConfig, engine_config};
use crate::harness::engine::context::ContextManager;
use crate::harness::engine::param::ModelParams;

// ─────────────────────────────────────────────
// 保留：模型调用抽象（真实实现：LlmClient/router；测试：mock）
// ─────────────────────────────────────────────

/// 模型调用抽象（真实实现：LlmClient；测试：mock）。
pub trait ChatProvider: Send + Sync {
    fn chat(&self, messages: &[LlmMessage], tools: &Value) -> Result<ChatResponse, String>;
}

/// 单轮模型调用记录（供前端展示调用过程）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundRecord {
    pub round: usize,
    pub model_text: String,
    pub tool_calls: Vec<ToolCallRecord>,
    /// 本轮模型调用 Token 用量（OpenAI 兼容 usage；缺失时为 None）。
    pub usage: Option<Usage>,
}

/// 单次工具调用记录。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    pub tool_id: String,
    pub arguments: Value,
    pub status: String,
    pub output: Value,
}

/// 循环最终结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLoopOutcome {
    pub rounds: Vec<RoundRecord>,
    pub final_text: String,
    pub truncated: bool,
    pub used_rounds: usize,
    /// 全部轮次累计 Token 用量。
    pub usage: Usage,
}

// ─────────────────────────────────────────────
// 引擎桥接：现有 ToolDispatcher → ToolExecutor
// ─────────────────────────────────────────────

/// 确认回调：StepConfirm 模式下每个工具执行前调用；返回 true 放行。
/// 参数：(call_id, tool_id, arguments)。
pub type ConfirmFn = Arc<dyn Fn(&str, &str, &Value) -> bool + Send + Sync>;

/// 桥接执行器：把现有 ToolDispatcher 包装为引擎 ToolExecutor。
pub struct DispatcherExecutor {
    /// 工具注册表（引擎确认回调查 def 用）。
    pub registry: Arc<ToolRegistry>,
    /// 工具调度器（含沙箱/审计/风险中间件链）。
    pub dispatcher: Arc<ToolDispatcher>,
    /// 轮次计数器。
    pub round: AtomicUsize,
    /// 确认回调（StepConfirm 模式挂载）。
    pub confirm: Option<ConfirmFn>,
}

#[async_trait::async_trait]
impl ToolExecutor for DispatcherExecutor {
    async fn execute(&self, name: &str, arguments: Value) -> anyhow::Result<String> {
        // 工具名解码：OpenAI 函数名编码形式 `builtin__get_time` → `builtin:get_time`
        let tool_id = crate::llm::decode_tool_name(name);

        // 轮次递增：fetch_add 返回旧值，+1 使首轮从 1 开始（优化4）
        let round = self.round.fetch_add(1, AtomicOrdering::SeqCst) + 1;

        // ── 权限确认（StepConfirm 模式）──
        if let Some(confirm) = &self.confirm {
            let call_id = uuid::Uuid::new_v4().to_string();
            let approved = confirm(&call_id, &tool_id, &arguments);
            if !approved {
                anyhow::bail!("用户拒绝执行该工具");
            }
        }

        // ── 沙箱/中间件链在 dispatcher.dispatch 内部自动生效 ──
        let call = ToolCall {
            id: uuid::Uuid::new_v4().to_string(),
            tool_id: tool_id.clone(),
            arguments: arguments.clone(),
            round,
        };
        let ctx = ToolContext {
            round,
            session_id: None,
            timeout_secs: None,
            data: Default::default(),
        };

        match self.dispatcher.dispatch(call, ctx).await {
            Ok(ToolResult::Success { output }) => {
                Ok(serde_json::to_string(&output).unwrap_or_else(|_| "{}".into()))
            }
            Ok(ToolResult::Error { message }) => anyhow::bail!(message),
            Ok(ToolResult::Interrupted { reason }) => anyhow::bail!(reason),
            Err(e) => anyhow::bail!(e.to_string()),
        }
    }
}

// ─────────────────────────────────────────────
// 主入口：完整 Agent 循环（签名与旧版一致）
// ─────────────────────────────────────────────

/// 取消时返回的统一结果。
fn agent_cancelled_outcome() -> ToolLoopOutcome {
    ToolLoopOutcome {
        used_rounds: 0,
        rounds: Vec::new(),
        final_text: "⏹ 任务已取消".into(),
        truncated: false,
        usage: Usage::default(),
    }
}

/// 将会话真实 token 用量累计到会话（上下文占用面板数据源）。
fn record_usage(sessions: &SessionManager, session_id: &str, usage: &Usage) {
    let mut session = sessions.get_or_create(session_id);
    session.total_input_tokens += usage.prompt_tokens;
    session.total_output_tokens += usage.completion_tokens;
    session.last_input_tokens = usage.prompt_tokens;
    sessions.update(session);
}

/// 完整 Agent 循环（方案B替换版）—— 签名不变，内部走 CodeWhale 引擎。
pub async fn run_agent_loop(
    provider: &Arc<dyn ChatProvider>,
    registry: &ToolRegistry,
    sandbox: &crate::middleware::sandbox::SandboxManager,
    dispatcher: &ToolDispatcher,
    sessions: &SessionManager,
    confirms: &CancelRegistry,
    session_id: &str,
    user_prompt: &str,
    max_rounds: usize,
    mode: AgentMode,
    resume: bool,
    timeout_secs: u64,
    progress: &ProgressSink,
    cancel: &CancellationToken,
    // 额外系统提示（如微信人设），注入到系统 prompt 末尾；None 则不注入
    extra_system_prompt: Option<&str>,
) -> Result<ToolLoopOutcome, String> {
    // ★ 2026-08-12 修复：resume / confirms 参数在引擎路径下不生效，
    //   保留签名兼容但不再假装使用（resume 断点续跑、confirms 旧确认通道
    //   均为遗留接口，实际确认走 PERMISSION_BRIDGE）。timeout_secs 已生效（见下）。
    let _ = (sandbox, resume, confirms);

    // ── Off 模式：直通 LLM，不调用工具（保留旧行为）──
    if matches!(mode, AgentMode::Off) {
        if cancel.is_cancelled() {
            return Ok(agent_cancelled_outcome());
        }
        // ★ 自定义端点（OpenCode Go 等 OpenAI 兼容端点）：Off 模式也走 chat/completions 引擎直答，
        //   这样不开启 Agent 也能用配置的自定义模型（如 deepseek-v4-flash @ opencode）。
        if let Some(cfg) = engine_config() {
            if !cfg.base_url.eq_ignore_ascii_case("https://api.deepseek.com") {
                let client = LlmClient::new(cfg.api_key.clone(), cfg.base_url.clone())
                    .map_err(|e| format!("构建引擎客户端失败: {e}"))?;
                let params = ModelParams {
                    model: cfg.model.clone(),
                    reasoning_effort: cfg.effort,
                    ..Default::default()
                };
                let msgs = vec![serde_json::json!({ "role": "user", "content": user_prompt })];
                let text = client
                    .chat_once(&params, &msgs, None)
                    .await
                    .map_err(|e| e.to_string())?;
                let usage = Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                };
                record_usage(sessions, session_id, &usage);
                eprintln!("[RUNNER] Off 模式走自定义端点: model={}", cfg.model);
                return Ok(ToolLoopOutcome {
                    used_rounds: 0,
                    rounds: Vec::new(),
                    final_text: text,
                    truncated: false,
                    usage,
                });
            }
        }
        // ★ 同步 HTTP 调用（ureq）放到阻塞线程，避免卡住 async runtime（2026-08-12）
        let provider_c = provider.clone();
        let msgs_c = vec![LlmMessage {
            role: Role::User,
            content: user_prompt.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];
        let tools_c = Value::Array(vec![]);
        let resp = tokio::task::spawn_blocking(move || provider_c.chat(&msgs_c, &tools_c))
            .await
            .map_err(|e| format!("模型请求线程失败: {e}"))?
            .map_err(|e| e)?;
        let usage = extract_usage(&resp);
        record_usage(sessions, session_id, &usage);
        return Ok(ToolLoopOutcome {
            used_rounds: 0,
            rounds: Vec::new(),
            final_text: extract_text(&resp),
            truncated: false,
            usage,
        });
    }

    // ── PlanOnly：只输出计划，不执行工具（保留旧行为）──
    if matches!(mode, AgentMode::PlanOnly) {
        if cancel.is_cancelled() {
            return Ok(agent_cancelled_outcome());
        }
        // ★ 同步 HTTP 调用（ureq）放到阻塞线程，避免卡住 async runtime（2026-08-12）
        let provider_c = provider.clone();
        let msgs_c = vec![LlmMessage {
            role: Role::User,
            content: build_plan_prompt(user_prompt),
            tool_calls: None,
            tool_call_id: None,
        }];
        let tools_c = Value::Array(vec![]);
        let plan_resp = tokio::task::spawn_blocking(move || provider_c.chat(&msgs_c, &tools_c))
            .await
            .map_err(|e| format!("模型请求线程失败: {e}"))?
            .map_err(|e| e)?;
        let steps = parse_plan(&extract_text(&plan_resp));
        let plan_text = if steps.is_empty() {
            extract_text(&plan_resp)
        } else {
            format!(
                "📋 执行计划：\n{}",
                steps
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("{}. {}", i + 1, s))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        progress(&AgentProgress::ModelText {
            round: 0,
            text: plan_text.clone(),
        });
        let usage = extract_usage(&plan_resp);
        record_usage(sessions, session_id, &usage);
        return Ok(ToolLoopOutcome {
            used_rounds: 0,
            rounds: Vec::new(),
            final_text: plan_text,
            truncated: false,
            usage,
        });
    }

    // ── StepConfirm / Yolo：引擎驱动 ──

    // 读取引擎配置（harness_set_model_config 写入；agent_chat 命令层也会同步 key）
    let cfg: EngineConfig = engine_config().ok_or_else(|| {
        "引擎未配置：请先调用 harness_set_model_config 设置 API Key/Base URL/Effort".to_string()
    })?;

    let client = LlmClient::new(cfg.api_key.clone(), cfg.base_url.clone())
        .map_err(|e| format!("构建引擎客户端失败: {e}"))?;
    let ctx_mgr = Arc::new(ContextManager::new(Some(client.clone())));

    // ── 事件通道（确认闭包与进度转发共用；'static Sender 可被闭包捕获）──
    let (tx_event, mut rx_event) = mpsc::channel::<EngineEvent>(256);

    // ── 确认回调（优化3：抽取为公共函数 make_step_confirm_callback，传 Some(tx_event) 保持 ConfirmRequired 事件）──
    let confirm: Option<ConfirmFn> = if matches!(mode, AgentMode::StepConfirm) {
        Some(crate::harness::hooks::bridge::make_step_confirm_callback(
            Arc::new((*registry).clone()),
            Some(tx_event.clone()),
        ))
    } else {
        None
    };

    let executor: Arc<dyn ToolExecutor> = Arc::new(DispatcherExecutor {
        registry: Arc::new((*registry).clone()),
        dispatcher: Arc::new((*dispatcher).clone()),
        round: AtomicUsize::new(0),
        confirm,
    });

    // ── 取消桥接：旧 AtomicBool 取消 → tokio_util CancellationToken ──
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_flag = cancel.clone();
    let cancel_token_watch = cancel_token.clone();
    let watch = tokio::spawn(async move {
        loop {
            if cancel_flag.is_cancelled() {
                cancel_token_watch.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    // ── 会话记忆 ──
    let mut session = sessions.get_or_create(session_id);
    // 历史清理：
    // 1. 跳过"空且无工具调用"的 assistant 消息（失败/中断残留）；
    // 2. ★ token 预算截取（对齐 AstrBot 上下文管理思路）：从最近消息往前逐条累积
    //    token 估算（字符数/3 粗估，中文≈1 token/字、英文≈4 字/token 的折中），
    //    直到预算用尽。相比固定 12 条：短消息（微信聊天）保留更多轮，长消息
    //    （工具输出/长文档）不撑爆上下文——微信长聊的上下文更充分且不越界。
    //    预算默认 32K token（DeepSeek 上下文 64K/128K 的 1/2~1/4，留足回复与工具空间）。
    // 注意：带 tool_calls 的空 assistant（工具调用回合）必须保留，否则 tool 消息格式错误。
    const HISTORY_TOKEN_BUDGET: usize = 32_000;
    const MAX_HISTORY_MSGS: usize = 60; // 消息条数兜底（极端短消息场景）
    let mut budget_used: usize = 0;
    let mut kept: Vec<&crate::llm::LlmMessage> = Vec::new();
    for m in session.messages.iter().rev() {
        if matches!(m.role, Role::Assistant) {
            let c = m.content.trim();
            if c.is_empty() && m.tool_calls.is_none() {
                continue; // 跳过失败/中断残留
            }
        }
        // 粗估 token 数（中文场景下字符数/3 略保守，避免超预算）
        let est = m.content.chars().count() / 3 + 16;
        if !kept.is_empty() && (budget_used + est > HISTORY_TOKEN_BUDGET || kept.len() >= MAX_HISTORY_MSGS)
        {
            break; // 已有一条以上时预算超限 → 停止收更早的消息
        }
        budget_used += est;
        kept.push(m);
    }
    let mut messages: Vec<Value> = kept
        .into_iter()
        .rev()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let mut msg_json = serde_json::json!({ "role": role, "content": m.content });
            // ★ 保留 tool_calls / tool_call_id（否则 assistant(无tool_calls)+tool消息 格式错误 → 模型输出残缺）
            if let Some(tc) = &m.tool_calls {
                if let Ok(v) = serde_json::to_value(tc) {
                    msg_json["tool_calls"] = v;
                }
            }
            if let Some(tci) = &m.tool_call_id {
                msg_json["tool_call_id"] = serde_json::json!(tci);
            }
            msg_json
        })
        .collect();
    messages.push(serde_json::json!({ "role": "user", "content": user_prompt }));

    // ★ 截断可能使保留段以 tool 开头 / 前置 assistant(tool_calls) 被截掉，
    //   导致 DeepSeek HTTP 400 "Messages with role 'tool' must be a response to a preceding message with 'tool_calls'"。
    //   统一清理悬空 tool 消息。
    crate::harness::engine::context::prune_dangling_tools(&mut messages);

    // 诊断：打印历史消息（role + 内容前 100 字符），排查坏消息导致服务端静默停止
    for (i, m) in messages.iter().enumerate() {
        let content = m["content"].as_str().unwrap_or("").to_string();
        let content_preview: String = content.chars().take(100).collect();
        eprintln!(
            "[RUNNER] msg[{i}] role={} tool_calls={} tool_call_id={} content={:?}",
            m["role"].as_str().unwrap_or("?"),
            m.get("tool_calls").is_some(),
            m.get("tool_call_id").is_some(),
            content_preview
        );
    }

    // ── 独立 runtime 调度 ──
    // 技能按需加载（方案 1）：固定保留 builtin/mcp，skillhub 技能按消息检索 top-N
    let tools_json = crate::llm::serialize_tools(&crate::llm::tool_selector::select_tools(
        &registry.list(),
        user_prompt,
        crate::llm::tool_selector::DEFAULT_TOP_N,
    ));
    let mut system_prompt = crate::llm::build_system_prompt();
    // ★ 智能分层：按任务复杂度动态注入委派引导（简单直答 / 复杂只读委派子 Agent / 写操作主 Agent 亲办）
    match crate::llm::classify_task(user_prompt) {
        crate::llm::TaskComplexity::Complex => {
            system_prompt.push_str(
                "\n\n## 当前任务判定（程序自动分析）\n本任务已被判定为【复杂只读任务】。\n请优先调用 `agent_subtask` 工具，把需要读取文件 / 查目录 / 深度调研 / 分析统计的部分\n委派给子 Agent（独立上下文 + 只读工具集）执行，拿到精炼结论后再综合整理回答用户。\n",
            );
        }
        crate::llm::TaskComplexity::Simple => {
            system_prompt.push_str(
                "\n\n## 当前任务判定（程序自动分析）\n本任务已被判定为【简单任务】。直接回答用户即可，不要调用工具、不要委派子任务。\n",
            );
        }
        crate::llm::TaskComplexity::Neutral => { /* 需写/执行或不确定：主 Agent 自行处理 */ }
    }
    // ★ 额外系统提示（微信人设等）：追加到系统 prompt 末尾，约束 AI 的角色与风格
    if let Some(extra) = extra_system_prompt {
        if !extra.trim().is_empty() {
            system_prompt.push_str("\n\n## 你的角色设定（用户指定，必须严格遵守）\n");
            system_prompt.push_str(extra.trim());
            system_prompt.push('\n');
        }
    }
    eprintln!(
        "[RUNNER] 请求规模: system_prompt={} 字符, tools={} 个, messages={} 条, model={}, 复杂度={:?}",
        system_prompt.len(),
        tools_json.as_array().map(|a| a.len()).unwrap_or(0),
        messages.len(),
        cfg.model,
        crate::llm::classify_task(user_prompt),
    );
    let config = TurnConfig {
        params: ModelParams {
            model: cfg.model.clone(),
            reasoning_effort: cfg.effort,
            ..Default::default()
        },
        system_prompt: Some(system_prompt),
        max_tool_calls_per_turn: max_rounds.clamp(1, 50),
        tools: tools_json,
    };

    let messages = Arc::new(tokio::sync::RwLock::new(messages));
    let client_loop = client.clone();
    let executor_loop = executor.clone();
    let ctx_loop = ctx_mgr.clone();
    let msgs_loop = messages.clone();
    let cancel_loop = cancel_token.clone();
    let tx_loop = tx_event.clone();

    // ── 引擎任务（优化2：oneshot 完成通知替代忙等轮询）──
    let (tx_done, mut rx_done) = tokio::sync::oneshot::channel::<()>();
    let engine_handle = ENGINE_RT.spawn(async move {
        let r = turn_loop::run_turn_loop(
            client_loop,
            executor_loop,
            ctx_loop,
            msgs_loop,
            config,
            cancel_loop,
            tx_loop,
        )
        .await
        .map_err(|e| e.to_string());
        let _ = tx_done.send(());
        r
    });

    // ── 事件转发（阻塞式 select!）──
    // 缺陷1修复：收集工具调用记录与轮次文本，回填 ToolLoopOutcome.rounds
    let mut tool_records: Vec<ToolCallRecord> = Vec::new();
    let mut round_model_text = String::new();
    let mut thinking_text = String::new();
    // 思考中提示只发一次，避免几十个 ThinkingDelta 各自 emit → 事件洪峰 →
    // 前端 Vue 重渲染卡顿 → tx_event 通道满 → turn_loop send().await 阻塞 → 流式读取暂停
    let mut thinking_hint_sent = false;
    // ★ 思考打印节流（2026-08-12 修复）：按时间戳节流（每 200ms 最多打一次），
    //   替代原 `len%1000 < text.len()` 的奇怪逻辑（每片 delta 都触发打印）。
    let mut last_think_print = std::time::Instant::now();

    // ★ 整体超时（2026-08-12 修复）：timeout_secs 参数此前被 `let _ =` 丢弃，
    //   现在真正用于包裹主循环 —— 超时返回明确错误并取消引擎任务。
    let outcome = tokio::time::timeout(
        Duration::from_secs(timeout_secs.max(1)),
        async {
            loop {
                tokio::select! {
                    ev = rx_event.recv() => {
                        match ev {
                            Some(ev) => {
                                match ev {
                                    EngineEvent::TextDelta(text) => {
                                        round_model_text.push_str(&text);
                                        progress(&AgentProgress::ModelText { round: 0, text });
                                    }
                                    EngineEvent::ThinkingDelta(text) => {
                                        // 累积思考链，不逐片发送（DeepSeek 把 reasoning 切成小块流式返回，
                                        // 逐片发送会导致前端显示成碎片）。思考中提示只发一次。
                                        thinking_text.push_str(&text);
                                        if last_think_print.elapsed() >= Duration::from_millis(200) {
                                            last_think_print = std::time::Instant::now();
                                            println!("[THINK] 已累积 {} 字符", thinking_text.len());
                                        }
                                        if !thinking_hint_sent {
                                            thinking_hint_sent = true;
                                            progress(&AgentProgress::ModelText {
                                                round: 0,
                                                text: "💭".into(),
                                            });
                                        }
                                    }
                                    EngineEvent::ToolCallStart { name } => {
                                        let tool_id = crate::llm::decode_tool_name(&name);
                                        tool_records.push(ToolCallRecord {
                                            tool_id: tool_id.clone(),
                                            arguments: Value::Null,
                                            status: "running".into(),
                                            output: Value::Null,
                                        });
                                        progress(&AgentProgress::ToolCall(ToolCallProgress {
                                            round: 0,
                                            tool_id,
                                            arguments: Value::Null,
                                            status: "started".into(),
                                            output: Value::Null,
                                        }));
                                    }
                                    EngineEvent::ToolCallEnd { name, result, ok } => {
                                        let tool_id = crate::llm::decode_tool_name(&name);
                                        if let Some(rec) = tool_records.iter_mut().rev().find(|rec| rec.tool_id == tool_id) {
                                            rec.status = if ok { "success" } else { "error" }.into();
                                            rec.output = serde_json::json!({ "content": result });
                                        }
                                        progress(&AgentProgress::ToolCall(ToolCallProgress {
                                            round: 0,
                                            tool_id,
                                            arguments: Value::Null,
                                            status: "finished".into(),
                                            output: serde_json::json!({ "content": result }),
                                        }));
                                    }
                                    EngineEvent::ConfirmRequired { call_id, tool_id, risk_level, arguments } => {
                                        let risk = if risk_level == "high" { RiskLevel::High } else { RiskLevel::Normal };
                                        progress(&AgentProgress::ConfirmRequired {
                                            call_id,
                                            tool_id,
                                            risk_level: risk,
                                            arguments,
                                        });
                                    }
                                    EngineEvent::Status(text) => {
                                        progress(&AgentProgress::ModelText { round: 0, text });
                                    }
                                    EngineEvent::Error(text) => {
                                        progress(&AgentProgress::ModelText {
                                            round: 0,
                                            text: format!("[错误] {text}"),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                            None => {
                                // 通道关闭 = 引擎已退出
                                break engine_handle.await.map_err(|e| format!("引擎任务失败: {e}"))?;
                            }
                        }
                    }
                    _ = &mut rx_done => {
                        // 引擎已结束：排空剩余事件后退出
                        while let Ok(ev) = rx_event.try_recv() {
                            match ev {
                                EngineEvent::TextDelta(text) => {
                                    round_model_text.push_str(&text);
                                    progress(&AgentProgress::ModelText { round: 0, text });
                                }
                                EngineEvent::ThinkingDelta(text) => {
                                    thinking_text.push_str(&text);
                                    if last_think_print.elapsed() >= Duration::from_millis(200) {
                                        last_think_print = std::time::Instant::now();
                                        println!("[THINK] 已累积 {} 字符", thinking_text.len());
                                    }
                                    if !thinking_hint_sent {
                                        thinking_hint_sent = true;
                                        progress(&AgentProgress::ModelText { round: 0, text: "💭".into() });
                                    }
                                }
                                EngineEvent::ToolCallStart { name } => {
                                    let tool_id = crate::llm::decode_tool_name(&name);
                                    tool_records.push(ToolCallRecord {
                                        tool_id: tool_id.clone(),
                                        arguments: Value::Null,
                                        status: "running".into(),
                                        output: Value::Null,
                                    });
                                }
                                EngineEvent::ToolCallEnd { name, result, ok } => {
                                    let tool_id = crate::llm::decode_tool_name(&name);
                                    if let Some(rec) = tool_records.iter_mut().rev().find(|rec| rec.tool_id == tool_id) {
                                        rec.status = if ok { "success" } else { "error" }.into();
                                        rec.output = serde_json::json!({ "content": result });
                                    }
                                }
                                EngineEvent::Status(text) => {
                                    progress(&AgentProgress::ModelText { round: 0, text });
                                }
                                EngineEvent::Error(text) => {
                                    progress(&AgentProgress::ModelText { round: 0, text: format!("[错误] {text}") });
                                }
                                _ => {}
                            }
                        }
                        break engine_handle.await.map_err(|e| format!("引擎任务失败: {e}"))?;
                    }
                }
            }
        },
    )
    .await;

    let result: Result<TurnResult, String> = match outcome {
        Ok(r) => r,
        Err(_) => {
            // 整体超时：取消引擎任务，返回明确错误
            cancel_token.cancel();
            let msg = format!("任务执行超过整体超时（{}s），已取消", timeout_secs);
            eprintln!("[RUNNER] {msg}");
            Err(msg)
        }
    };

    let _ = watch.abort();

    match result {
        Ok(turn) => {
            eprintln!(
                "[RUNNER] turn 完成: final_text={} 字符, thinking={} 字符, tool_calls={}",
                turn.final_text.len(),
                thinking_text.len(),
                turn.used_tool_calls
            );
            println!("[THINK] 完成，总 {} 字符", thinking_text.len());
            // 思考完成后，一次性发送完整思考链（前端放入可折叠区）
            if !thinking_text.is_empty() {
                progress(&AgentProgress::ModelText {
                    round: 0,
                    text: format!("💭\n{thinking_text}"),
                });
            }
            // 回写会话记忆（优化1：内存 + SQLite 持久化）
            // ★ 根治历史污染：只保留「user + 最终 assistant 回答」，丢弃工具循环的中间消息
            //   （tool 消息、带 tool_calls 的 assistant）。否则失败的工具循环会无限累积坏历史，
            //   模型每次请求都被坏历史干扰 → 乱调工具 → 更坏历史（恶性循环）。
            //   tool_calls 只在"当前轮"用于 OpenAI 格式，无需跨轮持久化。
            let updated: Vec<LlmMessage> = messages
                .read()
                .await
                .iter()
                .filter(|v| {
                    let role = v["role"].as_str().unwrap_or("user");
                    if role == "tool" {
                        return false; // 丢弃工具结果消息
                    }
                    if role == "assistant" {
                        // 只保留最终回答（无 tool_calls 且有内容）；丢弃带 tool_calls 的中间轮
                        return v.get("tool_calls").is_none()
                            && !v["content"].as_str().unwrap_or("").trim().is_empty();
                    }
                    true
                })
                .map(|v| LlmMessage {
                    role: role_from_str(v["role"].as_str().unwrap_or("user")),
                    content: v["content"].as_str().unwrap_or("").to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                })
                .collect();
            // ★ 合并连续的 user 消息（只保留最后一个），避免历史里堆叠重复用户输入
            let mut merged: Vec<LlmMessage> = Vec::new();
            for m in updated {
                let is_user = matches!(m.role, Role::User);
                let last_is_user = merged
                    .last()
                    .map(|l| matches!(l.role, Role::User))
                    .unwrap_or(false);
                if is_user && last_is_user {
                    merged.pop();
                }
                merged.push(m);
            }
            session.messages = merged;
            sessions.update(session);
            // ★ 累计真实 token 用量到会话（上下文占用面板数据源）
            let usage = Usage {
                prompt_tokens: turn.usage_input_tokens,
                completion_tokens: turn.usage_output_tokens,
                total_tokens: turn.usage_input_tokens + turn.usage_output_tokens,
            };
            record_usage(sessions, session_id, &usage);

            Ok(ToolLoopOutcome {
                used_rounds: turn.used_tool_calls,
                rounds: vec![RoundRecord {
                    round: 1,
                    model_text: round_model_text,
                    tool_calls: tool_records,
                    usage: None,
                }],
                final_text: turn.final_text,
                truncated: false,
                usage,
            })
        }
        Err(e) => {
            eprintln!("[RUNNER] turn 失败: {e}");
            Err(e)
        }
    }
}

fn role_from_str(s: &str) -> Role {
    match s {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    }
}
