//! 方案B追加：harness 引擎 IPC 命令。

use std::sync::Arc;

use tauri::{Emitter, State};

use crate::commands::AppState;

#[tauri::command]
pub fn harness_set_model_config(api_key: String, base_url: String, effort: String, model: Option<String>) -> Result<serde_json::Value, String> {
    let effort_parsed = crate::harness::engine::param::ReasoningEffort::from_str(&effort);
    let cfg = crate::harness::engine::config::EngineConfig { api_key, base_url, model: model.unwrap_or_else(|| "deepseek-chat".to_string()), effort: effort_parsed };
    crate::harness::engine::config::set_engine_config(cfg);
    Ok(serde_json::json!({ "ok": true, "protocol": "HTTP/1.1 (SSE)" }))
}

#[tauri::command]
pub fn harness_status() -> serde_json::Value {
    match crate::harness::engine::config::engine_config() {
        Some(cfg) => serde_json::json!({ "configured": true, "baseUrl": cfg.base_url, "model": cfg.model, "effort": cfg.effort.as_str() }),
        None => serde_json::json!({ "configured": false }),
    }
}

#[tauri::command]
pub async fn harness_start_task(app: tauri::AppHandle, state: State<'_, AppState>, api_key: String, base_url: String, effort: String, model: Option<String>, session_id: String, prompt: String) -> Result<serde_json::Value, String> {
    use crate::harness::core::turn_loop::{self, EngineEvent, ToolExecutor, TurnConfig, TurnResult};
    use crate::harness::engine::client::LlmClient;
    use crate::harness::engine::context::ContextManager;
    use crate::harness::engine::param::{ModelParams, ReasoningEffort};

    let cfg = crate::harness::engine::config::EngineConfig { api_key: api_key.clone(), base_url: base_url.clone(), model: model.unwrap_or_else(|| "deepseek-chat".to_string()), effort: ReasoningEffort::from_str(&effort) };
    crate::harness::engine::config::set_engine_config(cfg.clone());

    let client = LlmClient::new(api_key, base_url).map_err(|e| e.to_string())?;
    let ctx_mgr = Arc::new(ContextManager::new(Some(client.clone())));

    let run_id = format!("engine-{}", uuid::Uuid::new_v4());
    let cancel_flag = state.cancel_tokens.create(run_id.clone());
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_watch = cancel_flag.clone();
    let cancel_token_clone = cancel_token.clone();
    let watcher = tokio::spawn(async move { loop { if cancel_watch.is_cancelled() { cancel_token_clone.cancel(); break; } tokio::time::sleep(std::time::Duration::from_millis(100)).await; } });

    let registry = state.registry.clone();
    let dispatcher = state.dispatcher.clone();
    let agent_mode = *state.agent_mode.read().unwrap();

    let executor: Arc<dyn ToolExecutor> = Arc::new(crate::llm::runner::DispatcherExecutor {
        registry: registry.clone(), dispatcher, round: std::sync::atomic::AtomicUsize::new(0),
        confirm: if matches!(agent_mode, crate::llm::AgentMode::StepConfirm) { Some(crate::harness::hooks::bridge::make_step_confirm_callback(registry.clone(), None)) } else { None },
    });

    let session = state.sessions.get_or_create(&session_id);
    let mut messages: Vec<serde_json::Value> = session.messages.iter().map(|m| {
        let role = match m.role { crate::llm::Role::System=>"system", crate::llm::Role::User=>"user", crate::llm::Role::Assistant=>"assistant", crate::llm::Role::Tool=>"tool" };
        serde_json::json!({ "role": role, "content": m.content })
    }).collect();
    messages.push(serde_json::json!({ "role": "user", "content": prompt }));

    let (tx_event, mut rx_event) = tokio::sync::mpsc::channel::<EngineEvent>(256);
    let app_relay = app.clone();
    let relay = tokio::spawn(async move { while let Some(ev) = rx_event.recv().await {
        let payload = match ev {
            EngineEvent::TextDelta(t) => serde_json::json!({ "type":"text_delta","content":t }),
            EngineEvent::ThinkingDelta(t) => serde_json::json!({ "type":"thinking_delta","content":t }),
            EngineEvent::ToolCallStart{name} => serde_json::json!({ "type":"tool_start","name":name }),
            EngineEvent::ToolCallEnd{name,result,ok} => serde_json::json!({ "type":"tool_end","name":name,"result":result,"ok":ok }),
            EngineEvent::ConfirmRequired{call_id,tool_id,risk_level,arguments} => serde_json::json!({ "type":"confirm","callId":call_id,"toolId":tool_id,"riskLevel":risk_level,"arguments":arguments }),
            EngineEvent::Status(s) => serde_json::json!({ "type":"status","text":s }),
            EngineEvent::Error(e) => serde_json::json!({ "type":"error","message":e }),
            EngineEvent::Usage{input_tokens,output_tokens} => serde_json::json!({ "type":"usage","inputTokens":input_tokens,"outputTokens":output_tokens }),
            EngineEvent::TurnFinished{ok,error} => serde_json::json!({ "type":"turn_finished","ok":ok,"error":error }),
        }; let _ = app_relay.emit_to("main-window","engine://stream",payload);
    }});

    let params = ModelParams { model: cfg.model.clone(), reasoning_effort: cfg.effort, ..Default::default() };
    // 技能按需加载（方案 1）：固定保留 builtin/mcp，skillhub 技能按消息检索 top-N
    let mut selected = crate::llm::tool_selector::select_tools(
        &registry.list(),
        &prompt,
        crate::llm::tool_selector::DEFAULT_TOP_N,
    );
    let tools_json = crate::llm::serialize_tools(&selected);
    let turn_cfg = TurnConfig { params, system_prompt: Some(crate::llm::build_system_prompt()), max_tool_calls_per_turn: (*state.max_rounds.read().unwrap()).clamp(1, 50), tools: tools_json };
    let messages_arc = Arc::new(tokio::sync::RwLock::new(messages));
    let client_loop = client.clone(); let executor_loop = executor.clone(); let ctx_loop = ctx_mgr.clone(); let msgs_loop = messages_arc.clone(); let cancel_loop = cancel_token.clone(); let tx_loop = tx_event.clone();
    let result: Result<TurnResult, String> = crate::harness::ENGINE_RT.spawn(async move { turn_loop::run_turn_loop(client_loop,executor_loop,ctx_loop,msgs_loop,turn_cfg,cancel_loop,tx_loop).await.map_err(|e| e.to_string()) }).await.map_err(|e| format!("引擎任务失败: {e}"))?;
    // ★ 2026-08-12 修复：不再立即 abort relay（尾部事件丢失）。
    //   引擎退出时其 tx_event 被 drop → 通道关闭 → relay 的 recv() 返回 None 自然退出，
    //   期间会把通道里剩余事件（含 TurnFinished）全部转发完。这里 await 等它排空。
    let _ = watcher.abort();
    let _ = relay.await;
    state.cancel_tokens.remove(&run_id); state.cancel_tokens.clear_confirms();

    match result {
        Ok(turn) => {
            let mut session = state.sessions.get_or_create(&session_id);
            let updated = messages_arc.read().await;
            session.messages = updated.iter().map(|v| {
                let role = match v["role"].as_str().unwrap_or("user") { "system"=>crate::llm::Role::System, "assistant"=>crate::llm::Role::Assistant, "tool"=>crate::llm::Role::Tool, _=>crate::llm::Role::User };
                crate::llm::LlmMessage { role, content: v["content"].as_str().unwrap_or("").to_string(), tool_calls: None, tool_call_id: None }
            }).collect();
            state.sessions.update(session);
            Ok(serde_json::json!({ "ok":true, "finalText":turn.final_text, "usedToolCalls":turn.used_tool_calls, "usage":{ "inputTokens":turn.usage_input_tokens, "outputTokens":turn.usage_output_tokens } }))
        }
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub fn harness_stop_task(state: State<'_, AppState>, run_id: String) -> bool { state.cancel_tokens.cancel(&run_id) }

#[tauri::command]
pub async fn harness_respond_permission(request_id: String, approved: bool, note: Option<String>) -> bool {
    crate::harness::hooks::bridge::PERMISSION_BRIDGE.get().map(|b| crate::harness::ENGINE_RT.block_on(b.resolve(&request_id, approved, note))).unwrap_or(false)
}
