//! Reflexion 反思记忆层 —— 从失败中学习，避免重复犯错。
//!
//! 核心闭环（借鉴 Reflexion：Actor + Evaluator + Memory）：
//!   1. 执行（Actor）：agent 完成一次对话/任务
//!   2. 评估（Evaluator）：判断是否失败（最终回复含失败信号 / 工具循环报错 /
//!      用户明确否定），失败则抽取「错误 + 教训」
//!   3. 记忆（Memory）：教训落盘 JSON，下次构建 system prompt 时注入最近的教训
//!
//! 与自进化（self_evolve）的区别：
//!   - self_evolve 生成的是「新技能」（怎么做对一件事，正向）
//!   - reflexion 记录的是「教训」（上次怎么栽的，负向避坑）
//!   两者互补：一个是增长，一个是纠偏。
//!
//! 落盘：`<clawdesk_dir>/reflexions.jsonl`（每行一条，追加式，简单可靠）。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// 一条反思教训。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflexion {
    /// 毫秒时间戳
    pub ts_ms: u64,
    /// 触发反思的任务/用户请求（截断到 200 字，避免污染）
    pub task: String,
    /// 出错的具体现象（如「tool_call 参数错误」「超时」「输出格式错误」）
    pub mistake: String,
    /// 提炼出的教训（一句话，下次注入 system prompt 时 AI 依此规避）
    pub lesson: String,
    /// 是否已注入过（注入记一次，用于遗忘/去重参考）
    pub injected: u32,
}

static REFLEXIONS: OnceLock<Mutex<Vec<Reflexion>>> = OnceLock::new();

fn store() -> &'static Mutex<Vec<Reflexion>> {
    REFLEXIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn reflexions_file() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("reflexions.jsonl")
}

/// 启动时恢复教训库。
pub fn init() {
    let path = reflexions_file();
    let mut list = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if let Ok(r) = serde_json::from_str::<Reflexion>(line) {
                list.push(r);
            }
        }
    }
    let mut g = store().lock().unwrap_or_else(|e| e.into_inner());
    *g = list;
    eprintln!("[REFLEXION] 🧠 反思记忆恢复：{} 条", g.len());
}

/// 去重后追加一条教训（同 lesson 不重复记，避免同一错误刷屏）。
pub fn record(task: &str, mistake: &str, lesson: &str) -> bool {
    let lesson = lesson.trim();
    if lesson.is_empty() {
        return false;
    }
    let mut g = store().lock().unwrap_or_else(|e| e.into_inner());
    // 去重：同 lesson 已存在则只刷新时间戳（当它再次发生，说明教训没被记住，保留且置顶）
    if let Some(existing) = g.iter_mut().find(|r| r.lesson == lesson) {
        existing.ts_ms = now_ms();
        return false;
    }
    let rec = Reflexion {
        ts_ms: now_ms(),
        task: clip(task, 200),
        mistake: clip(mistake, 200),
        lesson: lesson.to_string(),
        injected: 0,
    };
    // 落盘（追加一行）：先序列化再入内存，避免 move 后借用
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(reflexions_file())
    {
        if let Ok(json) = serde_json::to_string(&rec) {
            let _ = writeln!(f, "{json}");
        }
    }
    g.push(rec);
    // 只保留最近 50 条，避免无限增长
    if g.len() > 50 {
        g.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
        g.truncate(50);
    }
    eprintln!("[REFLEXION] 📝 记录教训：{}", lesson);
    true
}

/// 注入最近的教训（最多 N 条，按时间倒序，注入计数 +1）。
/// 返回拼好的 prompt 片段；无教训时返回空字符串。
pub fn inject_lessons(max: usize) -> String {
    let mut g = store().lock().unwrap_or_else(|e| e.into_inner());
    if g.is_empty() {
        return String::new();
    }
    g.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
    let mut out = String::from("\n\n## 历史反思教训（Reflexion 记忆，请规避同类错误）\n");
    let mut shown = 0;
    for r in g.iter().take(max) {
        out.push_str(&format!("- 教训：{}（曾因：{}）\n", r.lesson, r.mistake));
        shown += 1;
    }
    for r in g.iter_mut().take(max) {
        r.injected += 1;
    }
    if shown == 0 {
        return String::new();
    }
    out
}

