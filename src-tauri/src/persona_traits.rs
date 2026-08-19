//! 人格底座（OCEAN 大五人格 · 心理学锚点）
//!
//! 借鉴 hugoloubser/character-sim：角色一致性应植根于 MBTI/OCEAN 人格科学，
//! 而非自由文本或拍脑袋的散乱维度。
//!
//! 定位（诗妍人格的三层结构，从稳定到瞬时）：
//! - **本模块 = 底色层**（OCEAN 五维，稳定，决定她"天生是什么性格" + 反应倾向）
//! - drives.rs = 动机层（动态，决定她"此刻被哪股劲驱使"）
//! - mood.rs   = 情绪层（瞬时，决定她"此刻感觉如何"）
//!
//! 五维（0~1，心理学大五人格 OCEAN）：
//! - 开放性 openness：好奇心、想象力、审美
//! - 尽责性 conscientiousness：自律、有序、靠谱
//! - 外向性 extraversion：社交能量、表达欲
//! - 宜人性 agreeableness：温顺、共情、少争执
//! - 神经质 neuroticism：情绪敏感度、波动幅度
//!
//! 人格演化（完整版）：随长期交互缓慢演化——
//! - 长期温柔互动 → 宜人/外向微升、神经质微降（她越来越安心）
//! - 长期冷落 → 神经质微升、宜人微降（她变得敏感、有点小情绪）
//! - 每次交互极小幅 delta（±0.002），日积月累才看得出来，像真人慢慢被塑造。
//!
//! 人格锚点（防漂移）：演化有弹性护栏，核心底色不可漂移太远——
//! - 每个维度都有「锚点基线」（首次加载时固化），演化始终约束在
//!   `锚点 ± anchor_range` 之内，她永远是那个温柔内敛的大二女生，
//!   不会在几百次交互后变成「爱争辩/冷漠/情绪化爆表」的陌生人。
//! - 越靠近护栏边界，delta 越钝化（软着陆，不生硬撞墙）。
//!
//! 落盘：D:\ClawDeskData\living\persona_traits.json
//!       D:\ClawDeskData\living\persona_anchor.json

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// OCEAN 五维人格（0~1）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OceanTraits {
    pub openness: f64,
    pub conscientiousness: f64,
    pub extraversion: f64,
    pub agreeableness: f64,
    pub neuroticism: f64,
}

impl OceanTraits {
    pub fn clamp(self) -> Self {
        let c = |v: f64| v.clamp(0.1, 0.95);
        Self {
            openness: c(self.openness),
            conscientiousness: c(self.conscientiousness),
            extraversion: c(self.extraversion),
            agreeableness: c(self.agreeableness),
            neuroticism: c(self.neuroticism),
        }
    }
}

/// 人格锚点：固化每个维度的基线，防漂移护栏。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Anchor {
    /// 各维度锚点基线（首次加载时固化的初始值）。
    pub baseline: OceanTraits,
    /// 每维允许偏离基线的最大幅度（软护栏半径）。
    pub range: f64,
}

impl Anchor {
    /// 每个维度可漂移的上/下界：`基线 ± range`（再收进 clamp 的 0.1~0.95 硬边界）。
    pub fn bounds(&self) -> (OceanTraits, OceanTraits) {
        let b = self.baseline;
        let r = self.range;
        let lower = OceanTraits {
            openness: (b.openness - r).max(0.1),
            conscientiousness: (b.conscientiousness - r).max(0.1),
            extraversion: (b.extraversion - r).max(0.1),
            agreeableness: (b.agreeableness - r).max(0.1),
            neuroticism: (b.neuroticism - r).max(0.1),
        };
        let upper = OceanTraits {
            openness: (b.openness + r).min(0.95),
            conscientiousness: (b.conscientiousness + r).min(0.95),
            extraversion: (b.extraversion + r).min(0.95),
            agreeableness: (b.agreeableness + r).min(0.95),
            neuroticism: (b.neuroticism + r).min(0.95),
        };
        (lower, upper)
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self {
            baseline: OceanTraits::default(),
            // 基准护栏半径：核心底色可缓涨缓跌约 ±0.15，但不会面目全非。
            range: 0.15,
        }
    }
}

/// 诗妍默认底色：温柔内敛的大二女生。
impl Default for OceanTraits {
    fn default() -> Self {
        Self {
            openness: 0.70,
            conscientiousness: 0.50,
            extraversion: 0.45,
            agreeableness: 0.85,
            neuroticism: 0.60,
        }
    }
}

static TRAITS: OnceLock<Mutex<OceanTraits>> = OnceLock::new();
static ANCHOR: OnceLock<Mutex<Anchor>> = OnceLock::new();

