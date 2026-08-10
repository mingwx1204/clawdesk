//! 引擎全局配置单例（API Key 仅内存态，不落盘、不打印）。
//!
//! 写入：`harness_set_model_config` / `harness_start_task` 命令。
//! 读取：`llm/runner.rs` 的 `run_agent_loop`（方案B替换版）。

use std::sync::{OnceLock, RwLock};

use super::param::ReasoningEffort;

/// 引擎配置。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub effort: ReasoningEffort,
}

static ENGINE_CONFIG: OnceLock<RwLock<Option<EngineConfig>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<EngineConfig>> {
    ENGINE_CONFIG.get_or_init(|| RwLock::new(None))
}

/// 写入引擎配置（覆盖）。
pub fn set_engine_config(cfg: EngineConfig) {
    if let Ok(mut slot) = slot().write() {
        *slot = Some(cfg);
    }
}

/// 读取引擎配置快照（未配置返回 None）。
pub fn engine_config() -> Option<EngineConfig> {
    slot().read().ok().and_then(|s| s.clone())
}