/// 截断字符串到 max 字符（按 char 边界，避免截断多字节 UTF-8）。
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// 评估一段最终回复是否为「失败」，并抽取教训。
///
/// 规则（保守：只在明确失败信号下记录，避免误伤正常回答）：
/// - 回复以「抱歉/无法/不能/失败/出错/超时」开头
/// - 回复命中「未配置 / 没有权限 / 无法完成 / 工具调用失败」
/// - 短回复且带「我错了/搞错了/重试」等认错词
/// 返回 Option<(mistake, lesson)>；非失败返回 None。
pub fn evaluate_failure(final_text: &str, user_request: &str) -> Option<(String, String)> {
    let t = final_text.trim();
    if t.is_empty() {
        return None;
    }
    let short = t.len() <= 80;
    let head = t.chars().take(30).collect::<String>();

    let lead_fail = [
        "抱歉", "很抱歉", "无法", "不能", "失败", "出错", "错误", "超时", "我无法",
    ]
    .iter()
    .any(|k| head.contains(k));

    let body_fail = [
        "未配置",
        "没有权限",
        "权限不足",
        "无法完成",
        "工具调用失败",
        "调用失败",
        "执行失败",
        "不支持的模型",
        "请求失败",
        "网络错误",
    ]
    .iter()
    .any(|k| t.contains(k));

    let admit = short
        && ["我错了", "搞错了", "弄错了", "理解错了", "重试", "再试一次"]
            .iter()
            .any(|k| t.contains(k));

    if !(lead_fail || body_fail || admit) {
        return None;
    }

    // 提炼 mistake + lesson（无需再调 LLM 生成，规则抽取足够轻量）
    let mistake = if body_fail {
        // 找出命中的第一个失败关键词作为现象
        let hit = [
            "未配置",
            "没有权限",
            "无法完成",
            "工具调用失败",
            "不支持的模型",
            "请求失败",
            "网络错误",
        ]
        .iter()
        .find(|k| t.contains(**k))
        .copied()
        .unwrap_or("执行失败");
        format!("回复中出现失败信号：{hit}")
    } else if admit {
        "依赖模型自认出错".to_string()
    } else {
        "回复以失败/抱歉开头".to_string()
    };

    let lesson = format!("任务「{}」上次因「{}」失败，请换一种方式重试或先确认前置条件（API Key / 路径 / 权限）再执行。", clip(user_request, 60), mistake);

    Some((mistake, lesson))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_failure_by_lead_word() {
        let r = evaluate_failure("抱歉，我无法完成这个任务。", "帮我做某事");
        assert!(r.is_some());
        let (m, l) = r.unwrap();
        assert!(m.contains("失败"), "mistake should mention failure: {m}");
        assert!(l.contains("帮我做某事"), "lesson should contain task: {l}");
    }

    #[test]
    fn detect_failure_by_body_keyword() {
        let r = evaluate_failure("我已经尝试了，但工具调用失败，请重试。", "查询XXX");
        assert!(r.is_some());
        assert!(r.unwrap().0.contains("工具调用失败"));
    }

    #[test]
    fn detect_failure_by_admit() {
        let r = evaluate_failure("搞错了，我重试", "测试");
        assert!(r.is_some());
    }

    #[test]
    fn no_failure_on_normal_reply() {
        let r = evaluate_failure("你的文件已经创建好了，在 /tmp/test.txt。", "创建文件");
        assert!(r.is_none());
    }

    #[test]
    fn no_failure_on_empty() {
        assert!(evaluate_failure("", "test").is_none());
    }

    #[test]
    fn record_and_inject_roundtrip() {
        // 清空全局 store（测试隔离）
        {
            let mut g = store().lock().unwrap();
            g.clear();
        }
        // 记录一条教训
        let ok = record("测试任务", "工具调用失败", "永远不要用空参数调用 tool_wrong");
        assert!(ok, "should record new lesson");
        // 注入应返回非空
        let injected = inject_lessons(3);
        assert!(!injected.is_empty(), "should inject lessons: {injected}");
        assert!(injected.contains("tool_wrong"), "lesson should appear: {injected}");
        // 清理
        let _ = std::fs::remove_file(reflexions_file());
    }
}
