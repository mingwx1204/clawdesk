//! Agent 运行进度事件 + 取消令牌 + 逐步确认通道。
//!
//! - 进度事件经 `ProgressSink` 回调推送给前端（Tauri emit 由命令层封装）；
//! - 取消令牌为 `Arc<AtomicBool>`，运行循环每步检查，用户可中断；
//! - `CancelRegistry` 管理运行中任务的取消令牌与逐步确认通道；
//! - 离线测试使用 `null_progress` 空回调。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::Value;

/// 取消令牌：线程安全的取消标志。
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 请求取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// 取消令牌注册表：管理运行中 Agent 任务的取消（按 run_id）与逐步确认。
#[derive(Default)]
pub struct CancelRegistry {
    tokens: RwLock<HashMap<String, CancellationToken>>,
    /// 逐步确认通道：call_id → 一次性应答（approve: bool）。
    confirms: Mutex<HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
}

impl CancelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个新令牌（run_id 唯一），返回令牌引用。
    pub fn create(&self, run_id: String) -> CancellationToken {
        let token = CancellationToken::new();
        self.tokens
            .write()
            .unwrap()
            .insert(run_id, token.clone());
        token
    }

    /// 登记一个待确认调用，返回接收端（runner 等待应答）。
    pub fn create_confirm(&self, call_id: String) -> tokio::sync::oneshot::Receiver<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.confirms.lock().unwrap().insert(call_id, tx);
        rx
    }

    /// 应答确认（StepConfirm 模式）；调用不存在返回 false。
    pub fn resolve_confirm(&self, call_id: &str, approve: bool) -> bool {
        if let Some(tx) = self.confirms.lock().unwrap().remove(call_id) {
            let _ = tx.send(approve);
            true
        } else {
            false
        }
    }

    /// 取消指定运行任务；不存在返回 false。
    pub fn cancel(&self, run_id: &str) -> bool {
        let token = self.tokens.read().unwrap().get(run_id).cloned();
        match token {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// 移除令牌（任务结束后清理）。
    pub fn remove(&self, run_id: &str) {
        self.tokens.write().unwrap().remove(run_id);
    }

    /// 清理全部待确认（任务结束时兜底，避免悬挂）。
    pub fn clear_confirms(&self) {
        self.confirms.lock().unwrap().clear();
    }

    /// 当前运行中任务数。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.tokens.read().unwrap().len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 单次工具调用的进度载荷。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallProgress {
    pub round: usize,
    pub tool_id: String,
    pub arguments: Value,
    pub status: String,
    pub output: Value,
}

/// Agent 运行进度事件（前端实时展示）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentProgress {
    /// 新一轮开始。
    RoundStarted { round: usize },
    /// 模型本轮文本。
    ModelText { round: usize, text: String },
    /// 一次工具调用完成。
    ToolCall(ToolCallProgress),
    /// 上下文压缩发生。
    Compaction {
        kept: usize,
        summary_chars: usize,
        compacted_count: usize,
    },
    /// 逐步确认模式：等待用户批准一次工具调用。
    ///
    /// 载荷包含：call_id（应答用）、tool_id、风险等级（riskLevel）与
    /// 完整入参（arguments），供前端弹窗展示"工具名称 / 执行参数 / 风险等级"
    /// （优化提示词三③）。
    #[serde(rename_all = "camelCase")]
    ConfirmRequired {
        call_id: String,
        tool_id: String,
        risk_level: crate::middleware::risk::RiskLevel,
        arguments: Value,
    },
    /// 用户取消。
    Cancelled,
    /// 循环结束。
    Finished {
        final_text: String,
        used_rounds: usize,
        truncated: bool,
    },
}

/// 进度回调签名（同步；Tauri emit 由命令层封装为闭包）。
pub type ProgressSink = Box<dyn Fn(&AgentProgress) + Send + Sync>;

/// 空进度回调（离线测试 / 无 AppHandle 场景）。
#[allow(dead_code)]
pub fn null_progress() -> ProgressSink {
    Box::new(|_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_token_switches_state() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_token_shared_across_clones() {
        let token = CancellationToken::new();
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn registry_create_cancel_remove() {
        let reg = CancelRegistry::new();
        let token = reg.create("run-1".into());
        assert_eq!(reg.len(), 1);
        assert!(!token.is_cancelled());

        assert!(reg.cancel("run-1"));
        assert!(token.is_cancelled());
        assert!(!reg.cancel("run-2"));

        reg.remove("run-1");
        assert!(reg.is_empty());
    }

    #[test]
    fn confirm_channel_roundtrip() {
        let reg = CancelRegistry::new();
        let rx = reg.create_confirm("call-1".into());
        assert!(reg.resolve_confirm("call-1", true));
        let result = rx.blocking_recv().unwrap();
        assert!(result);
        assert!(!reg.resolve_confirm("call-1", false));
    }

    #[test]
    fn progress_events_serialize_with_tag() {
        let ev = AgentProgress::ToolCall(ToolCallProgress {
            round: 1,
            tool_id: "builtin:get_time".into(),
            arguments: serde_json::json!({}),
            status: "success".into(),
            output: serde_json::json!({ "ok": true }),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "toolCall");
        assert_eq!(v["toolId"], "builtin:get_time");

        let ev2 = AgentProgress::ConfirmRequired {
            call_id: "c".into(),
            tool_id: "builtin:window_close".into(),
            risk_level: crate::middleware::risk::RiskLevel::High,
            arguments: serde_json::json!({ "x": 1 }),
        };
        let v2 = serde_json::to_value(&ev2).unwrap();
        assert_eq!(v2["type"], "confirmRequired");
        assert_eq!(v2["toolId"], "builtin:window_close");
        assert_eq!(v2["riskLevel"], "high");
        assert_eq!(v2["arguments"]["x"], 1);
    }

    #[test]
    fn null_progress_is_noop() {
        let sink = null_progress();
        sink(&AgentProgress::Cancelled);
    }
}
