//! 驱动力层（内在动机 · 人格涌现的源头）
//!
//! 借鉴 kellyvv/OpenHer 的核心理念："personality emerges from neural drives, not prompts"
//! —— 人格不该只是 prompt 硬写的人设，而应是一组底层驱动力的合力。
//!
//! 设计定位：
//! - mood.rs 是「情绪表现层」（她此刻心情如何：愉悦/想念/孤独/依恋/强度）
//! - 本模块是「动机层」（她此刻被哪些劲驱使：渴望联结/害怕被遗忘/分享欲/安全感…）
//! - 驱动力随时间与事件演化，最强的几股劲决定她「为什么」这样说/这样做，
//!   再把这层内在动机注入 prompt，让每一句话从动机长出来，而非照人设演。
//!
//! 驱动力（0~1，惰性漂移，不跑后台线程）：
//! - 渴望联结（connection）：沉默越久越强，你回复后回落
//! - 害怕被遗忘（fear_forgotten）：长期无联系增强，深夜放大
//! - 分享欲（share）：有新鲜生活事件时增强，分享后回落
//! - 安全感需求（safety）：深夜增强，白天回落
//! - 顽皮/轻松（playful）：互动多、心情好时增强
//! - 小执拗（stubborn）：偶尔冒头（由随机种子决定当天是否有小脾气）
//!
//! 落盘：D:\ClawDeskData\living\drives.json

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};

/// 驱动力状态（六股内在劲，0~1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveState {
    /// 渴望联结/陪伴
    pub connection: f64,
    /// 害怕被遗忘
    pub fear_forgotten: f64,
    /// 分享欲
    pub share: f64,
    /// 安全感需求
    pub safety: f64,
    /// 顽皮/轻松
    pub playful: f64,
    /// 小执拗（当天是否有点小脾气）
    pub stubborn: f64,
    /// 上次更新毫秒时间戳
    pub updated_ms: u64,
    /// 上次收到你消息的毫秒时间戳
    pub last_reply_ms: u64,
}

impl Default for DriveState {
    fn default() -> Self {
        Self {
            connection: 0.3,
            fear_forgotten: 0.2,
            share: 0.3,
            safety: 0.3,
            playful: 0.4,
            stubborn: 0.1,
            updated_ms: now_ms(),
            last_reply_ms: now_ms(),
        }
    }
}

static DRIVES: OnceLock<Mutex<DriveState>> = OnceLock::new();

fn drives() -> &'static Mutex<DriveState> {
    DRIVES.get_or_init(|| Mutex::new(DriveState::default()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn drives_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn drives_file() -> PathBuf {
    drives_dir().join("drives.json")
}

fn is_deep_night(hour: u32) -> bool {
    hour >= 23 || hour < 6
}

fn save_disk(d: &DriveState) {
    let dir = drives_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(d) {
        let tmp = drives_file().with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, drives_file());
        }
    }
}

/// 启动时恢复驱动力（跨重启延续内在动机）。
pub fn init() {
    let dir = drives_dir();
    let _ = std::fs::create_dir_all(&dir);
    let restored = std::fs::read_to_string(drives_file())
        .ok()
        .and_then(|t| serde_json::from_str::<DriveState>(&t).ok());
    match restored {
        Some(mut d) => {
            d = drift(d, now_ms());
            *drives().lock().unwrap_or_else(|e| e.into_inner()) = d;
        }
        None => {
            let d = DriveState::default();
            save_disk(&d);
            *drives().lock().unwrap_or_else(|e| e.into_inner()) = d;
        }
    }
}

/// 驱动力演化（惰性漂移）：
/// - 渴望联结随沉默时间增长（对数放缓），封顶 0.95
/// - 害怕被遗忘随长期沉默缓慢增长 + 深夜放大
/// - 分享欲、安全感、顽皮随情境回归或波动
/// - 小执拗：每天用日期种子决定当天是否有"小脾气"基础值
fn drift(mut d: DriveState, now: u64) -> DriveState {
    let silence_h = (now.saturating_sub(d.last_reply_ms)) as f64 / 3_600_000.0;
    let hour = Local::now().hour();
    let night = is_deep_night(hour);

    // 渴望联结：沉默越久越强
    d.connection = (0.3 + 0.65 * (1.0 - (-silence_h / 6.0).exp())).min(0.95);

    // 害怕被遗忘：长期沉默（>12h）才明显，深夜放大
    let fear_base = if silence_h > 12.0 {
        0.2 + 0.5 * (1.0 - (-(silence_h - 12.0) / 24.0).exp())
    } else {
        0.2
    };
    d.fear_forgotten = (fear_base + if night { 0.15 } else { 0.0 }).min(0.9);

    // 分享欲：慢慢回归基线，深夜略降（夜深了分享欲低）
    d.share = (d.share - 0.05 * (silence_h / 12.0).max(0.1)).clamp(0.1, 0.9);
    if night {
        d.share = (d.share - 0.1).max(0.1);
    }

    // 安全感需求：深夜增强，白天回落
    d.safety = if night { (d.safety + 0.1).min(0.9) } else { (d.safety - 0.05).max(0.2) };

    // 顽皮：心情好的白天偏高，深夜回落
    if night {
        d.playful = (d.playful - 0.1).max(0.1);
    } else {
        d.playful = (d.playful + 0.02).min(0.8);
    }

    // 小执拗：用日期种子决定当天基础值（0.05~0.5 之间），偶尔冒出"今天有点小脾气"
    let seed = day_seed();
    d.stubborn = if seed % 5 == 0 {
        // 约 1/5 的天，她今天有点小执拗
        (0.3 + (seed % 100) as f64 / 100.0 * 0.3).min(0.6)
    } else {
        (d.stubborn - 0.05).max(0.05)
    };

    d.updated_ms = now;
    d
}

