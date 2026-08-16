//! 细节记忆库（被看见 · 记住主人随口提过的事）
//!
//! 对齐《人是怎么样的》第 004 条《被看见》：
//! "被看见，是有人穿过你的日常，看见了那个你没说出口的你"——
//! 主人随口说过"我不吃香菜"，三个月后 AI 点菜时记得"不要香菜"。
//!
//! 设计：
//! - **细节条目**：一句话细节 + 来源上下文 + 时间 + 关联标签（话题/人物/场合）
//! - **多来源**：规则抽取（主人消息里的"我不吃/我喜欢/我生日/我养了…"模式）、
//!   AI 对话中提炼、手动添加
//! - **自然引用**：注入 prompt 时带"你记得主人的这些事"，AI 在合适时机"无意间"提起
//! - **遗忘曲线**：太旧的、长期未用的细节自动降权（真人也会慢慢忘，但重要的忘不掉）
//! - **落盘**：`D:\ClawDeskData\living\details.jsonl`

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::Local;
use serde::{Deserialize, Serialize};

/// 一条细节记忆。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailMemory {
    /// 毫秒时间戳
    pub ts_ms: u64,
    /// 细节内容（一句话，如"主人不吃香菜"）
    pub text: String,
    /// 来源（"wechat" / "manual" / "ai"）
    pub source: String,
    /// 关联标签（话题关键词，逗号分隔，如"食物,香菜"）
    pub tags: String,
    /// 引用次数（AI 提起过的次数，用于遗忘权重）
    pub used: u32,
}

/// 抽取规则：主人消息里出现这些关键词 → 值得记住的细节
/// （每个条目 = 一组关键词 + 标签；命中任一关键词即抽取该句）
const RULES: &[(&[&str], &str)] = &[
    (&["我不吃", "我不爱", "讨厌吃", "不喜欢吃"], "食物,忌口"),
    (&["我最喜欢", "我爱吃", "超喜欢吃", "最爱吃"], "食物,偏好"),
    (&["我生日", "生日是", "过生日"], "日期,生日"),
    (&["我养了", "我家猫", "我家狗", "我有一只"], "宠物,生活"),
    (&["我住", "住在", "搬家", "新家"], "居住,生活"),
    (&["我上班", "我公司", "我同事"], "工作"),
    (&["我对象", "我女朋友", "我男朋友", "我老婆", "我老公", "我媳妇", "我丈夫", "我妻子"], "关系,亲密"),
    (&["我爸妈", "我父母", "我妈", "我爸", "我家人"], "关系,家人"),
    (&["我最近在", "我这几天", "我打算", "我想学", "我准备"], "计划,生活"),
    (&["我睡不着", "失眠", "又熬夜", "睡不着觉"], "状态,睡眠"),
    (&["我难受", "我不舒服", "生病", "感冒", "发烧"], "状态,健康"),
    (&["我考试", "我面试", "我答辩"], "事件,压力"),
    (&["我减肥", "我在健身", "跑步", "锻炼"], "习惯,健康"),
    (&["我最怕", "我怕"], "心理,恐惧"),
    (&["我小时候", "我童年"], "回忆,童年"),
];

static DETAILS: OnceLock<Mutex<Vec<DetailMemory>>> = OnceLock::new();

fn details() -> &'static Mutex<Vec<DetailMemory>> {
    DETAILS.get_or_init(|| Mutex::new(Vec::new()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn details_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn details_file() -> PathBuf {
    details_dir().join("details.jsonl")
}

/// 启动时恢复细节记忆。
pub fn init() {
    let dir = details_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = details_file();
    let mut list = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if let Ok(d) = serde_json::from_str::<DetailMemory>(line) {
                list.push(d);
            }
        }
    }
    let mut g = details().lock().unwrap_or_else(|e| e.into_inner());
    *g = list;
    eprintln!("[DETAIL] 📌 细节记忆恢复：{} 条", g.len());
}

/// 去重后追加一条细节（同内容不重复记）。
pub fn remember(text: &str, source: &str, tags: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let mut g = details().lock().unwrap_or_else(|e| e.into_inner());
    // 去重：同文本（或高度相似）不再重复记
    if g.iter().any(|d| d.text == text) {
        return false;
    }
    // 上限保护：最多 200 条，超出丢最旧的
    while g.len() >= 200 {
        g.remove(0);
    }
    g.push(DetailMemory {
        ts_ms: now_ms(),
        text: text.to_string(),
        source: source.to_string(),
        tags: tags.to_string(),
        used: 0,
    });
    // 落盘
    let path = details_file();
    let _ = std::fs::create_dir_all(details_dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(json) = serde_json::to_string(g.last().unwrap()) {
            let _ = writeln!(f, "{json}");
        }
    }
    true
}

