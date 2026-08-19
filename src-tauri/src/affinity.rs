//! 六维好感度（Affinity）模型 · 关系从「叙事」升级为「可计算的数值维度」
//!
//! 借鉴 eros-engine 的 affinity-model：关系不应只是几段叙事，还要有可量化的
//! 亲密度向量，随时间演化——信任/亲密是「深维度」（一旦建立就牢固，不衰减），
//! 好奇/张力会随时间自然消退（像真人：久了不联系，好奇心淡了，那股较劲也散了）。
//!
//! 六维（0~1，每维都有 1~3 档标签）：
//! - 温暖 warmth：语气、称呼的亲近程度
//! - 信任 trust⭐深：愿意聊多深、袒露多少自己
//! - 好奇 intrigue：追问、主动找话题的欲望
//! - 亲密 intimacy⭐深：内部梗、昵称、呼应旧细节
//! - 耐心 patience：对短消息/敷衍的容忍度
//! - 张力 tension：推拉、玩闹中的较劲、小别扭
//!
//! 写入是「分级」而非精确数值：交互事件报一个粗粒度档位（如 +0.15 的「升温」），
//! 引擎把档位转成数值，先阻尼（乘衰减系数）再门控（限幅），最后 1:1 应用到维度。
//! 这样关系演化是「有方向的、可解释的」，不会因单次事件剧烈跳动。
//!
//! 衰减：intrigue 每天 -0.01，tension 每天 -0.005（无活动时惰性衰减）；
//! trust / intimacy 不衰减（深维度）；warmth / patience 派生自其他维度。
//!
//! 落盘：D:\ClawDeskData\living\affinity.json

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// 好感度六维状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Affinity {
    /// 温暖（派生：亲近措辞程度）
    pub warmth: f64,
    /// 信任（深维度，不衰减）
    pub trust: f64,
    /// 好奇（衰减：每天 -0.01）
    pub intrigue: f64,
    /// 亲密（深维度，不衰减）
    pub intimacy: f64,
    /// 耐心（派生）
    pub patience: f64,
    /// 张力（衰减：每天 -0.005）
    pub tension: f64,
    /// 上次更新毫秒时间戳（衰减计算锚点）
    pub updated_ms: u64,
}

impl Default for Affinity {
    fn default() -> Self {
        Self {
            warmth: 0.3,
            trust: 0.3,
            intrigue: 0.4,
            intimacy: 0.2,
            patience: 0.5,
            tension: 0.1,
            updated_ms: now_ms(),
        }
    }
}

static AFFINITY: OnceLock<Mutex<Affinity>> = OnceLock::new();

fn affinity() -> &'static Mutex<Affinity> {
    AFFINITY.get_or_init(|| Mutex::new(Affinity::default()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn aff_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn aff_file() -> PathBuf {
    aff_dir().join("affinity.json")
}

fn save_disk(a: &Affinity) {
    let dir = aff_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(a) {
        let tmp = aff_file().with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, aff_file());
        }
    }
}

/// 启动时恢复好感度（跨重启延续关系）。
pub fn init() {
    let dir = aff_dir();
    let _ = std::fs::create_dir_all(&dir);
    let restored = std::fs::read_to_string(aff_file())
        .ok()
        .and_then(|t| serde_json::from_str::<Affinity>(&t).ok());
    match restored {
        Some(a) => {
            let a = apply_decay(a);
            *affinity().lock().unwrap_or_else(|e| e.into_inner()) = a;
        }
        None => {
            let a = Affinity::default();
            save_disk(&a);
            *affinity().lock().unwrap_or_else(|e| e.into_inner()) = a;
        }
    }
}

/// 惰性时间衰减：intrigue -0.01/天、tension -0.005/天；trust/intimacy 不衰减。
fn apply_decay(mut a: Affinity) -> Affinity {
    let now = now_ms();
    let days = (now.saturating_sub(a.updated_ms)) as f64 / 86_400_000.0;
    if days <= 0.0 {
        return a;
    }
    a.intrigue = (a.intrigue - 0.01 * days).clamp(0.0, 1.0);
    a.tension = (a.tension - 0.005 * days).clamp(0.0, 1.0);
    a.updated_ms = now;
    a
}

/// 读取（含衰减）当前亲和度快照。
fn tick() -> Affinity {
    let mut g = affinity().lock().unwrap_or_else(|e| e.into_inner());
    let drifted = apply_decay(g.clone());
    *g = drifted.clone();
    save_disk(&drifted);
    drifted
}

/// 一次分级写入的主轴（四个「线轴」，两个派生轴自动计算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffinityAxis {
    Trust,
    Intrigue,
    Intimacy,
    Tension,
}

/// 一次写入的「力度档位」（分级，而非精确数值）。
#[derive(Debug, Clone, Copy)]
pub enum Grade {
    /// 轻微：+0.03（阻尼后）
    Small,
    /// 中等：+0.07（阻尼后）
    Medium,
    /// 明显：+0.12（阻尼后）
    Large,
}

impl Grade {
    fn delta(self) -> f64 {
        match self {
            Grade::Small => 0.03,
            Grade::Medium => 0.07,
            Grade::Large => 0.12,
        }
    }
}

