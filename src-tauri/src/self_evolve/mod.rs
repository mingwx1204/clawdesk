//! 自进化系统 —— AI 驱动的技能自动学习与优化闭环。
//!
//! 流程（参考用户的设计图）：
//!   1. 任务触发 → 分析任务类型（脚本/工具/工作流）
//!   2. 技能生成 → AI 自动创建 SKILL.md（含功能描述 + 使用条件）
//!   3. 自动注册 → 保存到技能库目录，系统自动加载
//!   4. 执行验证 → 执行并记录成功率
//!   5. 反馈学习 → 更新技能权重，优化选择策略
//!
//! 子模块：
//! - generator：AI 驱动生成 SKILL.md
//! - tracker：执行追踪 + 成功率统计
//! - selector：加权技能选择（高频、高成功率优先）

pub mod generator;
pub mod tracker;
pub mod selector;

use std::sync::{Arc, Mutex, OnceLock};

use crate::core::tool::registry::ToolRegistry;

/// 自进化引擎单例。
pub struct SelfEvolveEngine {
    /// 技能生成器（依赖 LLM 客户端）。
    pub generator: generator::SkillGenerator,
    /// 执行追踪器。
    pub tracker: Arc<tracker::SkillTracker>,
    /// 已从自进化生成的技能 ID 集合（避免重复生成）。
    pub generated_ids: Mutex<std::collections::HashSet<String>>,
    /// 是否已启用。
    pub enabled: std::sync::atomic::AtomicBool,
}

/// 全局引擎实例（启动时初始化，多线程安全）。
static ENGINE: OnceLock<SelfEvolveEngine> = OnceLock::new();

impl SelfEvolveEngine {
    /// 初始化引擎。
    pub fn init(api_key: String, base_url: String, model: String, tracker_path: std::path::PathBuf) -> &'static Self {
        ENGINE.get_or_init(|| {
            let generator = generator::SkillGenerator::new(api_key, base_url, model);
            let tracker = Arc::new(tracker::SkillTracker::new(tracker_path));
            Self {
                generator,
                tracker,
                generated_ids: Mutex::new(std::collections::HashSet::new()),
                enabled: std::sync::atomic::AtomicBool::new(false),
            }
        })
    }

    /// 获取全局引擎实例。
    pub fn get() -> Option<&'static Self> {
        ENGINE.get()
    }

/// ★ 从持久化设置自动恢复引擎（重启后设置里已开启、但内存引擎为空的场景）：
///   读取 AppState 的 settings（selfEvolveEnabled / selfEvolveModel / 端点）
///   与 keys（真实 API Key，不再用占位符）初始化引擎，并同步 enabled 状态。
///   引擎已初始化时仅同步 enabled（幂等，不重复创建）。
pub fn ensure_engine_from_settings(
    state: &crate::commands::AppState,
) -> Result<&'static Self, String> {
    let s = state.settings.get();
    let keys = state.settings.keys();
    // 用真实主 Key（无 Key 则无法生成技能）
    let api_key = if keys.main.is_empty() {
        return Err("未配置 API Key，无法启用自进化（请在「设置 → 模型 API」填写）".to_string());
    } else {
        keys.main.clone()
    };
    let base_url = crate::commands::endpoint_base_url(&s.model_endpoint);
    let model = if s.self_evolve_model.is_empty() {
        s.model.clone()
    } else {
        s.self_evolve_model.clone()
    };
    let tracker_path = crate::llm::settings::clawdesk_dir().join("self_evolve_tracker.json");
    let engine = SelfEvolveEngine::init(api_key, base_url, model, tracker_path);
    engine
        .enabled
        .store(s.self_evolve_enabled, std::sync::atomic::Ordering::Relaxed);
    Ok(engine)
}

    /// 启动自进化循环。
    pub async fn evolve(&self, registry: &ToolRegistry) -> Result<generator::EvolveReport, String> {
        if !self.enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("自进化未启用".into());
        }
        let candidates = self.tracker.get_evolution_candidates();
        if candidates.is_empty() {
            return Ok(generator::EvolveReport::empty());
        }
        let report = self.generator.generate_skills(&candidates).await?;
        let mut registered = 0usize;
        for skill_def in &report.generated_skills {
            if let Err(e) = self.generator.save_and_register(registry, skill_def) {
                eprintln!("[SELF_EVOLVE] 注册失败: {e}");
            } else {
                let mut ids = self.generated_ids.lock().unwrap();
                ids.insert(skill_def.name.clone());
                registered += 1;
            }
        }
        eprintln!("[SELF_EVOLVE] 本轮生成 {} 个技能（注册 {} 个）", report.generated_skills.len(), registered);
        Ok(report)
    }

    /// 记录一次工具执行结果。
    #[allow(dead_code)]
    pub fn record_execution(tool_id: &str, success: bool, elapsed_ms: u64, user_reverted: bool) {
        if let Some(engine) = Self::get() {
            engine.tracker.record(tool_id, success, elapsed_ms, user_reverted);
        }
    }
}

