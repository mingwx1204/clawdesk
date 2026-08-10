//! CodeWhale crates/core 部分移植（ClawDesk 副本版）。
//!
//! 裁剪说明（禁移依赖 config/execpolicy/mcp 无法在 ClawDesk 侧移植）：
//! - 保留：纯类型层（JobStatus/JobRecord/JobRetryMetadata/JobHistoryEntry/InitialHistory/NewThread）
//!   与 `job_record_to_agent_run` 映射（仅依赖已移植的 harness::protocol）；
//! - 剔除：`JobManager` / `ThreadManager` / `Runtime` 及依赖
//!   codewhale_config / codewhale_execpolicy / codewhale_mcp 的全部代码
//!   （headless 服务端层，由 ClawDesk AppState + commands 层承担等价职责）。

pub mod turn_loop;

use std::path::PathBuf;

// ── 类型层（原样保留）──────────────────────────────────────────

/// 后台任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// Waiting to be picked up.
    Queued,
    /// Currently executing.
    Running,
    /// Temporarily paused.
    Paused,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled by the user.
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
    pub fn is_paused(&self) -> bool {
        matches!(self, Self::Paused)
    }
}

/// 任务重试元数据。
#[derive(Debug, Clone)]
pub struct JobRetryMetadata {
    /// Current attempt number (0 = not yet retried).
    pub attempt: u32,
    /// Maximum number of retry attempts before giving up.
    pub max_attempts: u32,
    /// Base delay in milliseconds for exponential backoff.
    pub backoff_base_ms: u64,
    /// Computed delay in milliseconds until the next retry.
    pub next_backoff_ms: u64,
    /// Timestamp when the next retry should be attempted.
    pub next_retry_at: Option<i64>,
}

impl Default for JobRetryMetadata {
    fn default() -> Self {
        Self {
            attempt: 0,
            max_attempts: 3,
            backoff_base_ms: 500,
            next_backoff_ms: 0,
            next_retry_at: None,
        }
    }
}

/// 任务历史条目。
#[derive(Debug, Clone)]
pub struct JobHistoryEntry {
    /// Timestamp when this entry was recorded.
    pub at: i64,
    /// Phase name (e.g., "created", "running", "failed").
    pub phase: String,
    /// Job status at this point in time.
    pub status: JobStatus,
    /// Progress percentage at this point, if available.
    pub progress: Option<u8>,
    /// Human-readable detail message.
    pub detail: Option<String>,
    /// Retry state snapshot at this point.
    pub retry: JobRetryMetadata,
}

/// 完整任务记录。
#[derive(Debug, Clone)]
pub struct JobRecord {
    /// Unique job identifier.
    pub id: String,
    /// Human-readable job name.
    pub name: String,
    /// Current job status.
    pub status: JobStatus,
    /// Current progress percentage (0-100).
    pub progress: Option<u8>,
    /// Human-readable detail about the current state.
    pub detail: Option<String>,
    /// Retry state for failed jobs.
    pub retry: JobRetryMetadata,
    /// Chronological history of state transitions.
    pub history: Vec<JobHistoryEntry>,
    /// Timestamp when the job was created.
    pub created_at: i64,
    /// Timestamp of the last state change.
    pub updated_at: i64,
}

/// 新线程/续跑参数。
#[derive(Debug, Clone)]
pub enum InitialHistory {
    /// Start with an empty conversation.
    New,
    /// Forked from an existing thread with the given history items.
    Forked(Vec<serde_json::Value>),
    /// Resumed from a persisted thread with its full history.
    Resumed {
        conversation_id: String,
        history: Vec<serde_json::Value>,
        rollout_path: PathBuf,
    },
}

/// 新建线程结果。
#[derive(Debug, Clone)]
pub struct NewThread {
    /// The thread metadata.
    pub thread: crate::harness::protocol::Thread,
    /// Resolved model identifier.
    pub model: String,
    /// Provider that serves the model.
    pub model_provider: String,
    /// Working directory for the thread.
    pub cwd: PathBuf,
    /// Approval policy override, if any.
    pub approval_policy: Option<String>,
    /// Sandbox mode override, if any.
    pub sandbox: Option<String>,
}

/// 将持久化任务记录映射为依赖中立的运行读取模型。
///
/// Pure projection of the record as persisted: unknown budgets stay unset and
/// nothing is fabricated. `updated_at` (epoch seconds) provides the terminal
/// timestamp because the job manager records no separate end time.
#[must_use]
pub fn job_record_to_agent_run(
    record: &JobRecord,
) -> crate::harness::protocol::agent_run::AgentRunSnapshot {
    use crate::harness::protocol::agent_run::{
        AgentRunSnapshot, BudgetSummary, RunSource, RunState, TerminalOutcome, TerminalSummary,
    };

    let (state, terminal) = match record.status {
        JobStatus::Queued => (RunState::Queued, None),
        JobStatus::Running => (RunState::Running, None),
        JobStatus::Paused => (RunState::Paused, None),
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => {
            let outcome = match record.status {
                JobStatus::Completed => TerminalOutcome::Completed,
                JobStatus::Failed => TerminalOutcome::Failed,
                _ => TerminalOutcome::Cancelled,
            };
            (
                RunState::Terminal,
                Some(TerminalSummary {
                    outcome,
                    ended_at_ms: record.updated_at.checked_mul(1000),
                    detail: None,
                }),
            )
        }
    };

    AgentRunSnapshot {
        run_id: record.id.clone(),
        parent: None,
        source: RunSource::CoreJob,
        state,
        budget: BudgetSummary::default(),
        terminal,
        refs: Vec::new(),
    }
}