/// 应用一次分级写入：先阻尼（0.85 系数），再 clamp 到 [0,1]，最后派生 warmth/patience。
pub fn apply(axis: AffinityAxis, grade: Grade) {
    let mut a = tick();
    let delta = grade.delta() * 0.85; // 阻尼：让演化有惯性，不因单次事件跳变
    match axis {
        AffinityAxis::Trust => a.trust = (a.trust + delta).clamp(0.0, 1.0),
        AffinityAxis::Intrigue => a.intrigue = (a.intrigue + delta).clamp(0.0, 1.0),
        AffinityAxis::Intimacy => a.intimacy = (a.intimacy + delta).clamp(0.0, 1.0),
        AffinityAxis::Tension => a.tension = (a.tension + delta).clamp(0.0, 1.0),
    }
    // 派生：温暖 = f(亲密, 信任)；耐心 = f(信任, 1-张力)
    a.warmth = (0.3 * a.intimacy + 0.4 * a.trust + 0.2).clamp(0.0, 1.0);
    a.patience = (0.5 * a.trust + 0.5 * (1.0 - a.tension)).clamp(0.0, 1.0);
    a.updated_ms = now_ms();
    let mut g = affinity().lock().unwrap_or_else(|e| e.into_inner());
    *g = a.clone();
    save_disk(&a);
}

/// 收到你的消息（自动回复入口）：你主动找她 → 好奇升（想知道你说啥）、信任微升。
pub fn on_user_message() {
    apply(AffinityAxis::Intrigue, Grade::Small);
    apply(AffinityAxis::Trust, Grade::Small);
}

/// 她主动发消息成功：亲密微升（主动表达拉近距离）。
pub fn on_ai_message() {
    apply(AffinityAxis::Intimacy, Grade::Small);
}

/// 一次深聊（长对话/深夜长谈）：信任、亲密明显升。
pub fn on_deep_talk() {
    apply(AffinityAxis::Trust, Grade::Medium);
    apply(AffinityAxis::Intimacy, Grade::Medium);
}

/// 一次很深的倾诉（消息特别长/重大心事）：信任、亲密强烈升（Large 档）。
pub fn on_deep_talk_heavy() {
    apply(AffinityAxis::Trust, Grade::Large);
    apply(AffinityAxis::Intimacy, Grade::Large);
}

/// 一次小别扭（玩笑式推拉）：张力升，耐心微降。
pub fn on_tease() {
    apply(AffinityAxis::Tension, Grade::Small);
}

/// 敏感度查询：单维（调试/前端用）。
pub fn affinity_snapshot() -> Affinity {
    tick()
}

/// 亲和度上下文（注入 prompt）：只说最关键的倾向，不啰嗦。
pub fn affinity_context_for_prompt() -> String {
    let a = tick();
    // 只注出对当轮说话有影响的几维
    let mut notes = Vec::new();
    if a.intimacy >= 0.5 {
        notes.push("你们已经很熟了，说话可以自然亲昵，偶尔用你们之间的默契");
    }
    if a.trust >= 0.5 {
        notes.push("你蛮信任对方的，愿意多说一点心里话");
    }
    if a.intrigue >= 0.55 {
        notes.push("你对对方的生活挺好奇，会想知道TA今天过得怎样");
    }
    if a.tension >= 0.35 {
        notes.push("你们之间有点小较劲/小别扭的劲，可以带一点玩闹式的推拉");
    }
    if notes.is_empty() {
        return String::new();
    }
    format!(
        "【你和对方的关系（这决定你说话的语气亲疏与深浅，但不要每句都点破）】
{}",
        notes.join("；")
    )
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
    fn apply_raises_axis_clamped() {
        let _g = lock();
        *affinity().lock().unwrap() = Affinity::default();
        apply(AffinityAxis::Trust, Grade::Large);
        let a = affinity().lock().unwrap().clone();
        assert!(a.trust > 0.3, "信任应上升，got {}", a.trust);
        assert!(a.trust <= 1.0);
    }

    #[test]
    fn trust_and_intimacy_do_not_decay() {
        let _g = lock();
        let mut a = Affinity::default();
        a.trust = 0.7;
        a.intimacy = 0.6;
        a.updated_ms = now_ms() - 10 * 86_400_000; // 10 天前
        let out = apply_decay(a);
        assert_eq!(out.trust, 0.7, "信任是深维度，不应衰减");
        assert_eq!(out.intimacy, 0.6, "亲密是深维度，不应衰减");
    }

    #[test]
    fn intrigue_decays_with_time() {
        let _g = lock();
        let mut a = Affinity::default();
        a.intrigue = 0.9;
        a.updated_ms = now_ms() - 10 * 86_400_000; // 10 天前
        let out = apply_decay(a);
        assert!(out.intrigue < 0.9, "好奇应随时间衰减，got {}", out.intrigue);
    }

    #[test]
    fn warmth_is_derived() {
        let _g = lock();
        *affinity().lock().unwrap() = Affinity::default();
        apply(AffinityAxis::Intimacy, Grade::Large);
        apply(AffinityAxis::Trust, Grade::Large);
        let a = affinity().lock().unwrap().clone();
        assert!(a.warmth > 0.3, "温暖应随亲密/信任上升，got {}", a.warmth);
    }
}
