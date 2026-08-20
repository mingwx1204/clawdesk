//! 会话管理 IPC 命令（会话列表/重命名/消息/删除/用量/导出/搜索/分支/断点）。
//!
//! 包含 11 个命令：agent_sessions / agent_session_rename / agent_session_metas /
//! agent_session_messages / agent_session_delete / agent_session_usage / session_export /
//! session_search / agent_fork / agent_checkpoint / agent_branches。

use serde_json::{json, Value};
use tauri::State;

use crate::commands::AppState;
use crate::llm::runner::ChatProvider;
use crate::llm::{truncate, LlmMessage, Role};

/// 列出全部 Agent 会话 ID。
#[tauri::command]
pub fn agent_sessions(state: State<'_, AppState>) -> Vec<String> {
    state.sessions.list()
}

/// 重命名会话（new_name 为空则清除自定义名，前端回退显示 id）。
#[tauri::command]
pub fn agent_session_rename(
    state: State<'_, AppState>,
    session_id: String,
    new_name: String,
) -> bool {
    let mut session = state.sessions.get_or_create(&session_id);
    let name = new_name.trim().to_string();
    session.name = if name.is_empty() { None } else { Some(name) };
    state.sessions.update(session);
    true
}

/// 返回全部会话的 id + 自定义名称（前端显示友好名）。
#[tauri::command]
pub fn agent_session_metas(state: State<'_, AppState>) -> Vec<serde_json::Value> {
    state
        .sessions
        .list()
        .into_iter()
        .map(|id| {
            let session = state.sessions.get_or_create(&id);
            serde_json::json!({ "id": id, "name": session.name })
        })
        .collect()
}

/// 加载指定会话的历史消息（供前端切换会话时恢复显示）。
/// 只返回 role + content（前端据此重建 ChatMsg；工具消息/思考链不落库故不回显）。
#[tauri::command]
pub fn agent_session_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Vec<serde_json::Value> {
    let session = state.sessions.get_or_create(&session_id);
    session
        .messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": serde_json::to_value(&m.role).unwrap_or(serde_json::json!("")),
                "content": m.content,
            })
        })
        .collect()
}

/// 删除一个 Agent 会话。
#[tauri::command]
pub fn agent_session_delete(state: State<'_, AppState>, session_id: String) -> bool {
    state.sessions.delete(&session_id).is_some()
}

/// 清空指定会话的上下文（删除全部消息，保留会话本身与累计用量统计）。
#[tauri::command]
pub fn agent_session_clear(state: State<'_, AppState>, session_id: String) -> bool {
    state.sessions.clear_context(&session_id)
}

