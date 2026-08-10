//! 上下文管理 + Compacting context 压缩 —— 从 CodeWhale tui/src/compaction.rs 剥离重构。
//!
//! 内置 BUG 修复：
//!   1. 压缩 LLM 调用独立超时 120s（COMPACTION_TIMEOUT_SECS）；
//!   2. 后台协程执行压缩，主调度循环只轮询状态，不再被卡死；
//!   3. LLM 压缩失败/超时 → 降级为粗暴截断，保留最近 4 条历史。
//! 阶段九：对齐原版 tui 内核，压缩时"钉住"涉及工作集路径/错误/补丁的消息（不吞关键上下文）。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::RwLock;

use super::client::LlmClient;
use super::param::ModelParams;

/// 压缩 LLM 调用的独立超时（秒）。
pub const COMPACTION_TIMEOUT_SECS: u64 = 120;
/// 触发压缩的 token 估算阈值。
pub const COMPACTION_TOKEN_THRESHOLD: u64 = 40_000;
/// 压缩后保留的最近消息条数（降级截断也用它）。
pub const KEEP_RECENT_MESSAGES: usize = 4;
/// 摘要输入最大字符数。
const SUMMARY_INPUT_MAX_CHARS: usize = 24_000;
/// 摘要输出最大 token。
const SUMMARY_MAX_TOKENS: u32 = 1024;

// ── 阶段九：工作集路径钉住常量（对齐原版 RECENT_WORKING_SET_WINDOW / MAX_WORKING_SET_PATHS）──
/// 工作集路径推导：扫描最近多少条消息。
const RECENT_WORKING_SET_WINDOW: usize = 8;
/// 工作集路径上限。
const MAX_WORKING_SET_PATHS: usize = 32;

/// 压缩状态机。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStatus {
    Idle,
    Running,
    Completed,
    FallbackTrimmed,
}

/// 移除"悬空"的 tool 消息（双向配对清理）。
///
/// 截断/压缩后可能破坏 tool 配对组，DeepSeek 会报 HTTP 400，两种形态：
/// 1. `Messages with role 'tool' must be a response to a preceding message with 'tool_calls'`
///    —— 悬空 tool：前置 assistant(tool_calls) 被截掉 / 段首就是 tool 消息 → 丢弃该 tool。
/// 2. `An assistant message with 'tool_calls' must be followed by tool messages
///    responding to each 'tool_call_id'` —— assistant 带 tool_calls 但其部分/全部 tool
///    响应被截掉 → 整组删除该 assistant（及其残留 tool 响应），避免留下未响应的 tool_calls。
pub fn prune_dangling_tools(messages: &mut Vec<Value>) {
    let old = std::mem::take(messages);
    let mut out: Vec<Value> = Vec::with_capacity(old.len());
    let mut i = 0usize;
    while i < old.len() {
        let role = old[i]["role"].as_str().unwrap_or("");

        // 悬空 tool：无前置 assistant(tool_calls) 支撑 → 丢弃
        if role == "tool" {
            i += 1;
            continue;
        }

        // assistant 带 tool_calls：检查其 tool 响应是否完整
        if role == "assistant" && old[i].get("tool_calls").is_some() {
            let declared: Vec<String> = old[i]["tool_calls"]
                .as_array()
                .map(|tcs| {
                    tcs.iter()
                        .filter_map(|tc| tc["id"].as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            // 向后收集紧接着的 tool 响应（直到下一条非 tool 消息）
            let mut j = i + 1;
            let mut responded: Vec<String> = Vec::new();
            while j < old.len() && old[j]["role"].as_str() == Some("tool") {
                if let Some(id) = old[j]["tool_call_id"].as_str() {
                    responded.push(id.to_string());
                }
                j += 1;
            }

            // 每个声明的 tool_call_id 都必须有响应，否则整组删除 assistant（含残留响应）
            let complete = declared.iter().all(|id| responded.contains(id));
            if complete {
                out.push(old[i].clone());
                out.extend_from_slice(&old[i + 1..j]);
            }
            i = j;
            continue;
        }

        out.push(old[i].clone());
        i += 1;
    }
    *messages = out;
}

/// 降级截断：保留最近 `keep` 条消息，插入截断标记（模块级函数，供 turn_loop 直接调用）。
pub fn fallback_trim(mut messages: Vec<Value>, keep: usize) -> Vec<Value> {
    if messages.len() <= keep {
        return messages;
    }
    let split = messages.len() - keep;
    messages.insert(
        split,
        serde_json::json!({
            "role": "system",
            "content": "[上下文已截断：早期消息被移除以控制 token 预算。必要时请向用户确认上下文。]"
        }),
    );
    // ★ 修复：截断后清理悬空 tool 消息（前置 assistant tool_calls 可能被截掉）
    prune_dangling_tools(&mut messages);
    messages
}

// ══════════════════════════════════════════════════════════════
// 阶段九：工作集路径钉住机制（对齐原版 tui/src/compaction.rs 语义）
// ══════════════════════════════════════════════════════════════

/// 从文本中提取类路径片段（含 `/` 或 `\`、非 URL、非纯数字、长度 >= 4）。
fn extract_paths_from_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '\\' && c != '.' && c != '-' && c != '_' && c != ':');
        if cleaned.len() < 4 {
            continue;
        }
        if cleaned.contains("://") || cleaned.starts_with("http://") || cleaned.starts_with("https://") {
            continue;
        }
        if cleaned.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '_' || c == ':') {
            continue;
        }
        if cleaned.contains('/') || cleaned.contains('\\') {
            out.push(cleaned.to_string());
        }
    }
    out.sort();
    out.dedup();
    out.truncate(MAX_WORKING_SET_PATHS);
    out
}

