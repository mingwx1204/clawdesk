//! 引擎调度核心 —— 从 CodeWhale tui/src/core/engine/turn_loop.rs 重构移植（ClawDesk 副本版）。
//!
//! 3 个 BUG 修复点（★）：
//!   1. ★ 独立 ENGINE_RT 多线程 tokio runtime，与 Tauri runtime 完全隔离；
//!   2. ★ 显式 DISPATCH_TIMEOUT=600s / TOOL_EXEC_TIMEOUT=480s；
//!   3. ★ 轮次心跳 AtomicU64 + 心跳监控协程（停滞 30s → 取消令牌 abort）；
//!   4. ★ 所有工具调用强制 tokio::time::timeout 包裹。
//!
//! 裁剪说明（与原版 4914 行相比）：
//!   - 剔除：TUI 事件、LSP post-edit hooks、subagent/fleet 编排、goal/plan 状态、
//!     approval 模态通道（ClawDesk 用权限桥替代）、stuck-guard 指纹检测；
//!   - 保留：SSE 流式驱动、工具批量执行、上下文压缩挂载、取消/心跳/看门狗/工具超时。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use super::super::engine::client::LlmClient;
use super::super::engine::context::{CompactionStatus, ContextManager, fallback_trim, KEEP_RECENT_MESSAGES};
use super::super::engine::param::ModelParams;
use super::super::engine::stream::SseEvent;

// ══════════════════════════════════════════════════════════════
// ★ 1. 独立 runtime
// ══════════════════════════════════════════════════════════════

/// 引擎专用多线程 runtime —— 与 Tauri 自带 runtime 完全隔离。
/// 惰性初始化一次；长任务在此运行，不饿死 Tauri 事件循环。
pub static ENGINE_RT: std::sync::LazyLock<tokio::runtime::Runtime> =
    std::sync::LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("clawdesk-engine")
            .enable_all()
            .build()
            .expect("构建引擎 tokio runtime 失败")
    });

// ══════════════════════════════════════════════════════════════
// ★ 2. 超时常量
// ══════════════════════════════════════════════════════════════

/// 单轮调度总超时（看门狗级别）。
pub const DISPATCH_TIMEOUT: Duration = Duration::from_secs(600);
/// 单个工具执行超时。
pub const TOOL_EXEC_TIMEOUT: Duration = Duration::from_secs(480);
/// SSE 单块空闲超时。
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// 心跳刷新间隔。
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// 心跳停滞判定阈值。
/// 注意：SSE 流式读取期间 `stream.next()` 会阻塞等待数据，DeepSeek 思考/生成阶段
/// 可能停顿 30s+（尤其带大量工具的大请求），此前 30s 阈值会误杀正常任务。
/// 改为 320s（略大于 STREAM_IDLE_TIMEOUT=300s），流式阶段由 300s 超时自行兜底，
/// 心跳监控只用于兜底真正的死锁（工具执行/上下文压缩卡死）。
pub const HEARTBEAT_STALL_THRESHOLD: Duration = Duration::from_secs(320);

// ══════════════════════════════════════════════════════════════
// 类型定义
// ══════════════════════════════════════════════════════════════

/// 工具执行抽象 —— 由宿主（ClawDesk 现有 ToolDispatcher 适配）实现。
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    /// 执行一次工具调用；返回工具结果文本。
    /// 宿主应在内部完成沙箱/权限（StepConfirm 确认）检查。
    async fn execute(&self, name: &str, arguments: Value) -> Result<String>;
}

/// 引擎事件 —— 宿主桥接到前端（agent://progress / engine://stream）。
#[derive(Debug, Clone)]
pub enum EngineEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallStart { name: String },
    ToolCallEnd {
        name: String,
        result: String,
        /// 工具是否执行成功（false = 执行错误，前端显示"失败"）。
        ok: bool,
    },
    Status(String),
    Error(String),
    Usage { input_tokens: u64, output_tokens: u64 },
    /// StepConfirm 权限确认请求（宿主转发为前端弹窗）。
    ConfirmRequired {
        call_id: String,
        tool_id: String,
        risk_level: String,
        arguments: Value,
    },
    TurnFinished { ok: bool, error: Option<String> },
}

/// 单轮调度配置。
pub struct TurnConfig {
    pub params: ModelParams,
    pub system_prompt: Option<String>,
    /// 每轮最大工具调用数（与前端 max_rounds 语义一致）。
    pub max_tool_calls_per_turn: usize,
    /// 工具描述（OpenAI function schema，经 serialize_tools 生成）。
    pub tools: serde_json::Value,
}

/// 调度循环结果。
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub final_text: String,
    pub used_tool_calls: usize,
    pub usage_input_tokens: u64,
    pub usage_output_tokens: u64,
}

