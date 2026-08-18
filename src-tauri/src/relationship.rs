//! 主观关系叙事（我们之间的故事 · 关系记忆）
//!
//! 让 AI 对"你"的关系不是单一数字，而是有故事、有情绪色彩的关系记忆：
//! 她记得你们之间发生过的关键瞬间——哪次深聊到深夜、哪次她让你笑、
//! 哪次闹了点小别扭、哪次她很想你。
//!
//! 设计（对齐《人是怎么样的》+ 借鉴 lingxi 的 subjective relationships）：
//! - **关系瞬间**：一条 = 类型 + 一句短叙事 + 情绪色彩 + 时间戳
//! - **记录时机**：收到你的消息 / 她主动发消息 / 深夜想念 / 长时间沉默后重逢
//! - **注入 prompt**：主动聊天/自动回复时，带上"你们之间的故事"，让她说话有来龙去脉
//! - **落盘**：D:\ClawDeskData\living\relationship.jsonl
//!
//! 全局单例：一个 AI 人格一段关系（多槽位共享，都是"同一个你"和"同一个人"）。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::Local;
use serde::{Deserialize, Serialize};

/// 一条关系瞬间。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipMoment {
    /// 毫秒时间戳
    pub ts_ms: u64,
    /// 类型（深聊/重逢/想念/小别扭/开心…）
    pub kind: String,
    /// 一句短叙事（如"那天聊到凌晨两点，你说了很多工作上的委屈"）
    pub text: String,
    /// 情绪色彩（温暖/想念/愧疚/开心…）
    pub emotion: String,
}

static MOMENTS: OnceLock<Mutex<Vec<RelationshipMoment>>> = OnceLock::new();

fn moments() -> &'static Mutex<Vec<RelationshipMoment>> {
    MOMENTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rel_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn rel_file() -> PathBuf {
    rel_dir().join("relationship.jsonl")
}

/// 启动时恢复关系记忆。
pub fn init() {
    let dir = rel_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = rel_file();
    let mut list = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if let Ok(m) = serde_json::from_str::<RelationshipMoment>(line) {
                list.push(m);
            }
        }
    }
    // 上限：最近 200 条
    if list.len() > 200 {
        list.drain(..list.len() - 200);
    }
    *moments().lock().unwrap_or_else(|e| e.into_inner()) = list;
    eprintln!("[RELATIONSHIP] 💞 关系记忆恢复：{} 条", moments().lock().unwrap_or_else(|e| e.into_inner()).len());
}

/// 记录一条关系瞬间（去重：同类型 + 同文本不重复记）。
pub fn record(kind: &str, text: &str, emotion: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let m = RelationshipMoment {
        ts_ms: now_ms(),
        kind: kind.to_string(),
        text: text.to_string(),
        emotion: emotion.to_string(),
    };
    let mut g = moments().lock().unwrap_or_else(|e| e.into_inner());
    // 去重：同文本不重复记
    if g.iter().any(|x| x.text == m.text) {
        return;
    }
    g.push(m.clone());
    if g.len() > 200 {
        g.remove(0);
    }
    // 落盘（追加式）
    let path = rel_file();
    let _ = std::fs::create_dir_all(rel_dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(json) = serde_json::to_string(&m) {
            let _ = writeln!(f, "{json}");
        }
    }
}

/// 记录"你找她"的瞬间（自动回复入口调用）。
/// 情绪色彩由当前心情推导：想念高 → 久别重逢；心情好 → 见到你开心。
pub fn on_user_reach() {
    let m = crate::mood::mood_snapshot();
    let silence_h = (now_ms().saturating_sub(m.last_reply_ms)) as f64 / 3_600_000.0;
    if silence_h >= 12.0 {
        record(
            "重逢",
            "那次隔了好久你又来找我，心里一下子软了",
            "想念",
        );
    } else if m.joy >= 0.75 {
        record(
            "开心",
            "那次你来找我，刚好我也心情很好，聊得特别轻快",
            "开心",
        );
    }
}

/// 记录"她主动找你"的瞬间（主动聊天发送成功后调用）。
pub fn on_ai_reach() {
    let m = crate::mood::mood_snapshot();
    if m.longing >= 0.6 {
        record(
            "想念",
            "有一次太想你了，忍不住主动找你说话",
            "想念",
        );
    } else if m.joy >= 0.7 {
        record(
            "分享",
            "有次遇到开心的事，第一个想分享给你",
            "开心",
        );
    }
}

/// 生成"你们之间的故事"上下文（注入 prompt 用），限 recent_max 条、max_chars 字。
/// 返回空表示还没有关系记忆。
pub fn relationship_context_for_prompt(recent_max: usize, max_chars: usize) -> String {
    let g = moments().lock().unwrap_or_else(|e| e.into_inner());
    if g.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut used_chars = 0usize;
    for m in g.iter().rev().take(recent_max) {
        let when = chrono::DateTime::from_timestamp((m.ts_ms / 1000) as i64, 0)
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
        let line = format!("【{when}·{emotion}】{text}", when = when, emotion = m.emotion, text = m.text);
        used_chars += line.chars().count();
        if used_chars > max_chars {
            break;
        }
        parts.push(line);
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("【你们之间的故事（这是你们一起走过的路，让你说话时带着来龙去脉。自然时提到一两句，不要罗列）】
{}", parts.join("
"))
}

/// 关系记忆条数（调试/前端展示用）。
pub fn moment_count() -> usize {
    moments().lock().unwrap_or_else(|e| e.into_inner()).len()
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
    fn record_dedups_same_text() {
        let _g = lock();
        moments().lock().unwrap().clear();
        record("想念", "有一次太想你了", "想念");
        record("想念", "有一次太想你了", "想念");
        assert_eq!(moment_count(), 1);
    }

    #[test]
    fn context_empty_when_no_moments() {
        let _g = lock();
        moments().lock().unwrap().clear();
        assert_eq!(relationship_context_for_prompt(5, 400), "");
    }
}