/// 是否为工作集路径：含源码目录标记或常见文件扩展名。
fn is_working_set_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    const DIR_MARKERS: &[&str] = &[
        "/src/", "\\src\\", "/crates/", "\\crates\\", "/tests/", "\\tests\\",
        "/config/", "\\config\\", "c:\\", "d:\\", "/workspace/", "/home/",
    ];
    if DIR_MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    const EXT_MARKERS: &[&str] = &[
        ".rs", ".toml", ".json", ".ts", ".vue", ".md", ".py", ".js", ".css", ".html", ".dll", ".exe",
    ];
    EXT_MARKERS.iter().any(|e| lower.contains(e))
}

/// 推导工作集路径：扫描最近 RECENT_WORKING_SET_WINDOW 条消息。
fn derive_working_set_paths(messages: &[Value]) -> HashSet<String> {
    let mut set = HashSet::new();
    let start = messages.len().saturating_sub(RECENT_WORKING_SET_WINDOW);
    for m in &messages[start..] {
        let content = m["content"].as_str().unwrap_or("");
        for path in extract_paths_from_text(content) {
            if is_working_set_path(&path) {
                set.insert(path);
            }
            if set.len() >= MAX_WORKING_SET_PATHS {
                return set;
            }
        }
    }
    set
}

/// 判断消息是否钉住（不参与摘要，原样保留）——完全对齐原版三规则。
fn should_pin_message(content: &str, working_set_paths: &HashSet<String>) -> bool {
    // 规则1：提及任何工作集路径
    if working_set_paths.iter().any(|p| content.contains(p.as_str())) {
        return true;
    }
    let lower = content.to_lowercase();
    // 规则2：错误标记
    const ERROR_MARKERS: &[&str] = &[
        "error:", "error ", "failed", "panic", "traceback", "stack trace",
        "assertion failed", "test failed",
    ];
    if ERROR_MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    // 规则3：补丁标记
    const PATCH_MARKERS: &[&str] = &[
        "diff --git", "+++ b/", "--- a/", "*** begin patch", "*** update file:",
        "*** add file:", "*** delete file:", "```diff", "apply_patch",
    ];
    PATCH_MARKERS.iter().any(|m| lower.contains(m))
}

// ══════════════════════════════════════════════════════════════

