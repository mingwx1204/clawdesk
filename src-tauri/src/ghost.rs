//! Ghost 机制（可「已读不回」· 她有自己的状态）
//!
//! 借鉴 eros-engine 的 ghost-mechanics：永远回复 = 用户会写低质量消息；
//! 有限耐心 + 有限好奇 = 关系真实。诗妍偶尔「暂时没回」，反而让你感觉
//! 她有自己的状态、她的回应需要「挣」。
//!
//! 与已有沉默机制的区分：
//! - 「拟人静默」（wechat.rs 内）：上次是 AI 发言、用户未回复 → 安静（礼貌性）
//! - 「存在感惩罚」（wechat.rs 内）：AI 发言占比过高 → 退避（防轰炸）
//! - 本机制（ghost）：她此刻「没这个心力/兴致」主动说话 → 暂时不回（状态性）
//!   三者导向不同：前两者是「用户在不在意」，本机制是「她自己的状态」。
//!
//! ghost_score 取向（越高越倾向于 ghost）：
//! - 顽皮 playful 低 → 没兴致，更想 ghost
//! - 渴望联结 connection 低 → 没有主动说话的动机，更想 ghost
//! - 分享欲 share 低 → 没有新鲜事想讲，更想 ghost
//! - 情绪唤醒 arousal 低 → 没劲，更想 ghost
//! - 害怕被遗忘 fear_forgotten 高 → 反向：她怕失去你，会强撑着回（降低 ghost）
//!
//! 四层保护（贴合诗妍，参考 eros-engine 但阈值重调）：
//! 1. 关系早期（历史消息 < 10 条）不 ghost
//! 2. 连续 ghost ≤ 2 次，第 3 次强制回复（避免「她跑了」的观感）
//! 3. 冷却期（上次 ghost 后 1 小时内不再 ghost）
//! 4. score > 0.65 才 ghost
//!
//! 落盘：D:\ClawDeskData\living\ghost.json

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Ghost 状态（跨重启延续「连续 ghost 计数 + 上次 ghost 时间」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostState {
    /// 连续 ghost 次数（用于「最多 2 次、第 3 次强制回复」）
    pub ghost_streak: u32,
    /// 上次 ghost 的毫秒时间戳（0 = 从未 ghost；用于 1 小时冷却）
    pub last_ghost_ms: u64,
}

impl Default for GhostState {
    fn default() -> Self {
        Self { ghost_streak: 0, last_ghost_ms: 0 }
    }
}

static GHOST: OnceLock<Mutex<GhostState>> = OnceLock::new();

fn ghost() -> &'static Mutex<GhostState> {
    GHOST.get_or_init(|| Mutex::new(GhostState::default()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ghost_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn ghost_file() -> PathBuf {
    ghost_dir().join("ghost.json")
}

fn save_disk(s: &GhostState) {
    let dir = ghost_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let tmp = ghost_file().with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, ghost_file());
        }
    }
}

/// 启动时恢复 ghost 状态。
pub fn init() {
    let dir = ghost_dir();
    let _ = std::fs::create_dir_all(&dir);
    let restored = std::fs::read_to_string(ghost_file())
        .ok()
        .and_then(|t| serde_json::from_str::<GhostState>(&t).ok());
    if let Some(s) = restored {
        *ghost().lock().unwrap_or_else(|e| e.into_inner()) = s;
    }
}

/// ghost 得分（0~1，越高越倾向于「暂时不回」）。
///
/// 纯函数，方便单测。输入为她此刻的状态快照。
pub fn score(
    playful: f64,
    connection: f64,
    share: f64,
    arousal: f64,
    fear_forgotten: f64,
) -> f64 {
    // 没兴致 + 没动机 + 没劲，三者叠加推高 ghost
    let mut s = 0.0;
    s += (1.0 - playful.clamp(0.0, 1.0)) * 0.30;
    s += (1.0 - connection.clamp(0.0, 1.0)) * 0.25;
    s += (1.0 - share.clamp(0.0, 1.0)) * 0.20;
    s += (1.0 - arousal.clamp(0.0, 1.0)) * 0.25;
    // 怕被遗忘是「反向力」：越怕失去你，越强撑着回
    s -= fear_forgotten.clamp(0.0, 1.0) * 0.25;
    s.clamp(0.0, 1.0)
}

