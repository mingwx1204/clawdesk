//! 技能执行追踪器 —— 记录每次工具调用的成功/失败/耗时/撤销，
//! 为自进化提供数据基础。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 单次执行记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRecord {
    pub tool_id: String,
    pub timestamp: u64, // UNIX 秒
    pub success: bool,
    pub elapsed_ms: u64,
    pub user_reverted: bool, // 用户撤销了结果
}

/// 每个工具的累计统计。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolStats {
    pub total: u64,
    pub success: u64,
    pub reverted: u64,
    pub avg_elapsed_ms: f64,
    pub last_used_ts: u64,
}

impl ToolStats {
    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.success as f64 / self.total as f64
        }
    }

    /// 是否需要进化（成功率低于阈值 + 使用次数≥最小样本）。
    pub fn needs_evolution(&self, min_samples: u64, min_rate: f64) -> bool {
        self.total >= min_samples && self.success_rate() < min_rate
    }
}

/// 技能执行追踪器（线程安全，10 分钟自动落盘）。
pub struct SkillTracker {
    stats: Mutex<HashMap<String, ToolStats>>,
    records: Mutex<Vec<ExecRecord>>,
    path: PathBuf,
}

impl SkillTracker {
    pub fn new(path: PathBuf) -> Self {
        let state = Self::load_from(&path);
        Self {
            stats: Mutex::new(state.0),
            records: Mutex::new(state.1),
            path,
        }
    }

    /// 记录一次工具执行。
    #[allow(dead_code)]
    pub fn record(&self, tool_id: &str, success: bool, elapsed_ms: u64, user_reverted: bool) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // 记录
        {
            let mut recs = self.records.lock().unwrap();
            recs.push(ExecRecord {
                tool_id: tool_id.to_string(),
                timestamp: now,
                success,
                elapsed_ms,
                user_reverted,
            });
            if recs.len() > 10_000 {
                recs.drain(0..2000); // 定期裁剪，保留最近 8000 条
            }
        }

        // 更新统计
        {
            let mut stats = self.stats.lock().unwrap();
            let s = stats.entry(tool_id.to_string()).or_default();
            s.total += 1;
            if success {
                s.success += 1;
            }
            if user_reverted {
                s.reverted += 1;
            }
            s.avg_elapsed_ms = (s.avg_elapsed_ms * ((s.total - 1) as f64) + elapsed_ms as f64) / s.total as f64;
            s.last_used_ts = now;
        }

        // 每 50 条自动落盘
        if self.records.lock().unwrap().len() % 50 == 0 {
            let _ = self.flush();
        }
    }

    /// 获取需要进化的工具列表（成功率低于 60% 且至少执行了 5 次）。
    pub fn get_evolution_candidates(&self) -> Vec<(String, ToolStats)> {
        let stats = self.stats.lock().unwrap();
        stats
            .iter()
            .filter(|(_, s)| s.needs_evolution(5, 0.6))
            .map(|(id, s)| (id.clone(), s.clone()))
            .collect()
    }

    /// 获取所有工具的排名（按使用频率 × 成功率排序）。
    pub fn get_ranking(&self) -> Vec<(String, ToolStats)> {
        let stats = self.stats.lock().unwrap();
        let mut v: Vec<_> = stats.iter().map(|(id, s)| (id.clone(), s.clone())).collect();
        v.sort_by(|a, b| {
            let score_a = (a.1.total as f64) * a.1.success_rate();
            let score_b = (b.1.total as f64) * b.1.success_rate();
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    /// 获取单个工具统计。
    #[allow(dead_code)]
    pub fn get_stats(&self, tool_id: &str) -> Option<ToolStats> {
        self.stats.lock().unwrap().get(tool_id).cloned()
    }

    /// 强制落盘。
    pub fn flush(&self) -> std::io::Result<()> {
        let stats = self.stats.lock().unwrap().clone();
        let records = self.records.lock().unwrap().clone();
        let data = serde_json::json!({ "stats": stats, "records": records });
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(&data).unwrap_or_default())?;
        Ok(())
    }

    /// 从文件加载。
    fn load_from(path: &PathBuf) -> (HashMap<String, ToolStats>, Vec<ExecRecord>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return (HashMap::new(), Vec::new()),
        };
        let data: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => return (HashMap::new(), Vec::new()),
        };
        let stats = data.get("stats")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let records = data.get("records")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        (stats, records)
    }

    /// 系统退出时落盘。
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        let _ = self.flush();
    }
}

impl Drop for SkillTracker {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