/// 上下文管理器。
pub struct ContextManager {
    client: Option<LlmClient>,
    params: ModelParams,
    status: Arc<RwLock<CompactionStatus>>,
    summary: Arc<RwLock<Option<String>>>,
    /// 后台压缩任务句柄。
    task: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl ContextManager {
    pub fn new(client: Option<LlmClient>) -> Self {
        Self {
            client,
            params: ModelParams {
                model: "deepseek-chat".to_string(),
                max_tokens: Some(SUMMARY_MAX_TOKENS),
                temperature: Some(0.3),
                ..Default::default()
            },
            status: Arc::new(RwLock::new(CompactionStatus::Idle)),
            summary: Arc::new(RwLock::new(None)),
            task: Arc::new(RwLock::new(None)),
        }
    }

    /// 简易 token 估算：字符数 / 3。
    pub fn estimate_tokens(messages: &[Value]) -> u64 {
        let total: usize = messages
            .iter()
            .map(|m| serde_json::to_string(m).unwrap_or_default().chars().count())
            .sum();
        (total / 3) as u64
    }

    /// 是否需要压缩（已 Running 时返回 false，避免重复触发）。
    pub async fn should_compact(&self, messages: &[Value]) -> bool {
        if *self.status.read().await == CompactionStatus::Running {
            return false;
        }
        Self::estimate_tokens(messages) > COMPACTION_TOKEN_THRESHOLD
    }

    /// 异步触发压缩：立即返回，后台协程执行，不阻塞主调度循环。
    pub async fn compact_async(&self, messages: Vec<Value>) -> bool {
        {
            let mut status = self.status.write().await;
            if *status == CompactionStatus::Running {
                return false;
            }
            *status = CompactionStatus::Running;
        }

        let Some(client) = self.client.clone() else {
            *self.status.write().await = CompactionStatus::FallbackTrimmed;
            return false;
        };

        let params = self.params.clone();
        let status = self.status.clone();
        let summary = self.summary.clone();

        let handle = tokio::spawn(async move {
            let fut = async {
                let input = build_summary_input(&messages);
                let msgs = vec![serde_json::json!({
                    "role": "user",
                    "content": format!(
                        "对以下对话生成结构化摘要（≤800词），覆盖：主要目标/关键决策/待办任务/工作区状态。\
                         代码与路径保持原样。\n\n--- 对话 ---\n{input}\n--- 结束 ---"
                    ),
                })];
                client
                    .chat_once(&params, &msgs, Some("你是精确的对话摘要器，只输出 Markdown 摘要。"))
                    .await
            };
            match tokio::time::timeout(Duration::from_secs(COMPACTION_TIMEOUT_SECS), fut).await {
                Ok(Ok(text)) => {
                    *summary.write().await = Some(text);
                    *status.write().await = CompactionStatus::Completed;
                }
                Ok(Err(e)) => {
                    tracing::warn!(target: "engine.context", "压缩 LLM 失败: {e} —— 降级截断");
                    *status.write().await = CompactionStatus::FallbackTrimmed;
                }
                Err(_) => {
                    tracing::warn!(target: "engine.context", "压缩超时 {}s —— 降级截断", COMPACTION_TIMEOUT_SECS);
                    *status.write().await = CompactionStatus::FallbackTrimmed;
                }
            }
        });

        *self.task.write().await = Some(handle);
        true
    }

    /// 等待后台压缩结束并返回状态（主循环轮询用）。
    pub async fn wait_for_compaction(&self) -> CompactionStatus {
        loop {
            let st = *self.status.read().await;
            if st != CompactionStatus::Running {
                return st;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// 压缩摘要文本（Completed 后可取）。
    pub async fn summary_text(&self) -> Option<String> {
        self.summary.read().await.clone()
    }

    /// 用压缩摘要替换历史：保留最近 N 条 ∪ 钉住消息（阶段九）。
    pub fn apply_summary(messages: Vec<Value>, summary: &str) -> Vec<Value> {
        // 1) 推导工作集路径
        let working_set = derive_working_set_paths(&messages);

        // 2) 收集"钉住"消息：最近 KEEP_RECENT_MESSAGES 条 或 should_pin_message == true
        let mut pinned: Vec<Value> = Vec::new();
        let keep = messages.len().min(KEEP_RECENT_MESSAGES);
        let recent_start = messages.len() - keep;
        for (i, m) in messages.iter().enumerate() {
            let content = m["content"].as_str().unwrap_or("");
            if i >= recent_start || should_pin_message(content, &working_set) {
                pinned.push(m.clone());
            }
        }

        // 3) 组装：摘要 system 消息 + 钉住消息（保持原有相对顺序）
        let mut out = vec![serde_json::json!({
            "role": "system",
            "content": format!("[会话摘要(自动生成)]\n{summary}")
        })];
        out.extend(pinned);
        // ★ 修复：钉住段可能以 tool 开头 / 前置 assistant(tool_calls) 被截 → 清理悬空 tool
        prune_dangling_tools(&mut out);
        out
    }
}

/// 构建摘要输入：取头部 + 尾部，中间省略，总量受控。
fn build_summary_input(messages: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut total = 0usize;
    let head = messages.len().saturating_mul(3) / 5;
    let tail_start = messages.len().saturating_sub(head / 2);

    for m in messages.iter().take(head) {
        let s = format_message(m);
        if total + s.len() > SUMMARY_INPUT_MAX_CHARS * 3 / 5 {
            parts.push("[...早期消息省略...]".to_string());
            break;
        }
        total += s.len();
        parts.push(s);
    }
    parts.push("\n--- 近期 ---\n".to_string());
    for m in messages.iter().skip(tail_start) {
        let s = format_message(m);
        if total + s.len() > SUMMARY_INPUT_MAX_CHARS {
            break;
        }
        total += s.len();
        parts.push(s);
    }
    parts.join("\n")
}

/// 单条消息 → 摘要文本。
fn format_message(m: &Value) -> String {
    let role = m["role"].as_str().unwrap_or("unknown");
    let content = match m.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str().or_else(|| b["content"].as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let truncated: String = content.chars().take(800).collect();
    format!("[{role}] {truncated}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> Value {
        serde_json::json!({ "role": role, "content": content })
    }

    #[test]
    fn should_pin_on_working_set_path_mention() {
        let ws: HashSet<String> = ["C:\\workspace\\foo.rs".to_string()].into_iter().collect();
        assert!(should_pin_message("读取了 C:\\workspace\\foo.rs 内容", &ws));
    }

    #[test]
    fn should_pin_on_error_marker() {
        let ws = HashSet::new();
        assert!(should_pin_message("build failed with error: cannot find crate", &ws));
        assert!(should_pin_message("panic: index out of bounds", &ws));
    }

    #[test]
    fn should_pin_on_patch_marker() {
        let ws = HashSet::new();
        assert!(should_pin_message("diff --git a/src/lib.rs b/src/lib.rs", &ws));
        assert!(should_pin_message("+++ b/src/main.rs", &ws));
    }

    #[test]
    fn should_not_pin_plain_text() {
        let ws = HashSet::new();
        assert!(!should_pin_message("今天天气不错，继续推进任务。", &ws));
    }

    #[test]
    fn apply_summary_keeps_pinned_messages() {
        let messages = vec![
            msg("user", "你好"),
            serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "id": "call_1", "type": "function", "function": { "name": "builtin__read_file", "arguments": "{}" } }],
            }),
            serde_json::json!({
                "role": "tool",
                "content": "读取 C:\\workspace\\foo.rs 成功",
                "tool_call_id": "call_1",
            }),
            msg("user", "继续"),
        ];
        let out = ContextManager::apply_summary(messages, "摘要内容");
        // 摘要 system 在前
        assert_eq!(out[0]["role"], "system");
        // 含工作集路径的 tool 消息被钉住保留（且与 assistant(tool_calls) 配对，不会被 prune）
        let kept = out.iter().any(|m| {
            m["content"]
                .as_str()
                .map(|s| s.contains("C:\\workspace\\foo.rs"))
                .unwrap_or(false)
        });
        assert!(kept, "工作集路径消息应被钉住保留");
    }
}
