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
///
/// 语义分层（借鉴 eros-engine 的 Profile vs Relationship）：
/// - `layer = "profile"`：跨会话稳定事实（如"你对花生过敏"）——任何情境都该知道，永久保留
/// - `layer = "relationship"`：会话时刻/回调（如"昨晚聊到凌晨"）——与具体关系绑定
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
    /// 语义层："profile"（稳定事实）或 "relationship"（会话时刻）。
    /// 旧数据无此字段 → serde 用 default 补 "relationship"（向前兼容）。
    #[serde(default = "default_layer")]
    pub layer: String,
    /// 情绪效价（-1~1）：-1=负面记忆，0=中性，1=正面记忆。旧数据 default 0.0。
    #[serde(default)]
    pub valence: f64,
    /// 记忆可信度 / 精度（0~1）：情绪检索加权用。旧数据 default 0.5。
    #[serde(default = "default_precision")]
    pub precision: f64,
}

fn default_precision() -> f64 { 0.5 }

fn default_layer() -> String {
    "relationship".to_string()
}

/// 抽取规则：主人消息里出现这些关键词 → 值得记住的细节
/// （每个条目 = 一组关键词 + 标签；命中任一关键词即抽取该句）
const RULES: &[(&[&str], &str, f64)] = &[
    (&["我不吃", "我不爱", "讨厌吃", "不喜欢吃"], "食物,忌口", -0.4),
    (&["我最喜欢", "我爱吃", "超喜欢吃", "最爱吃"], "食物,偏好", 0.5),
    (&["我生日", "生日是", "过生日"], "日期,生日", 0.6),
    (&["我养了", "我家猫", "我家狗", "我有一只"], "宠物,生活", 0.6),
    (&["我住", "住在", "搬家", "新家"], "居住,生活", 0.3),
    (&["我上班", "我公司", "我同事"], "工作", 0.1),
    (&["我对象", "我女朋友", "我男朋友", "我老婆", "我老公", "我媳妇", "我丈夫", "我妻子"], "关系,亲密", 0.7),
    (&["我爸妈", "我父母", "我妈", "我爸", "我家人"], "关系,家人", 0.5),
    (&["我最近在", "我这几天", "我打算", "我想学", "我准备"], "计划,生活", 0.4),
    (&["我睡不着", "失眠", "又熬夜", "睡不着觉"], "状态,睡眠", -0.3),
    (&["我难受", "我不舒服", "生病", "感冒", "发烧"], "状态,健康", -0.5),
    (&["我考试", "我面试", "我答辩"], "事件,压力", -0.3),
    (&["我减肥", "我在健身", "跑步", "锻炼"], "习惯,健康", 0.4),
    (&["我最怕", "我怕"], "心理,恐惧", -0.6),
    (&["我小时候", "我童年"], "回忆,童年", 0.3),
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
/// `layer`：「profile」（稳定事实，永久保留）或「relationship」（会话时刻）。
pub fn remember_with_layer(text: &str, source: &str, tags: &str, layer: &str) -> bool {
    remember_with_valence(text, source, tags, layer, 0.0, 0.5)
}

/// 记录一条带情绪效价的细节（layer + valence + precision），真正的落盘/去重入口。
fn remember_with_valence(
    text: &str,
    source: &str,
    tags: &str,
    layer: &str,
    valence: f64,
    precision: f64,
) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let layer = if layer == "profile" { "profile".to_string() } else { "relationship".to_string() };
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
        layer,
        valence: valence.clamp(-1.0, 1.0),
        precision: precision.clamp(0.0, 1.0),
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
    for (keywords, tags, valence) in RULES {
        for kw in *keywords {
            if let Some(pos) = msg.find(kw) {
                // 提取该关键词所在的一句（前后各截 ~40 字）
                let start = pos.saturating_sub(30);
                let end = (pos + kw.len() + 50).min(msg.len());
                let sentence = msg[start..end].trim().to_string();
                if !sentence.is_empty() && remember_with_valence(&sentence, source, tags, "relationship", *valence, 0.5) {
                    added += 1;
                }
                break; // 该组命中一次即可
            }
        }
    }
    added
}

/// 便捷包装：默认记入 relationship 层（会话时刻），保持旧 API 签名兼容。
pub fn remember(text: &str, source: &str, tags: &str) -> bool {
    remember_with_layer(text, source, tags, "relationship")
}

/// 手动添加细节（前端/AI 调用）。默认 relationship 层。
pub fn add_detail(text: &str, tags: &str) -> Result<usize, String> {
    Ok(if remember(text, "manual", tags) { 1 } else { 0 })
}