// ═══════════════════════════════════════════
// IPC 命令（供前端设置页调用）
// ═══════════════════════════════════════════

/// 启用/禁用自进化。
#[tauri::command]
pub fn self_evolve_enable(api_key: String, base_url: String, model: String, enabled: bool) -> Result<serde_json::Value, String> {
    let tracker_path = crate::llm::settings::clawdesk_dir().join("self_evolve_tracker.json");
    let engine = SelfEvolveEngine::init(api_key, base_url, model, tracker_path);
    engine.enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
    eprintln!("[SELF_EVOLVE] 自进化: {}", if enabled { "已启用" } else { "已禁用" });
    Ok(serde_json::json!({ "enabled": enabled }))
}

/// 手动触发一次进化（前端按钮调用）。
#[tauri::command]
pub async fn self_evolve_run(
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<serde_json::Value, String> {
    // ★ 引擎未初始化时先从持久化设置自动恢复（重启后无需重新开关一次）
    let engine = match SelfEvolveEngine::get() {
        Some(e) => e,
        None => SelfEvolveEngine::ensure_engine_from_settings(&state)?,
    };
    if !engine.enabled.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("自进化未启用（设置中未开启）".into());
    }
    let report = engine.evolve(&state.registry).await?;
    Ok(serde_json::to_value(&report).map_err(|e| format!("序列化报告失败: {e}"))?)
}

/// 查询自进化状态（工具排名 + 最近进化的技能）。
#[tauri::command]
pub fn self_evolve_status(
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<serde_json::Value, String> {
    // ★ 引擎未初始化时自动恢复（状态查询也支持重启后直接可用）
    let engine = match SelfEvolveEngine::get() {
        Some(e) => e,
        None => SelfEvolveEngine::ensure_engine_from_settings(&state)?,
    };
    let ranking = engine.tracker.get_ranking();
    let generated: Vec<String> = engine.generated_ids.lock().unwrap().iter().cloned().collect();
    Ok(serde_json::json!({
        "enabled": engine.enabled.load(std::sync::atomic::Ordering::Relaxed),
        "totalTracked": ranking.len(),
        "generatedSkills": generated,
        "ranking": ranking.iter().take(20).map(|(id, s)| {
            serde_json::json!({
                "toolId": id,
                "total": s.total,
                "successRate": format!("{:.1}%", s.success_rate() * 100.0),
                "avgMs": s.avg_elapsed_ms as u64,
            })
        }).collect::<Vec<_>>(),
    }))
}

/// 获取加权工具排名（供 system prompt 注入）。
#[tauri::command]
pub fn self_evolve_ranking(
    state: tauri::State<'_, crate::commands::AppState>,
) -> Result<serde_json::Value, String> {
    let engine = match SelfEvolveEngine::get() {
        Some(e) => e,
        None => SelfEvolveEngine::ensure_engine_from_settings(&state)?,
    };
    let sel = selector::WeightedSelector::new(engine.tracker.clone(), selector::SelectorConfig::default());
    let top = sel.top_tools(15);
    Ok(serde_json::json!({
        "promptHint": sel.prompt_hint(15),
        "tools": top.iter().map(|t| serde_json::json!({
            "toolId": t.tool_id,
            "score": format!("{:.2}", t.score),
            "uses": t.total_uses,
            "rate": format!("{:.0}%", t.success_rate * 100.0),
            "avgMs": t.avg_ms,
        })).collect::<Vec<_>>(),
    }))
}
