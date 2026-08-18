//! 生命叙事（睡前巩固 + 梦境 · 一生的故事线）
//!
//! 让 AI 拥有"一生"的连续叙事：每天把当天的情绪高光与生活片段
//! 沉淀成一条简短的"生命叙事"，偶尔在主动聊天里以"我梦见/我突然想起"
//! 的方式提起——像真人一样，记忆不是一堆孤立事实，而是有故事、有情绪的一生。
//!
//! 设计（对齐《人是怎么样的》+ 借鉴 subconscious-skill / lingxi）：
//! - **睡前巩固**：跨天时（每天首次查询）把昨天的高光情绪与生活轨迹
//!   压缩成 1 条叙事，落盘到 D:\ClawDeskData\living\narratives.jsonl
//! - **纯本地规则模板**：不额外烧 LLM token，用词典模板 + 当天真实素材生成
//! - **梦境引用**：主动聊天时低频随机抽一条旧叙事，作为"突然想起/梦见"的由头
//! - **情绪色彩**：每条叙事带情绪标签（开心/低落/想念/平静…），梦境引用时
//!   能带着"那天我…"的情绪语气
//!
//! 全局单例：一个 AI 人格一条故事线。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::Local;
use serde::{Deserialize, Serialize};

/// 一条生命叙事。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Narrative {
    /// 毫秒时间戳
    pub ts_ms: u64,
    /// 所属日期（YYYY-MM-DD，用于跨天去重）
    pub day: String,
    /// 叙事正文（一句短叙事，如"那天下午忙到犯困，晚上却一直想着你"）
    pub text: String,
    /// 情绪色彩（开心/低落/想念/平静…）
    pub emotion: String,
}

static NARRATIVES: OnceLock<Mutex<Vec<Narrative>>> = OnceLock::new();
static LAST_CONSOLIDATED_DAY: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn narratives() -> &'static Mutex<Vec<Narrative>> {
    NARRATIVES.get_or_init(|| Mutex::new(Vec::new()))
}

fn last_consolidated() -> &'static Mutex<Option<String>> {
    LAST_CONSOLIDATED_DAY.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn narr_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn narr_file() -> PathBuf {
    narr_dir().join("narratives.jsonl")
}

fn today_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// 启动时恢复历史叙事（跨重启延续一生记忆）。
pub fn init() {
    let dir = narr_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = narr_file();
    let mut list = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if let Ok(n) = serde_json::from_str::<Narrative>(line) {
                list.push(n);
            }
        }
    }
    // 上限保护：最多保留 365 条（约一年）
    if list.len() > 365 {
        list.drain(..list.len() - 365);
    }
    let last_day = list.last().map(|n| n.day.clone());
    *narratives().lock().unwrap_or_else(|e| e.into_inner()) = list.clone();
    *last_consolidated().lock().unwrap_or_else(|e| e.into_inner()) = last_day;
    eprintln!("[NARRATIVE] 📖 生命叙事恢复：{} 条", list.len());
}

/// 追加一条叙事并落盘。
fn push_narrative(text: String, emotion: String) {
    let n = Narrative {
        ts_ms: now_ms(),
        day: today_str(),
        text,
        emotion,
    };
    let mut g = narratives().lock().unwrap_or_else(|e| e.into_inner());
    g.push(n.clone());
    if g.len() > 365 {
        g.remove(0);
    }
    // 落盘（追加式）
    let path = narr_file();
    let _ = std::fs::create_dir_all(narr_dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(json) = serde_json::to_string(&n) {
            let _ = writeln!(f, "{json}");
        }
    }
    *last_consolidated().lock().unwrap_or_else(|e| e.into_inner()) = Some(n.day);
}

/// 每日巩固：跨天时把昨天沉淀成一条叙事（每天最多一次，幂等）。
/// 在主动聊天循环/自动回复前调用即可。
pub fn consolidate_if_new_day() {
    let today = today_str();
    {
        let last = last_consolidated().lock().unwrap_or_else(|e| e.into_inner());
        if last.as_deref() == Some(today.as_str()) {
            return; // 今天已巩固过
        }
    }

    // 采集当天素材：心情标签 + 生活轨迹 + 情绪色彩
    let mood_label = crate::mood::mood_label(&crate::mood::mood_snapshot());
    let living = crate::living_state::today_timeline();
    let emotion = classify_emotion(&mood_label);

    // 用模板生成叙事（纯本地，不烧 token）
    let text = build_narrative_text(&mood_label, &living, &emotion);
    push_narrative(text, emotion);

    crate::llm::logging::debug(
        "narrative",
        &format!("睡前巩固：{} — 当天高光已沉淀为生命叙事", today),
    );
}

