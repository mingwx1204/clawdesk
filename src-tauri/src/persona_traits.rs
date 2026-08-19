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
//! 落盘：D:\ClawDeskData\living\persona_traits.json

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

fn traits() -> &'static Mutex<OceanTraits> {
    TRAITS.get_or_init(|| Mutex::new(OceanTraits::default()))
}

fn traits_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

fn traits_file() -> PathBuf {
    traits_dir().join("persona_traits.json")
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

/// 启动时恢复人格底座。
pub fn init() {
    let dir = traits_dir();
    let _ = std::fs::create_dir_all(&dir);
    let restored = std::fs::read_to_string(traits_file())
        .ok()
        .and_then(|t| serde_json::from_str::<OceanTraits>(&t).ok())
        .unwrap_or_default()
        .clamp();
    *traits().lock().unwrap_or_else(|e| e.into_inner()) = restored;
    save_disk(&restored);
}

/// 读取当前人格底座（用于调制 drives/mood）。
pub fn snapshot() -> OceanTraits {
    *traits().lock().unwrap_or_else(|e| e.into_inner())
}

/// 人格演化：一次正向/负向交互的微小 delta。
/// direction：true = 积极互动（你温柔回应/她开心），false = 消极（冷落/疏远）。
pub fn evolve(positive: bool) {
    let mut t = traits().lock().unwrap_or_else(|e| e.into_inner());
    let delta = 0.002;
    if positive {
        // 积极互动：她越来越安心、越愿表达、越温柔
        t.agreeableness += delta;
        t.extraversion += delta * 0.5;
        t.neuroticism -= delta;
    } else {
        // 消极冷落：她变得更敏感、更不易亲近
        t.neuroticism += delta;
        t.agreeableness -= delta;
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