// ══════════════════════════════════════════════════════════════
// 主调度循环
// ══════════════════════════════════════════════════════════════

/// 运行一轮完整 Agent 调度循环。
///
/// # 参数
/// - `client`：reqwest 流式客户端（engine/client.rs）
/// - `executor`：工具执行器（宿主 ToolDispatcher 适配）
/// - `ctx_mgr`：上下文管理器（engine/context.rs）
/// - `messages`：OpenAI 兼容消息数组（可变，循环中持续更新）
/// - `cancel`：取消令牌（宿主 agent_cancel 桥接）
/// - `tx_event`：引擎事件通道（宿主桥接前端）
pub async fn run_turn_loop(
    client: LlmClient,
    executor: Arc<dyn ToolExecutor>,
    ctx_mgr: Arc<ContextManager>,
    messages: Arc<RwLock<Vec<Value>>>,
    config: TurnConfig,
    cancel: CancellationToken,
    tx_event: mpsc::Sender<EngineEvent>,
) -> Result<TurnResult> {
    // ── 心跳状态 ──
    let heartbeat = Arc::new(AtomicU64::new(now_unix()));
    let mut final_text = String::new();
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut used_tool_calls = 0usize;
    let mut loop_count = 0usize;

    // ★ 3. 心跳监控协程：停滞超过阈值 → 取消令牌（宿主可据此 abort 卡死任务）
    {
        let hb = heartbeat.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
                let last = hb.load(Ordering::Acquire);
                if now_unix().saturating_sub(last) > HEARTBEAT_STALL_THRESHOLD.as_secs() {
                    eprintln!(
                        "[HB] 心跳停滞超过 {}s（上次心跳 {}），abort 卡死任务",
                        HEARTBEAT_STALL_THRESHOLD.as_secs(),
                        last
                    );
                    cancel.cancel();
                    break;
                }
            }
        });
    }

    // ★ 整个调度循环用 DISPATCH_TIMEOUT 看门狗包裹
    // ★ 连续工具失败计数：打破"工具参数坏→失败→历史污染→模型更乱"的恶性循环
    let mut consecutive_tool_failures = 0usize;
    let outcome = tokio::time::timeout(DISPATCH_TIMEOUT, async {
        loop {
            loop_count += 1;
            heartbeat.store(now_unix(), Ordering::Release);
            eprintln!(
                "[TL] 第 {} 轮开始，messages={} 条，cancel={}",
                loop_count,
                messages.read().await.len(),
                cancel.is_cancelled()
            );

            if cancel.is_cancelled() {
                return Ok::<_, anyhow::Error>(TurnResult {
                    final_text,
                    used_tool_calls,
                    usage_input_tokens: input_tokens,
                    usage_output_tokens: output_tokens,
                });
            }

            // ── 上下文压缩检查（★ 后台协程，不阻塞）──
            let cur = messages.read().await.clone();
            if ctx_mgr.should_compact(&cur).await {
                let _ = tx_event
                    .send(EngineEvent::Status("Compacting context...".into()))
                    .await;
                let snapshot = cur.clone();
                let started = ctx_mgr.compact_async(snapshot).await;
                if started {
                    let st = ctx_mgr.wait_for_compaction().await;
                    match st {
                        CompactionStatus::Completed => {
                            if let Some(summary) = ctx_mgr.summary_text().await {
                                let fresh = cur.clone();
                                let replaced = ContextManager::apply_summary(fresh, &summary);
                                *messages.write().await = replaced;
                                let _ = tx_event
                                    .send(EngineEvent::Status("Context compacted".into()))
                                    .await;
                            }
                        }
                        CompactionStatus::FallbackTrimmed => {
                            let fresh = cur.clone();
                            let trimmed = fallback_trim(fresh, KEEP_RECENT_MESSAGES);
                            *messages.write().await = trimmed;
                            let _ = tx_event
                                .send(EngineEvent::Status("Context trimmed (fallback)".into()))
                                .await;
                        }
                        _ => {}
                    }
                }
            }

            // ── 发起流式请求 ──
            let cur = messages.read().await.clone();
            let mut stream = client
                .stream_chat(&config.params, &cur, config.system_prompt.as_deref(), &config.tools)
                .await
                .map_err(|e| {
                    // 注意：不能在这里用 blocking_send（当前在 async runtime 线程内，会 panic
                    // "Cannot block the current thread from within a runtime"），用 try_send 非阻塞发送。
                    let _ = tx_event.try_send(EngineEvent::Error(e.to_string()));
                    e
                })?;

            // ── 处理 SSE 事件 ──
            let mut assistant_text = String::new();
            let mut tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, args)

            while !cancel.is_cancelled() {
                heartbeat.store(now_unix(), Ordering::Release);
                let next = tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await;
                match next {
                    Ok(Some(Ok(event))) => match event {
                        SseEvent::TextDelta { content } => {
                            eprintln!("[SSE] TextDelta: {:?}", content);
                            assistant_text.push_str(&content);
                            final_text.push_str(&content);
                            let _ = tx_event.send(EngineEvent::TextDelta(content)).await;
                        }
                        SseEvent::ThinkingDelta { content } => {
                            eprintln!("[SSE] ThinkingDelta: {:?}", content);
                            let _ = tx_event.send(EngineEvent::ThinkingDelta(content)).await;
                        }
                        SseEvent::ToolCallStart { id, name, index } => {
                            eprintln!("[SSE] ToolCallStart: id={:?} name={} index={}", id, name, index);
                            let _ = tx_event
                                .send(EngineEvent::ToolCallStart { name: name.clone() })
                                .await;
                            // ★ 并行工具按 index 占位：确保 tool_calls[index] 存在
                            while tool_calls.len() <= index as usize {
                                tool_calls.push((String::new(), String::new(), String::new()));
                            }
                            tool_calls[index as usize] = (id, name, String::new());
                        }
                        SseEvent::ToolCallDelta { id, arguments, index } => {
                            // ★ BUG 修复：OpenAI 流式 tool_calls 的后续 chunk 可能无 id 但有 index。
                            // 并行工具调用时（同一轮多个工具），必须按 index 匹配——否则参数串到错误的工具。
                            eprintln!("[SSE] ToolCallDelta: id={:?} index={} args={:?}", id, index, arguments);
                            let idx = index as usize;
                            if idx < tool_calls.len() {
                                tool_calls[idx].2.push_str(&arguments);
                            } else if !id.is_empty() {
                                if let Some(i) = tool_calls.iter().position(|(tid, _, _)| *tid == id) {
                                    tool_calls[i].2.push_str(&arguments);
                                }
                            }
                        }
                        SseEvent::MessageStop => break,
                        SseEvent::Usage { input_tokens: i, output_tokens: o } => {
                            input_tokens += i;
                            output_tokens += o;
                        }
                        SseEvent::Error { message } => {
                            let _ = tx_event.send(EngineEvent::Error(message)).await;
                        }
                    },
                    Ok(Some(Err(e))) => {
                        // 流错误（含 decode body 报错）：非致命，上报后结束本轮
                        eprintln!("[SSE] 流错误: {e:?}");
                        let _ = tx_event
                            .send(EngineEvent::Error(format!("流错误(继续): {e}")))
                            .await;
                        break;
                    }
                    Ok(None) => {
                        eprintln!("[SSE] 流正常结束(None), assistant_text={:?}", assistant_text);
                        break; // 流正常结束
                    }
                    Err(_) => {
                        // 单块空闲超时：本流结束，可进入下一轮
                        eprintln!("[SSE] 流空闲超时({}s)", STREAM_IDLE_TIMEOUT.as_secs());
                        let _ = tx_event
                            .send(EngineEvent::Status("流空闲超时，结束本轮".into()))
                            .await;
                        break;
                    }
                }
            }

            // ── 追加 assistant 消息 ──
            // OpenAI 格式要求：若本轮有工具调用，assistant 消息必须带 tool_calls 字段，
            // 否则后续 role:"tool" 消息会报 "must be a response to a preceding message with 'tool_calls'"。
            eprintln!(
                "[TL] 第 {} 轮 SSE 结束: assistant_text={} 字符, tool_calls={} 个",
                loop_count,
                assistant_text.len(),
                tool_calls.len()
            );
            {
                let mut msgs = messages.write().await;
                let mut assistant_msg = serde_json::Map::new();
                assistant_msg.insert("role".into(), serde_json::json!("assistant"));
                assistant_msg.insert(
                    "content".into(),
                    if assistant_text.is_empty() {
                        Value::Null
                    } else {
                        Value::String(assistant_text.clone())
                    },
                );
                if !tool_calls.is_empty() {
                    assistant_msg.insert(
                        "tool_calls".into(),
                        Value::Array(
                            tool_calls
                                .iter()
                                .map(|(tid, tname, targs)| {
                                    serde_json::json!({
                                        "id": tid,
                                        "type": "function",
                                        "function": {
                                            "name": tname,
                                            "arguments": if targs.is_empty() { "{}" } else { targs },
                                        }
                                    })
                                })
                                .collect(),
                        ),
                    );
                }
                msgs.push(Value::Object(assistant_msg));
            }

            // ── 执行工具调用（★ 全部 timeout 包裹 + ★ 并行 join_all）──
            // ★ 多 Agent 真并行：同一轮多个工具调用（如多个 agent_subtask 子代理）
            //   并发执行，总耗时 ≈ 最慢的一个，而非逐个累加。
            let mut any_tool = false;
            if !tool_calls.is_empty() {
                any_tool = true;
                used_tool_calls += tool_calls.len();
                heartbeat.store(now_unix(), Ordering::Release);

                // 并行启动全部工具调用（每个独立 timeout）
                let tasks: Vec<_> = tool_calls
                    .iter()
                    .map(|(id, name, args)| {
                        let executor = executor.clone();
                        let cancel = cancel.clone();
                        let name = name.clone();
                        let id = id.clone();
                        let args = args.clone();
                        async move {
                            if cancel.is_cancelled() {
                                return (id, name, "任务已取消".to_string(), true);
                            }
                            let arguments: Value = serde_json::from_str(&args).unwrap_or_else(|e| {
                                eprintln!("[TL] 解析工具参数失败: {e} | args={:?}", args);
                                Value::Null
                            });
                            eprintln!("[TL] 执行工具(并行): name={} args={:?} parsed={:?}", name, args, arguments);
                            let exec = executor.execute(name.as_str(), arguments);
                            let result = tokio::time::timeout(TOOL_EXEC_TIMEOUT, exec).await;
                            match result {
                                Ok(Ok(text)) => (id, name, text, false),
                                Ok(Err(e)) => (id, name, format!("工具执行错误: {e}"), true),
                                Err(_) => {
                                    // ★ 工具卡死 → 注入超时错误，不阻塞主循环
                                    let msg = format!(
                                        "工具 {name} 执行超过 {}s，已强制终止",
                                        TOOL_EXEC_TIMEOUT.as_secs()
                                    );
                                    tracing::error!(target: "engine.turn_loop", "{msg}");
                                    (id, name, msg, true)
                                }
                            }
                        }
                    })
                    .collect();

                let results = futures_util::future::join_all(tasks).await;
                heartbeat.store(now_unix(), Ordering::Release);

                // ★ 连续失败熔断：统计本轮失败数；≥3 次 → 记录并准备终止循环
                let fail_count = results.iter().filter(|(_, _, _, is_err)| *is_err).count();
                for (_, _, text, is_err) in &results {
                    if *is_err || text.contains("[MaxRoundsExceeded]") {
                        consecutive_tool_failures += 1;
                        eprintln!(
                            "[TL] 工具失败（连续 {} 次）: {}",
                            consecutive_tool_failures,
                            crate::llm::truncate(text, 200)
                        );
                    } else {
                        consecutive_tool_failures = 0;
                    }
                }
                if fail_count >= 3 {
                    eprintln!("[TL] 本轮 {} 个工具失败（≥3），准备终止循环", fail_count);
                    consecutive_tool_failures = fail_count.max(consecutive_tool_failures);
                }

                // 事件 + 消息回填（★ 保持 tool_calls 原顺序，OpenAI 要求 tool 消息一一对应）
                for (id, name, text, is_err) in results {
                    let _ = tx_event
                        .send(EngineEvent::ToolCallEnd {
                            name: name.clone(),
                            result: text.clone(),
                            ok: !is_err,
                        })
                        .await;

                    let mut msgs = messages.write().await;
                    msgs.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": text,
                        "is_error": is_err,
                    }));
                }
            }

            // ── 终止条件 ──
            // ★ 连续失败 ≥3 次：真正终止循环（避免模型反复用坏参数调工具，污染历史）
            if !any_tool || loop_count >= config.max_tool_calls_per_turn || consecutive_tool_failures >= 3 {
                if consecutive_tool_failures >= 3 {
                    eprintln!("[TL] 连续 {} 次工具失败，终止循环", consecutive_tool_failures);
                }
                break;
            }
        }

        Ok(TurnResult {
            final_text,
            used_tool_calls,
            usage_input_tokens: input_tokens,
            usage_output_tokens: output_tokens,
        })
    })
    .await;

    match outcome {
        Ok(Ok(result)) => {
            let _ = tx_event
                .send(EngineEvent::TurnFinished { ok: true, error: None })
                .await;
            Ok(result)
        }
        Ok(Err(e)) => {
            let _ = tx_event
                .send(EngineEvent::TurnFinished {
                    ok: false,
                    error: Some(e.to_string()),
                })
                .await;
            Err(e)
        }
        Err(_) => {
            // ★ 看门狗：调度整体超时 → 取消所有子任务
            cancel.cancel();
            let msg = format!("Turn dispatch timed out after {:?}", DISPATCH_TIMEOUT);
            tracing::error!(target: "engine.turn_loop", "{msg}");
            let _ = tx_event
                .send(EngineEvent::TurnFinished {
                    ok: false,
                    error: Some(msg.clone()),
                })
                .await;
            bail!(msg)
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