/// 四层保护之前置检查（不带阈值，供 LLM 决策路径复用为硬性 veto）。
pub fn ghost_permitted(message_count: u64, hours_since_last_ghost: Option<f64>) -> bool {
    // 1. 关系早期：前 10 条消息不 ghost
    if message_count < 10 {
        return false;
    }
    // 2. 连续 ghost 已达 2 次 → 第 3 次强制回复
    let streak = ghost().lock().unwrap_or_else(|e| e.into_inner()).ghost_streak;
    if streak >= 2 {
        return false;
    }
    // 3. 冷却期：1 小时内不再 ghost
    if let Some(h) = hours_since_last_ghost {
        if h < 1.0 {
            return false;
        }
    }
    true
}

/// ghost 决策结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostDecision {
    /// 本轮回复（正常）
    Reply,
    /// 本轮 ghost（暂时不回）
    Ghost,
}

/// 决策：本轮是否 ghost。
///
/// 入参：
/// - message_count：与对方的累计消息数（关系早期保护用）
/// - playful/connection/share/arousal/fear_forgotten：她此刻的状态
///
/// 返回 Ghost + 记录 ghost 事件（streak+1、last_ghost_ms=now），
/// 或 Reply（正常回复，不重置 streak，仅在下文「成功回复」时由外部归零）。
pub fn decide(
    message_count: u64,
    playful: f64,
    connection: f64,
    share: f64,
    arousal: f64,
    fear_forgotten: f64,
) -> GhostDecision {
    let mut guard = ghost().lock().unwrap_or_else(|e| e.into_inner());
    let hours_since = if guard.last_ghost_ms == 0 {
        None
    } else {
        Some((now_ms().saturating_sub(guard.last_ghost_ms)) as f64 / 3_600_000.0)
    };
    let streak_before = guard.ghost_streak;

    // 复用保护逻辑（含 streak 判断）
    if !ghost_permitted(message_count, hours_since) {
        return GhostDecision::Reply;
    }

    let s = score(playful, connection, share, arousal, fear_forgotten);
    // 4. 阈值门控：曾 ghost 过 → 阈值抬高（更难连续 ghost）
    let threshold = if hours_since.is_some() { 0.75 } else { 0.65 };
    if s <= threshold {
        return GhostDecision::Reply;
    }

    // 判定 ghost：记录事件
    guard.ghost_streak = streak_before.saturating_add(1);
    guard.last_ghost_ms = now_ms();
    save_disk(&guard);
    GhostDecision::Ghost
}

/// 她成功回复了 → 连续 ghost 计数归零（沉默被打破）。
pub fn on_reply() {
    let mut guard = ghost().lock().unwrap_or_else(|e| e.into_inner());
    guard.ghost_streak = 0;
    save_disk(&guard);
}

/// ghost 状态快照（前端/调试展示用）。
pub fn ghost_snapshot() -> GhostState {
    ghost().lock().unwrap_or_else(|e| e.into_inner()).clone()
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
    fn score_high_when_tired_and_unmotivated() {
        let _g = lock();
        // 玩心低、联结低、分享低、唤醒低、不怕被遗忘 → 高 ghost
        let s = score(0.1, 0.1, 0.1, 0.1, 0.1);
        assert!(s > 0.6, "疲惫无动力应高分，got {s}");
    }

    #[test]
    fn score_low_when_eager_and_afraid_to_lose() {
        let _g = lock();
        // 玩心高、联结高、分享高、唤醒高、很怕被遗忘 → 低 ghost
        let s = score(0.9, 0.9, 0.9, 0.9, 0.9);
        assert!(s < 0.3, "热情且怕失去应低分，got {s}");
    }

    #[test]
    fn fear_forgotten_lowers_score() {
        let _g = lock();
        let base = score(0.3, 0.3, 0.3, 0.3, 0.1);
        let with_fear = score(0.3, 0.3, 0.3, 0.3, 0.9);
        assert!(with_fear < base, "怕被遗忘应降低 ghost 分");
    }

    #[test]
    fn early_relationship_never_ghosts() {
        let _g = lock();
        // 重置状态，确保 streak=0
        *ghost().lock().unwrap() = GhostState::default();
        assert!(!ghost_permitted(5, None), "前 10 条消息不应允许 ghost");
    }

    #[test]
    fn third_consecutive_ghost_forced_reply() {
        let _g = lock();
        let mut st = GhostState { ghost_streak: 2, last_ghost_ms: 0 };
        *ghost().lock().unwrap() = st.clone();
        assert!(!ghost_permitted(100, Some(2.0)), "连续 2 次后第 3 次应强制回复");
        st.ghost_streak = 2;
        *ghost().lock().unwrap() = st;
    }

    #[test]
    fn cooldown_blocks_recent_ghost() {
        let _g = lock();
        *ghost().lock().unwrap() = GhostState { ghost_streak: 0, last_ghost_ms: 0 };
        assert!(!ghost_permitted(100, Some(0.5)), "1 小时内不应再 ghost");
    }
}