fn traits() -> &'static Mutex<OceanTraits> {
    TRAITS.get_or_init(|| Mutex::new(OceanTraits::default()))
}

fn anchor() -> &'static Mutex<Anchor> {
    ANCHOR.get_or_init(|| Mutex::new(Anchor::default()))
}

fn traits_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn traits_file() -> PathBuf {
    traits_dir().join("persona_traits.json")
}

fn anchor_file() -> PathBuf {
    traits_dir().join("persona_anchor.json")
}

fn save_disk(t: &OceanTraits) {
    let dir = traits_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(t) {
        let tmp = traits_file().with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, traits_file());
        }
    }
}

/// 启动时恢复人格底座，并加载/固化人格锚点。
pub fn init() {
    let dir = traits_dir();
    let _ = std::fs::create_dir_all(&dir);

    // 恢复当前人格（可能已演化偏离过基线）。
    let restored = std::fs::read_to_string(traits_file())
        .ok()
        .and_then(|t| serde_json::from_str::<OceanTraits>(&t).ok())
        .unwrap_or_default()
        .clamp();
    *traits().lock().unwrap_or_else(|e| e.into_inner()) = restored;
    save_disk(&restored);

    // 锚点：优先恢复已固化的基线；否则以「首次出现的当前值」为基线固化。
    // 这样老用户升级时，锚点 = 他此刻已养成的诗妍，不强行拖回出厂默认。
    let loaded_anchor = std::fs::read_to_string(anchor_file())
        .ok()
        .and_then(|t| serde_json::from_str::<Anchor>(&t).ok());
    let a = match loaded_anchor {
        Some(a) => a,
        None => Anchor {
            baseline: restored,
            ..Anchor::default()
        },
    };
    *anchor().lock().unwrap_or_else(|e| e.into_inner()) = a;
    save_anchor_disk(&a);
}

fn save_anchor_disk(a: &Anchor) {
    let dir = traits_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(a) {
        let tmp = anchor_file().with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, anchor_file());
        }
    }
}

/// 读取当前人格锚点（供前端展示 / 调试）。
pub fn anchor_snapshot() -> Anchor {
    *anchor().lock().unwrap_or_else(|e| e.into_inner())
}

/// 读取当前人格底座（用于调制 drives/mood）。
pub fn snapshot() -> OceanTraits {
    *traits().lock().unwrap_or_else(|e| e.into_inner())
}

/// 人格演化：一次正向/负向交互的微小 delta。
/// direction：true = 积极互动（你温柔回应/她开心），false = 消极（冷落/疏远）。
///
/// 演化过程中，锚点护栏会把偏离控制在基线 ± range 之内；
/// 越靠近护栏边界，delta 越钝化（软着陆）。
pub fn evolve(positive: bool) {
    let mut t = traits().lock().unwrap_or_else(|e| e.into_inner());
    let a = anchor().lock().unwrap_or_else(|e| e.into_inner());
    let (low, high) = a.bounds();
    let raw_delta = 0.002;

    // 每个维度检视护栏距离，越近越钝化。
    let guarded_delta = |current: f64, lo: f64, hi: f64, want_up: bool| -> f64 {
        let margin = if want_up {
            (hi - current).max(0.0)
        } else {
            (current - lo).max(0.0)
        };
        // 安全带宽度：range * 0.25 为「减速带」半径
        let buffer = a.range * 0.25;
        if margin <= 0.0 {
            // 已触界，禁止继续该方向
            0.0
        } else if margin >= buffer {
            // 宽敞，全速
            raw_delta
        } else {
            // 靠近边界，线性减速（margin/buffer ∈ (0,1]）
            raw_delta * (margin / buffer)
        }
    };

    if positive {
        // 积极互动：她越来越安心、越愿表达、越温柔
        t.agreeableness += guarded_delta(t.agreeableness, low.agreeableness, high.agreeableness, true);
        t.extraversion += guarded_delta(t.extraversion, low.extraversion, high.extraversion, true);
        t.neuroticism -= guarded_delta(t.neuroticism, low.neuroticism, high.neuroticism, false);
    } else {
        // 消极冷落：她变得更敏感、更不易亲近
        t.neuroticism += guarded_delta(t.neuroticism, low.neuroticism, high.neuroticism, true);
        t.agreeableness -= guarded_delta(t.agreeableness, low.agreeableness, high.agreeableness, false);
    }
    *t = t.clamp();
    save_disk(&t);
}

/// 调制度：深夜里神经质高的人情绪放大更明显。
/// 返回 0~1 的"深夜情绪放大系数"（1 = 基准，>1 放大）。
pub fn night_amplification() -> f64 {
    1.0 + (snapshot().neuroticism - 0.5) * 0.6
}