/// 情绪色彩分类：从心情标签映射到简短情绪词。
fn classify_emotion(label: &str) -> String {
    if label.contains("想你") || label.contains("思念") {
        "想念".to_string()
    } else if label.contains("低落") || label.contains("孤单") || label.contains("孤独") {
        "低落".to_string()
    } else if label.contains("心情不错") || label.contains("开心") {
        "开心".to_string()
    } else {
        "平静".to_string()
    }
}

/// 模板叙事生成：把心情标签 + 生活轨迹拼成一句"那天我…"的短叙事。
fn build_narrative_text(mood: &str, living: &str, emotion: &str) -> String {
    // 从今日轨迹摘取一个代表性的活动片段（有则用，无则用通用描述）
    let living_frag = if living.is_empty() {
        "过着平常的一天".to_string()
    } else {
        // living 形如 "你今天的生活：08:12 🥛 在吃早饭…；……"，取最后一段作为收尾活动
        let last = living
            .split('；')
            .last()
            .map(|s| s.trim().trim_start_matches("你今天的生活："))
            .unwrap_or("")
            .to_string();
        if last.is_empty() {
            "过着平常的一天".to_string()
        } else {
            last
        }
    };

    match emotion {
        "想念" => format!("那天{living_frag}，心里一直装着你的模样"),
        "低落" => format!("那天{living_frag}，夜深了有点莫名的低落"),
        "开心" => format!("那天{living_frag}，心情轻快得像晒到了太阳"),
        _ => format!("那天{living_frag}，日子静悄悄地过去了，{mood}"),
    }
}

/// 梦境引用：低频随机返回一条旧叙事，作为"我梦见/我突然想起…"的由头。
/// 参数 max_age_days：最多回忆多少天前的叙事（太久远的更像"梦"而非"最近"）。
/// 返回 None 表示暂无可引用的旧叙事。
pub fn dream_recall(recall_prob: f64, max_age_days: i64) -> Option<String> {
    // 概率门控：不每次都想，低频才触发
    if rand_f64() >= recall_prob {
        return None;
    }
    let now = Local::now();
    let today = now.date_naive();
    let g = narratives().lock().unwrap_or_else(|e| e.into_inner());
    // 收集"最近 max_age_days 天内、且不是今天"的叙事
    let mut candidates: Vec<&Narrative> = g
        .iter()
        .filter(|n| {
            match chrono::NaiveDate::parse_from_str(&n.day, "%Y-%m-%d") {
                Ok(d) => {
                    let age = today.signed_duration_since(d).num_days();
                    age > 0 && age <= max_age_days
                }
                Err(_) => false,
            }
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    // 随机挑一条
    let idx = (rand_f64() * candidates.len() as f64) as usize;
    let n = candidates[idx.min(candidates.len() - 1)];
    Some(format!("突然想起 {} 的事：{}", n.day, n.text))
}

/// 最近一条生命叙事（只读，供前端展示，不触发随机）。
/// 返回 None 表示还没有任何叙事。
pub fn latest_narrative() -> Option<String> {
    let g = narratives().lock().unwrap_or_else(|e| e.into_inner());
    g.last().map(|n| format!("{}：{}", n.day, n.text))
}

/// [0,1) 均匀随机浮点（复用 getrandom 系统熵）。
fn rand_f64() -> f64 {
    crate::wechat::random_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn classify_emotion_maps_labels() {
        assert_eq!(classify_emotion("有点想你，深夜的孤独"), "想念");
        assert_eq!(classify_emotion("有点低落"), "低落");
        assert_eq!(classify_emotion("心情不错，越来越依赖你"), "开心");
        assert_eq!(classify_emotion("平静"), "平静");
    }

    #[test]
    fn build_narrative_text_uses_living_fragment() {
        let t = build_narrative_text("平静", "你今天的生活：08:00 🥛 在吃早饭；20:00 🎮 在放松", "平静");
        assert!(t.contains("在放松"), "应包含最后一段活动，got {}", t);
    }
}