/// 记录一条「稳定事实」（profile 层）：跨会话持久、永久保留、任何情境都该知道。
/// 用途：你对花生过敏、你养了猫、你生日、你住在哪儿…这类身份级事实。
pub fn add_profile_fact(text: &str, tags: &str) -> Result<usize, String> {
    Ok(if remember_with_layer(text, "manual", tags, "profile") { 1 } else { 0 })
}

/// 只取 profile 层的稳定事实（按 used 降序 → 时间倒序）。
pub fn profile_facts() -> Vec<DetailMemory> {
    let g = details().lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<DetailMemory> = g
        .iter()
        .filter(|d| d.layer == "profile")
        .cloned()
        .collect();
    v.sort_by(|a, b| b.used.cmp(&a.used).then(b.ts_ms.cmp(&a.ts_ms)));
    v
}

/// 只取 relationship 层的会话时刻（按 used 降序 → 时间倒序）。
pub fn relationship_facts() -> Vec<DetailMemory> {
    let g = details().lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<DetailMemory> = g
        .iter()
        .filter(|d| d.layer == "relationship")
        .cloned()
        .collect();
    v.sort_by(|a, b| b.used.cmp(&a.used).then(b.ts_ms.cmp(&a.ts_ms)));
    v
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
    let removed = {
        let mut g = details().lock().unwrap_or_else(|e| e.into_inner());
        let before = g.len();
        g.retain(|d| d.text != text);
        before - g.len()
    };
    if removed > 0 {
        // 复用 flush_disk 全量重写（删除场景低频，可接受）
        flush_disk();
    }
    removed > 0
}

/// 标记一条细节被想起过（AI 注入 prompt 时调用，影响遗忘权重）。
/// 只改内存，批量落盘由调用方随后调用「flush_disk」统一处理（避免高频重写文件）。
pub fn mark_used(text: &str) {
    let mut g = details().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = g.iter_mut().find(|d| d.text == text) {
        d.used += 1;
    }
}