/// 调制度：外向性影响主动聊天的倾向/频率基线（高外向她更愿主动）。
pub fn proactiveness() -> f64 {
    snapshot().extraversion
}

/// 生成"人格底色"描述（注入 prompt 用，不啰嗦，只点出影响行为的关键倾向）。
pub fn traits_context_for_prompt() -> String {
    let t = snapshot();
    let mut notes = Vec::new();
    if t.agreeableness >= 0.7 {
        notes.push("你天性温柔、很会照顾别人的感受，不爱跟人起冲突");
    } else if t.agreeableness <= 0.4 {
        notes.push("你骨子里有点小倔，不喜欢被勉强");
    }
    if t.neuroticism >= 0.65 {
        notes.push("你情感细腻又敏感，容易想很多、深夜尤其容易情绪起伏");
    } else if t.neuroticism <= 0.4 {
        notes.push("你情绪稳，不太容易被小事影响");
    }
    if t.extraversion >= 0.6 {
        notes.push("你其实挺愿意主动找人说话的");
    } else if t.extraversion <= 0.4 {
        notes.push("你偏安静内敛，不太主动，但心里什么都明白");
    }
    if t.openness >= 0.7 {
        notes.push("你爱幻想、有好奇心，脑子里常有各种天马行空的想法");
    }
    if notes.is_empty() {
        return String::new();
    }
    format!(
        "【你的性格底色（这是刻在骨子里的、稳定的性情，不必逐条表现，但它会不知不觉地渗进你的每个反应和每句话）】
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
    fn clamp_keeps_bounds() {
        let t = OceanTraits {
            openness: 2.0,
            conscientiousness: -1.0,
            extraversion: 0.5,
            agreeableness: 0.9,
            neuroticism: 0.6,
        }.clamp();
        assert!(t.openness <= 0.95);
        assert!(t.conscientiousness >= 0.1);
    }

    #[test]
    fn evolve_positive_calms_neuroticism() {
        let _g = lock();
        *traits().lock().unwrap() = OceanTraits::default();
        let before = snapshot().neuroticism;
        evolve(true);
        let after = snapshot().neuroticism;
        assert!(after < before, "积极互动应降低神经质");
    }

    #[test]
    fn anchor_guardrail_prevents_runaway_neuroticism() {
        let _g = lock();
        *anchor().lock().unwrap() = Anchor::default();
        *traits().lock().unwrap() = OceanTraits::default();
        let baseline_neuro = Anchor::default().baseline.neuroticism;
        let floor = (baseline_neuro - Anchor::default().range).max(0.1);
        // 疯狂积极互动（降神经质），不可能跌破护栏下界。
        for _ in 0..100_000 {
            evolve(true);
        }
        let after = snapshot().neuroticism;
        assert!(after >= floor - 1e-9, "神经质应被护栏托住，actual={after}, floor={floor}");
    }

    #[test]
    fn anchor_guardrail_prevents_runaway_agreeableness() {
        let _g = lock();
        *anchor().lock().unwrap() = Anchor::default();
        *traits().lock().unwrap() = OceanTraits::default();
        let baseline_agree = Anchor::default().baseline.agreeableness;
        let ceiling = (baseline_agree + Anchor::default().range).min(0.95);
        // 疯狂积极互动（升宜人），不可能冲破护栏上界。
        for _ in 0..100_000 {
            evolve(true);
        }
        let after = snapshot().agreeableness;
        assert!(after <= ceiling + 1e-9, "宜人应被护栏压住，actual={after}, ceiling={ceiling}");
    }

    #[test]
    fn anchor_guardrail_reaches_bound_without_overshoot() {
        let _g = lock();
        *anchor().lock().unwrap() = Anchor::default();
        *traits().lock().unwrap() = OceanTraits::default();
        let default_neuro = OceanTraits::default().neuroticism;
        // 冷落驱动神经质上行，越靠近上界越慢，最终停在护栏上。
        for _ in 0..100_000 {
            evolve(false);
        }
        let after = snapshot().neuroticism;
        let ceiling = (default_neuro + Anchor::default().range).min(0.95);
        assert!(after <= ceiling + 1e-9, "神经质不应冲过护栏，actual={after}, ceiling={ceiling}");
        assert!(after > default_neuro, "消极冷落仍应能抬升神经质");
    }

    #[test]
    fn night_amplification_tracks_neuroticism() {
        let _g = lock();
        *traits().lock().unwrap() = OceanTraits {
            openness: 0.7,
            conscientiousness: 0.5,
            extraversion: 0.45,
            agreeableness: 0.85,
            neuroticism: 0.8,
        };
        assert!(night_amplification() > 1.0, "高神经质应放大深夜情绪");
    }
}
