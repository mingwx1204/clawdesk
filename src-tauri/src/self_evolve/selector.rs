//! 加权技能选择器 —— 根据频率 × 成功率给工具排序，
//! 供 LLM system prompt 注入推荐工具列表，引导 AI 优先使用高效工具。
//!
//! 权重公式：score = total_uses × success_rate（兼顾使用频率与质量）。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::tracker::SkillTracker;

/// 选择器配置。
#[derive(Debug, Clone)]
pub struct SelectorConfig {
    /// 最小使用次数（低于此值的工具不计入排名）。
    pub min_uses: u64,
    /// 最少返回数量。
    pub min_count: usize,
    /// 最近使用衰减系数（越久没用的工具权重越低）。
    pub decay_days: f64,
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            min_uses: 2,
            min_count: 5,
            decay_days: 7.0,
        }
    }
}

/// 加权技能选择器。
pub struct WeightedSelector {
    tracker: Arc<SkillTracker>,
    config: SelectorConfig,
}

impl WeightedSelector {
    pub fn new(tracker: Arc<SkillTracker>, config: SelectorConfig) -> Self {
        Self { tracker, config }
    }

    /// 获取当前推荐排序的工具列表（用于注入 system prompt）。
    pub fn top_tools(&self, limit: usize) -> Vec<ScoredTool> {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0) as f64;

        let ranking = self.tracker.get_ranking();
        let mut scored: Vec<ScoredTool> = ranking
            .into_iter()
            .filter(|(_, s)| s.total >= self.config.min_uses)
            .map(|(id, s)| {
                let base_score = (s.total as f64) * s.success_rate();
                // 时间衰减
                let days_since = (now_secs - s.last_used_ts as f64) / 86400.0;
                let decay = if self.config.decay_days > 0.0 {
                    (1.0 - (days_since / self.config.decay_days).min(0.9)).max(0.1)
                } else {
                    1.0
                };
                ScoredTool {
                    tool_id: id,
                    score: base_score * decay,
                    total_uses: s.total,
                    success_rate: s.success_rate(),
                    avg_ms: s.avg_elapsed_ms as u64,
                }
            })
            .collect();

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit.max(self.config.min_count));
        scored
    }

    /// 生成 system prompt 注入片段（"推荐工具排行榜"）。
    pub fn prompt_hint(&self, limit: usize) -> String {
        let top = self.top_tools(limit);
        if top.is_empty() {
            return String::new();
        }
        let mut s = String::from("\n\n## 工具推荐（自进化）\n以下工具经过追踪验证，使用频率高且成功率好，遇到对应任务时优先调用：\n");
        for (i, t) in top.iter().enumerate() {
            s.push_str(&format!(
                "{}. `{}` — 使用 {} 次，成功率 {:.0}%（平均 {}ms）\n",
                i + 1,
                t.tool_id,
                t.total_uses,
                t.success_rate * 100.0,
                t.avg_ms,
            ));
        }
        s.push_str("如果你有更合适的工具，优先选择成功率高的。\n");
        s
    }
}

/// 加权后的工具条目。
#[derive(Debug, Clone)]
pub struct ScoredTool {
    pub tool_id: String,
    pub score: f64,
    pub total_uses: u64,
    pub success_rate: f64,
    pub avg_ms: u64,
}
