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

fn default_kind() -> String {
    "daily".to_string()
}

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
    /// 叙事种类："daily" = 每日巩固，"topic_cluster" = 话题聚类结果。
    /// 旧数据无此字段 → serde 用 default 补 "daily"（向前兼容）。
    #[serde(default = "default_kind")]
    pub kind: String,
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
    *last_consolidated()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = last_day;
    eprintln!("[NARRATIVE] 📖 生命叙事恢复：{} 条", list.len());
}

/// 追加一条叙事并落盘。
fn push_narrative(text: String, emotion: String) {
    let n = Narrative {
        ts_ms: now_ms(),
        day: today_str(),
        text,
        emotion,
        kind: "daily".to_string(),
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
    *last_consolidated()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(n.day);
}

/// 每日巩固：跨天时把昨天沉淀成一条叙事（每天最多一次，幂等）。
/// 在主动聊天循环/自动回复前调用即可。
pub fn consolidate_if_new_day() {
    let today = today_str();
    {
        let last = last_consolidated()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

/// 情绪/活动关键词表（纯本地规则，不烧 LLM）：
/// 从叙事文本中提取这些词的共现频率，聚合出"最近生活的主题"。
const TOPIC_KEYWORDS: &[(&str, &str)] = &[
    ("想你", "想你"), ("惦记", "惦记"), ("想念", "想念"),
    ("低落", "低落"), ("孤单", "孤单"), ("孤独", "孤独"),
    ("开心", "开心"), ("轻快", "开心"), ("开心地", "开心"),
    ("工作", "工作"), ("忙", "忙碌"), ("加班", "忙碌"),
    ("电影", "观影"), ("剧", "观影"), ("综艺", "娱乐"),
    ("歌", "音乐"), ("音乐", "音乐"),
    ("游戏", "游戏"), ("打游戏", "游戏"), ("排位", "游戏"),
    ("吃", "吃"), ("饭", "吃"), ("火锅", "吃"), ("奶茶", "吃"), ("咖啡", "吃"),
    ("睡", "睡眠"), ("失眠", "睡眠"), ("熬夜", "睡眠"), ("觉", "睡眠"),
    ("书", "书"), ("读", "读书"), ("写作", "写作"),
    ("散步", "散步"), ("跑步", "运动"), ("健身", "运动"), ("锻炼", "运动"),
    ("旅行", "旅行"), ("出差", "旅行"), ("出门", "出门"),
    ("家里", "家"), ("回家", "家"), ("猫", "猫"), ("狗", "宠物"),
    ("雨", "天气"), ("晴", "天气"), ("冷", "天气"), ("热", "天气"),
];

/// 话题聚类（本地关键字共现，不烧 token）：
/// 扫描最近 `window_days` 天的 "daily" 叙事，统计情绪/活动词的出现频率，
/// 返回一个"最近生活的主题"摘要。这是 soul-protocol "dream cycle → topic
/// clustering" 的纯本地子集——让梦境引用/突然想起有跨时间的连贯感。
///
/// 返回 None 表示叙事太少或没有高置信主题。
pub fn topic_clusters(window_days: i64) -> Option<String> {
    use std::collections::HashMap;
    let today = Local::now().date_naive();
    let g = narratives().lock().unwrap_or_else(|e| e.into_inner());
    let recent: Vec<&Narrative> = g
        .iter()
        .filter(|n| {
            n.kind == "daily"
                && chrono::NaiveDate::parse_from_str(&n.day, "%Y-%m-%d")
                    .map(|d| {
                        let age = today.signed_duration_since(d).num_days();
                        age >= 0 && age <= window_days
                    })
                    .unwrap_or(false)
        })
        .collect();
    // 至少 3 条叙事才有聚类意义（样本太少只是噪声）
    if recent.len() < 3 {
        return None;
    }
    let mut freq: HashMap<&str, usize> = HashMap::new();
    for n in recent.iter() {
        for (kw, topic) in TOPIC_KEYWORDS {
            if n.text.contains(kw) {
                *freq.entry(topic).or_insert(0) += 1;
            }
        }
    }
    // 情绪也可能形成主题（连续几天低落/开心）
    let mut emotion_freq: HashMap<&str, usize> = HashMap::new();
    for n in recent.iter() {
        if !n.emotion.is_empty() {
            *emotion_freq.entry(n.emotion.as_str()).or_insert(0) += 1;
        }
    }
    // 取出现 ≥2 次的主题，按频次排序
    let mut topics: Vec<(&str, usize)> = freq
        .iter()
        .filter(|(_, c)| **c >= 2)
        .map(|(t, c)| (*t, *c))
        .collect();
    if let Some((e, c)) = emotion_freq.iter().max_by_key(|(_, c)| *c) {
        if *c >= 2 {
            topics.push((*e, *c));
        }
    }
    topics.sort_by(|a, b| b.1.cmp(&a.1));
    if topics.is_empty() {
        return None;
    }
    let top: Vec<&str> = topics.iter().take(3).map(|(t, _)| *t).collect();
    Some(format!(
        "最近 {window_days} 天里，你的生活反复出现这些主题：{}",
        top.join("、")
    ))
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
    let candidates: Vec<&Narrative> = g
        .iter()
        .filter(
            |n| match chrono::NaiveDate::parse_from_str(&n.day, "%Y-%m-%d") {
                Ok(d) => {
                    let age = today.signed_duration_since(d).num_days();
                    age > 0 && age <= max_age_days
                }
                Err(_) => false,
            },
        )
        .collect();
    if candidates.is_empty() {
        return None;
    }
    // ★ 30% 概率返回"最近生活的主题"（话题聚类），70% 返回具体某天的事——
    //   真人回忆有时是"最近总在牵挂什么"，有时是"突然想起那天"。
    if rand_f64() < 0.30 {
        if let Some(cluster) = topic_clusters(14) {
            return Some(cluster);
        }
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
    #[allow(dead_code)]
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    #[allow(dead_code)]
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SERIAL
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
        let t = build_narrative_text(
            "平静",
            "你今天的生活：08:00 🥛 在吃早饭；20:00 🎮 在放松",
            "平静",
        );
        assert!(t.contains("在放松"), "应包含最后一段活动，got {}", t);
    }

    #[test]
    fn topic_clusters_detects_recurring_theme() {
        let _g = lock();
        // 清空全局叙事，注入 3 条重复"游戏/吃"的叙事
        {
            let mut list = narratives().lock().unwrap_or_else(|e| e.into_inner());
            list.clear();
            let today = today_str();
            for _ in 0..3 {
                list.push(Narrative {
                    ts_ms: now_ms(),
                    day: today.clone(),
                    text: "那天打游戏排位连跪，后来点了奶茶吃".to_string(),
                    emotion: "平静".to_string(),
                    kind: "daily".to_string(),
                });
            }
        }
        let cluster = topic_clusters(7);
        assert!(cluster.is_some(), "应有聚类结果");
        let s = cluster.unwrap();
        assert!(s.contains("游戏") || s.contains("吃"), "应包含高频主题，got {}", s);
    }
}