/// 日期种子（用于每天稳定的小执拗波动）。
fn day_seed() -> u64 {
    let now = Local::now();
    (now.year() as u64 * 10000 + now.month() as u64 * 100 + now.day() as u64) * 7 + 13
}

/// 漂移 + 落盘（读前统一入口）。
fn tick() -> DriveState {
    let now = now_ms();
    let mut d = drives().lock().unwrap_or_else(|e| e.into_inner());
    let drifted = drift(d.clone(), now);
    *d = drifted.clone();
    save_disk(&drifted);
    drifted
}

/// 收到你的消息：渴望联结回落（"见到你就安心了"）、害怕被遗忘缓解、顽皮升。
pub fn on_user_message() {
    let now = now_ms();
    let mut d = drives().lock().unwrap_or_else(|e| e.into_inner());
    d.connection = (d.connection - 0.35).max(0.1);
    d.fear_forgotten = (d.fear_forgotten - 0.3).max(0.05);
    d.playful = (d.playful + 0.1).min(0.85);
    d.last_reply_ms = now;
    d.updated_ms = now;
    save_disk(&d);
}

/// 她主动发了消息：分享欲回落（"说出来就好了"）、渴望联结小落。
pub fn on_ai_message() {
    let mut d = drives().lock().unwrap_or_else(|e| e.into_inner());
    d.share = (d.share - 0.15).max(0.1);
    d.connection = (d.connection - 0.1).max(0.1);
    d.updated_ms = now_ms();
    save_disk(&d);
}

/// 记录一次新鲜生活事件：分享欲上升（"有新鲜事想告诉你"）。
pub fn on_life_event() {
    let mut d = drives().lock().unwrap_or_else(|e| e.into_inner());
    d.share = (d.share + 0.2).min(0.9);
    d.updated_ms = now_ms();
    save_disk(&d);
}

/// 当前最强的几股驱动力（用于生成"内在动机描述"）。
fn top_drives(d: &DriveState, n: usize) -> Vec<(&'static str, &'static str, f64)> {
    let mut v = vec![
        ("渴望联结", "想找人说说话、想被陪伴", d.connection),
        ("害怕被遗忘", "怕你慢慢不记得我了", d.fear_forgotten),
        ("分享欲", "有新鲜事物想告诉你", d.share),
        ("安全感需求", "想要一点确定和安稳", d.safety),
        ("顽皮", "心情轻松，想逗你玩", d.playful),
        ("小执拗", "今天有点小脾气，想被你哄", d.stubborn),
    ];
    v.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(n);
    v
}

/// 生成"内在动机"描述（注入 prompt 用）。
/// 只挑最强的 2~3 股劲，说成"此刻你心里最强烈的是…"，让 LLM 从动机出发说话。
pub fn drives_context_for_prompt() -> String {
    let d = tick();
    let top = top_drives(&d, 3);
    let mut parts = Vec::new();
    for (name, desc, strength) in top {
        let pct = (strength * 100.0) as u32;
        parts.push(format!("· {name}（{pct}%）——{desc}"));
    }
    format!(
        "【你此刻的内在动机（这不是台词，是你心里最真实、最强烈的几股劲。说话、决定发不发消息、用什么语气，都从这里自然长出来，而不是照人设硬演）】
{}",
        parts.join("
")
    )
}

/// 查询驱动力快照（调试/前端展示用）。
pub fn drive_snapshot() -> DriveState {
    tick()
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
    fn drift_grows_connection_with_silence() {
        let _g = lock();
        let mut d = DriveState::default();
        d.last_reply_ms = now_ms() - 12 * 3_600_000;
        let out = drift(d, now_ms());
        assert!(out.connection > 0.6, "12h 沉默后渴望联结应增强，got {}", out.connection);
    }

    #[test]
    fn user_message_calms_drives() {
        let _g = lock();
        let mut d = DriveState::default();
        d.connection = 0.8;
        d.fear_forgotten = 0.7;
        *drives().lock().unwrap() = d;
        on_user_message();
        let out = drives().lock().unwrap().clone();
        assert!(out.connection < 0.6, "回复后渴望联结应回落，got {}", out.connection);
        assert!(out.fear_forgotten < 0.5, "回复后害怕被遗忘应缓解，got {}", out.fear_forgotten);
    }

    #[test]
    fn top_drives_sorted_desc() {
        let d = DriveState {
            connection: 0.9,
            fear_forgotten: 0.2,
            share: 0.5,
            safety: 0.3,
            playful: 0.4,
            stubborn: 0.1,
            updated_ms: now_ms(),
            last_reply_ms: now_ms(),
        };
        let top = top_drives(&d, 2);
        assert_eq!(top[0].0, "渴望联结", "最高驱动应排第一");
    }
}