/// 全量重写落盘（内存里的 used 累加 → 写回 details.jsonl）。
/// 由 details_context_for_prompt 在注入后调用一次；删除场景也复用（低频可接受）。
fn flush_disk() {
    let g = details().lock().unwrap_or_else(|e| e.into_inner());
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

/// 情绪键控记忆检索：读取当前 mood，按「情绪一致性 × 精度 × 遗忘度」重排
/// 所有记忆（profile 稳定事实有少量优先）。返回空表示还没有值得说的细节。
pub fn details_context_for_prompt_mood(recent_max: usize, max_chars: usize) -> String {
    let mood = crate::mood::mood_snapshot();
    // 当下心情效价：joy 高偏正面；loneliness/longing 高时，更想去碰那些软的记忆。
    let mood_valence = (mood.joy - mood.loneliness * 0.6 - mood.longing * 0.4).clamp(-1.0, 1.0);

    let all = all_details();
    if all.is_empty() {
        return String::new();
    }

    let now = now_ms();
    let mut scored: Vec<(f64, &DetailMemory)> = all
        .iter()
        .map(|d| {
            let age_days = (now.saturating_sub(d.ts_ms)) as f64 / 86_400_000.0;
            let recency = 1.0 / (age_days + 1.0);
            let used = (d.used as f64 + 1.0).ln_1p();
            // 情绪一致性：0 = 完全不一致，1 = 与当下心情同频
            let congruence = 1.0 - (d.valence - mood_valence).abs().clamp(0.0, 2.0) / 2.0;
            let precision = d.precision.clamp(0.1, 1.0);
            // profile 稳定事实少量优先，但仍受情绪一致性调制
            let layer_bonus = if d.layer == "profile" { 0.3 } else { 0.0 };
            let score =
                used * 0.25 + recency * 0.35 + congruence * 0.50 + precision * 0.20 + layer_bonus;
            (score, d)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // ★ 沉默权（借鉴 mnemo）：低于阈值 0.35 的记忆不塞入上下文，
    //   避免弱匹配污染 prompt——真人不会每句话都翻遍所有记忆。
    const SCORE_THRESHOLD: f64 = 0.35;

    let mut parts: Vec<String> = Vec::new();
    let mut used_bumped: Vec<String> = Vec::new();
    let mut used_chars = 0usize;
    for (score, d) in scored.iter().take(recent_max) {
        if *score < SCORE_THRESHOLD {
            continue; // 弱匹配，沉默（mnemo 风格）
        }
        let line = fmt_line(d);
        if used_chars + line.chars().count() > max_chars {
            break;
        }
        used_chars += line.chars().count();
        parts.push(line);
        used_bumped.push(d.text.clone());
    }
    if parts.is_empty() {
        return String::new();
    }
    for t in used_bumped {
        mark_used(&t);
    }
    flush_disk();
    format!("【你记得的关于主人的事（被看见：这些是他随口提过、你放在心上的。自然聊天时可以提起，但不要一口气全说出来，像真人一样在合适的时机提到）】
{}", parts.join("
"))
}

/// 注入 prompt 的"你记得主人的事"：按时间倒序取最近 N 条（去重、限长）。
/// 返回空表示还没有值得说的细节。
/// 保留旧入口（无情绪排序的确定性检索）供测试与无 mood 依赖的调用方。
#[allow(dead_code)]
pub fn details_context_for_prompt(recent_max: usize, max_chars: usize) -> String {
    // ══ 语义分层检索 ══
    // 1. profile 层（稳定事实）优先——「任何情境都该知道」，永远先注入；
    // 2. relationship 层（会话时刻）按遗忘曲线补足——used 降序 + 时间倒序。
    let facts = profile_facts();
    let moments = relationship_facts();
    if facts.is_empty() && moments.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut used_bumped: Vec<String> = Vec::new();
    let mut used_chars = 0usize;
    // 先注入 profile 稳定事实
    for d in facts.iter().take(recent_max) {
        let line = fmt_line(d);
        if used_chars + line.chars().count() > max_chars {
            break;
        }
        used_chars += line.chars().count();
        parts.push(line);
        used_bumped.push(d.text.clone());
    }
    // 再补 relationship 会话时刻
    for d in moments.iter().take(recent_max) {
        let line = fmt_line(d);
        if used_chars + line.chars().count() > max_chars {
            break;
        }
        used_chars += line.chars().count();
        parts.push(line);
        used_bumped.push(d.text.clone());
    }
    if parts.is_empty() {
        return String::new();
    }
    // 被选中注入的细节「又被想起来一次」→ used+1
    for t in used_bumped {
        mark_used(&t);
    }
    flush_disk();
    format!("【你记得的关于主人的事（被看见：这些是他随口提过、你放在心上的。自然聊天时可以提起，但不要一口气全说出来，像真人一样在合适的时机提到）】
{}", parts.join("
"))
}

/// 把一条细节格式化为一行（带相对时间 + 标签）。
fn fmt_line(d: &DetailMemory) -> String {
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
    format!("【{}】{}{}", when, d.text, if d.tags.is_empty() { String::new() } else { format!("（{}）", d.tags) })
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

    #[test]
    fn forgetting_curve_prefers_used_details() {
        let _g = lock();
        details().lock().unwrap().clear();
        // 旧的但反复想起过的细节（used 高）
        remember("主人不吃香菜", "test", "食物,忌口");
        remember("主人喜欢喝美式", "test", "饮品");
        // 手动标记前一调为被想起过 3 次
        for _ in 0..3 {
            mark_used("主人不吃香菜");
        }
        // 只取 1 条：应选中 used 更高的「香菜」，而非更新的「美式」
        let ctx = details_context_for_prompt(1, 400);
        assert!(ctx.contains("香菜"), "遗忘曲线应优先想起 used 高的细节，got: {ctx}");
        assert!(!ctx.contains("美式"), "used 低的新细节应沉底，got: {ctx}");
    }

    #[test]
    fn context_injection_bumps_used_and_persists() {
        let _g = lock();
        details().lock().unwrap().clear();
        remember("主人养了一只猫", "test", "宠物");
        let before = all_details()[0].used;
        // 注入一次 prompt → used +1 并落盘
        let _ = details_context_for_prompt(5, 400);
        let after = all_details()[0].used;
        assert_eq!(after, before + 1, "注入 prompt 应把 used +1");
    }

    #[test]
    fn profile_facts_are_separated_from_relationship() {
        let _g = lock();
        details().lock().unwrap().clear();
        remember_with_layer("主人对花生过敏", "test", "食物,忌口", "profile");
        remember("昨晚聊到凌晨两点", "test", "聊天");
        let pf = profile_facts();
        let rf = relationship_facts();
        assert_eq!(pf.len(), 1, "profile 应只有 1 条");
        assert_eq!(rf.len(), 1, "relationship 应只有 1 条");
        assert!(pf[0].text.contains("花生"));
        assert!(rf[0].text.contains("凌晨"));
        // 分层检索：profile 稳定事实排在前面（优先占字符预算）
        let ctx = details_context_for_prompt(1, 400);
        assert!(ctx.contains("花生"), "profile 稳定事实应注入");
        assert!(ctx.contains("凌晨"), "relationship 时刻也应注入（两层各取 1 条）");
        // profile 应排在前面
        let p = ctx.find("花生").unwrap();
        let r2 = ctx.find("凌晨").unwrap();
        assert!(p < r2, "profile 稳定事实应排在 relationship 之前");
    }
}