/// 把对话历史转成可送给 LLM 的压缩素材。
/// 从最新消息往回收集，单条截断 1200 字，总量控制在 24K 字以内（保住最近上下文）。
fn compact_transcript(messages: &[LlmMessage]) -> String {
    const BUDGET_CHARS: usize = 24_000;
    let mut entries: Vec<String> = Vec::new();
    let mut used = 0usize;

    for m in messages.iter().rev() {
        let label = match &m.role {
            Role::User => "用户",
            Role::Assistant => "ClawDesk",
            Role::Tool => "工具结果",
            Role::System => continue,
        };
        let body = if !m.content.trim().is_empty() {
            truncate(&m.content, 1_200)
        } else if matches!(&m.role, Role::Assistant) {
            match m.tool_calls.as_ref() {
                Some(tcs) if !tcs.is_empty() => {
                    let names: Vec<String> = tcs
                        .iter()
                        .map(|t| t.function.name.replace("__", ":"))
                        .collect();
                    format!("调用工具：{}", names.join("、"))
                }
                _ => continue,
            }
        } else {
            continue;
        };

        let entry = format!("{label}：{body}
");
        let len = entry.chars().count();
        if used + len > BUDGET_CHARS {
            break; // 预算用尽：更早的历史舍弃（与 runner 的 32K token 截断策略一致）
        }
        used += len;
        entries.push(entry);
    }
    entries.reverse();
    entries.concat()
}

/// 手动压缩指定会话：用主模型生成历史摘要，调用 `SessionManager::compact_with`
/// 保留最近 keep_last 条原始消息，并用 system 摘要替代更早的历史。
#[tauri::command]
pub async fn agent_session_compact(
    state: State<'_, AppState>,
    session_id: String,
    api_key: String,
) -> Result<serde_json::Value, String> {
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("请先在「设置 → 模型 API」中填写 DeepSeek API Key".into());
    }

    let mut session = state.sessions.get_or_create(&session_id);
    if !state.sessions.can_compact(&session) {
        return Err(format!(
            "当前会话只有 {} 条历史消息，不足以压缩（压缩后仍需保留最近消息）",
            session.messages.len()
        ));
    }

    let transcript = compact_transcript(&session.messages);
    if transcript.trim().len() < 40 {
        return Err("可压缩的对话内容太少，无需压缩".into());
    }

    state.router.ensure_main_key(api_key);
    let messages = vec![
        LlmMessage {
            role: Role::System,
            content: "你是会话压缩器。请把用户与 ClawDesk 的对话历史压缩成一份简洁的上下文摘要，供后续对话继续使用。必须保留：1) 用户目标与未完成任务；2) 关键事实、偏好和约束；3) 重要文件路径、代码位置；4) 已做决策及原因。忽略寒暄、重复内容与已完成的中间过程。直接输出摘要正文，不要加任何前缀或解释。".into(),
            tool_calls: None,
            tool_call_id: None,
        },
        LlmMessage {
            role: Role::User,
            content: format!("[对话历史]
{transcript}"),
            tool_calls: None,
            tool_call_id: None,
        },
    ];
    let response = state
        .router
        .chat(&messages, &Value::Array(vec![]))
        .map_err(|e| format!("生成摘要失败：{e}"))?;

    let summary = response
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .unwrap_or_default()
        .trim()
        .to_string();
    if summary.is_empty() {
        return Err("模型返回了空摘要，请稍后重试".into());
    }
    let summary = truncate(&summary, 6_000);

    state.sessions.compact_with(&mut session, summary.clone());
    Ok(json!({
        "ok": true,
        "summary": summary,
        "messages": session.messages.len(),
        "compactions": session.compacted_count,
    }))
}

/// 输出会话用量统计（上下文窗口占用 + 累计 token 用量）。
///
/// 数据源：
/// - `windowTokens` / `pct`：runner 累计的「最近一次请求输入 token」→ 上下文窗口占用；
/// - `totalInput/Output/Tokens`：会话累计真实用量；
/// - `sys`：系统指令 + 工具定义 schema（估算）；
/// - `usr`：消息 / 工具输出 / 文件（按会话真实消息字符估算）。
#[tauri::command]
pub fn agent_session_usage(state: State<'_, AppState>, session_id: String) -> serde_json::Value {
    let s = state.sessions.get_or_create(&session_id);
    // 主模型上下文窗口（deepseek-v4 系列按 1M 估算，可在设置中调整）
    let window_limit: u64 = 1_000_000;
    // 一位小数百分比：1M 窗口下 <0.5% 的占用不再被取整成 0（例如系统指令 2200 token = 0.2%）
    let pct_of = |t: u64| -> f64 {
        if window_limit == 0 {
            0.0
        } else {
            ((t as f64 / window_limit as f64) * 1000.0).round() / 10.0
        }
    };

    // 内容细分估算（字符数 / 4 ≈ token），基于会话真实消息
    let mut msg_tokens: u64 = 0;
    let mut tool_tokens: u64 = 0;
    let mut file_tokens: u64 = 0;
    for m in &s.messages {
        let t = (m.content.chars().count() as u64) / 4;
        match m.role {
            crate::llm::Role::User => {
                if m.content.contains("[用户附件文件]") || m.content.contains("[用户图片]") {
                    file_tokens += t;
                } else {
                    msg_tokens += t;
                }
            }
            crate::llm::Role::Assistant => msg_tokens += t,
            crate::llm::Role::Tool => tool_tokens += t,
            crate::llm::Role::System => msg_tokens += t,
        }
    }

    // 系统指令固定 prompt 估算 + 工具定义 schema 估算（真实注册表）
    let sys_prompt_tokens: u64 = 2_200;
    let mut tool_defs_tokens: u64 = 0;
    for d in state.registry.list() {
        if let Ok(json) = serde_json::to_string(&d) {
            tool_defs_tokens += (json.chars().count() as u64) / 4;
        }
    }

    // 窗口占用：真实「最近一次请求输入 token」优先；重启后为 0 时用内容估算兜底，
    // 保证历史会话也能看到有意义的占用百分比。
    let sys_total = sys_prompt_tokens + tool_defs_tokens;
    let est_total = msg_tokens + tool_tokens + file_tokens + sys_total;
    let window_tokens = s.last_input_tokens.max(est_total).min(window_limit);

    serde_json::json!({
        "windowTokens": window_tokens,
        "windowLimit": window_limit,
        "pct": pct_of(window_tokens),
        "totalInput": s.total_input_tokens,
        "totalOutput": s.total_output_tokens,
        "totalTokens": s.total_input_tokens + s.total_output_tokens,
        "messages": s.messages.len(),
        "compactions": s.compacted_count,
        "sys": [pct_of(sys_prompt_tokens), pct_of(tool_defs_tokens)],
        "usr": [pct_of(msg_tokens), pct_of(tool_tokens), pct_of(file_tokens)],
    })
}

/// 导出会话为 Markdown 文件（对标大厂"导出对话"），返回保存路径。
#[tauri::command]
pub fn session_export(state: State<'_, AppState>, session_id: String) -> Result<String, String> {
    let s = state.sessions.get_or_create(&session_id);
    let mut md = String::new();
    md.push_str(&format!("# ClawDesk 对话导出\n\n- 会话 ID：`{session_id}`\n- 导出时间：{}\n\n---\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
    for m in &s.messages {
        let role = match m.role {
            crate::llm::Role::User => "👤 用户",
            crate::llm::Role::Assistant => "🤖 ClawDesk",
            crate::llm::Role::System => "⚙️ 系统",
            crate::llm::Role::Tool => "🔧 工具",
        };
        md.push_str(&format!("## {role}\n\n{}\n\n", m.content));
    }
    if s.messages.is_empty() {
        md.push_str("（空会话）\n");
    }
    let dir = crate::executors::builtin::attachment::attach_dir()?;
    let safe_id: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let path = dir.join(format!("export_{}_{}.md", chrono::Local::now().format("%Y%m%d_%H%M%S"), safe_id));
    std::fs::write(&path, md).map_err(|e| format!("导出写入失败: {e}"))?;
    eprintln!("[EXPORT] 会话 {session_id} 已导出 → {}", path.display());
    Ok(path.to_string_lossy().to_string())
}

/// 跨会话关键词搜索历史消息（对标大厂"搜索对话"），返回 { sessionId, role, content, time }。
#[tauri::command]
pub fn session_search(state: State<'_, AppState>, keyword: String) -> Vec<serde_json::Value> {
    if keyword.trim().is_empty() {
        return Vec::new();
    }
    state.sessions.search(keyword.trim())
}

/// Fork 分支会话（§十二.2）：完整拷贝源会话记忆，返回新分支会话。
#[tauri::command]
pub fn agent_fork(state: State<'_, AppState>, source_id: String, new_id: String) -> Option<crate::llm::session::AgentSession> {
    let branch = state.sessions.fork(&source_id, new_id.clone())?;
    eprintln!("[FORK] {} -> {}", source_id, new_id);
    Some(branch)
}

/// 查询会话断点状态（§十二.1）：是否有可续跑的断点。
#[tauri::command]
pub fn agent_checkpoint(state: State<'_, AppState>, session_id: String) -> Option<crate::llm::session::AgentCheckpoint> {
    state.sessions.load_checkpoint(&session_id)
}

/// 列出指定父会话的全部分支会话 ID（侧边栏标注从属关系）。
#[tauri::command]
pub fn agent_branches(state: State<'_, AppState>, parent_id: String) -> Vec<String> {
    state.sessions.branches_of(&parent_id)
}