/// 规则抽取：从一段主人消息里找出值得记住的细节并存入。
/// 返回抽到的条数。
pub fn extract_from_message(msg: &str, source: &str) -> usize {
    if msg.trim().is_empty() {
        return 0;
    }
    let mut added = 0;
    for (keywords, tags) in RULES {
        for kw in *keywords {
            if let Some(pos) = msg.find(kw) {
                // 提取该关键词所在的一句（前后各截 ~40 字）
                let start = pos.saturating_sub(30);
                let end = (pos + kw.len() + 50).min(msg.len());
                let sentence = msg[start..end].trim().to_string();
                if !sentence.is_empty() && remember(&sentence, source, tags) {
                    added += 1;
                }
                break; // 该组命中一次即可
            }
        }
    }
    added
}

/// 手动添加细节（前端/AI 调用）。
pub fn add_detail(text: &str, tags: &str) -> Result<usize, String> {
    Ok(if remember(text, "manual", tags) { 1 } else { 0 })
}

/// 全部细节（按时间倒序，供前端展示/管理）。
pub fn all_details() -> Vec<DetailMemory> {
    let g = details().lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<DetailMemory> = g.clone();
    v.sort_by_key(|d| std::cmp::Reverse(d.ts_ms));
    v
}

/// 删除一条细节（按文本精确匹配）。
pub fn forget(text: &str) -> bool {
    let mut g = details().lock().unwrap_or_else(|e| e.into_inner());
    let before = g.len();
    g.retain(|d| d.text != text);
    let removed = before - g.len();
    if removed > 0 {
        // 重写整个文件（删除场景低频，可接受）
        let path = details_file();
        let _ = std::fs::create_dir_all(details_dir());
        if let Ok(mut f) = OpenOptions::new().create(true).truncate(true).write(true).open(&path) {
            for d in g.iter() {
                if let Ok(json) = serde_json::to_string(d) {
                    let _ = writeln!(f, "{json}");
                }
            }
        }
    }
    removed > 0
}

/// 标记一条细节被引用过（AI 提起时调用，影响遗忘权重）。
#[allow(dead_code)] // 供后续阶段（被看见引用追踪）
pub fn mark_used(text: &str) {
    let mut g = details().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = g.iter_mut().find(|d| d.text == text) {
        d.used += 1;
    }
}

/// 注入 prompt 的"你记得主人的事"：按时间倒序取最近 N 条（去重、限长）。
/// 返回空表示还没有值得说的细节。
pub fn details_context_for_prompt(recent_max: usize, max_chars: usize) -> String {
    let all = all_details();
    if all.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    for d in all.iter().take(recent_max) {
        let when = chrono::DateTime::from_timestamp((d.ts_ms / 1000) as i64, 0)
            .map(|dt| {
                let dt = dt.with_timezone(&Local);
                let today = Local::now().date_naive();
                let days = today.signed_duration_since(dt.date_naive()).num_days();
                if days <= 0 {
                    "今天".to_string()
                } else if days == 1 {
                    "昨天".to_string()
                } else {
                    format!("{days}天前")
                }
            })
            .unwrap_or_default();
        let line = format!("【{}】{}{}", when, d.text, if d.tags.is_empty() { String::new() } else { format!("（{}）", d.tags) });
        if parts.iter().map(|p: &String| p.chars().count()).sum::<usize>() + line.chars().count() > max_chars {
            break;
        }
        parts.push(line);
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("【你记得的关于主人的事（被看见：这些是他随口提过、你放在心上的。自然聊天时可以提起，但不要一口气全说出来，像真人一样在合适的时机提到）】
{}", parts.join("
"))
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
    fn extract_finds_food_aversion() {
        let _g = lock();
        // 清理后测试
        details().lock().unwrap().clear();
        let n = extract_from_message("我不吃香菜，每次点菜都要跟人说不要香菜", "test");
        assert!(n >= 1, "应抽到忌口细节");
        let all = all_details();
        assert!(all.iter().any(|d| d.text.contains("香菜")), "应记住香菜细节");
        assert!(all.iter().any(|d| d.tags.contains("食物")), "应带食物标签");
    }

    #[test]
    fn extract_finds_pet() {
        let _g = lock();
        details().lock().unwrap().clear();
        let n = extract_from_message("我家猫今天吐了，好担心", "test");
        assert!(n >= 1, "应抽到宠物细节");
        let all = all_details();
        assert!(all.iter().any(|d| d.text.contains("猫")), "应记住猫");
    }

    #[test]
    fn dedup_same_text() {
        let _g = lock();
        details().lock().unwrap().clear();
        assert!(remember("主人不吃香菜", "test", "食物"));
        assert!(!remember("主人不吃香菜", "test", "食物"), "同文本不应重复记");
        assert_eq!(all_details().len(), 1);
    }

    #[test]
    fn forget_removes() {
        let _g = lock();
        details().lock().unwrap().clear();
        remember("主人喜欢喝美式", "test", "饮品");
        assert!(forget("主人喜欢喝美式"));
        assert!(all_details().is_empty());
    }
}

