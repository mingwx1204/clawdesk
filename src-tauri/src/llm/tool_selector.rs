//! 工具按需加载（检索式工具选择）—— 方案 1。
//!
//! 背景：SkillHub 一次性注册 87+ 技能，若全部序列化进 LLM 上下文，
//! token 开销巨大且模型选择精度下降。
//!
//! 策略：
//! - **固定保留**：builtin / mcp / 其他非 skillhub 源（核心能力，数量少）；
//! - **按需检索**：skillhub 技能按「用户消息 × 技能索引」n-gram 命中数打分，
//!   每轮只暴露 top-N（默认 10），零依赖、纯 CPU、毫秒级。

use crate::core::tool::def::UnifiedToolDef;
use std::collections::HashSet;

/// 默认每轮暴露的 skillhub 技能数量（不含内置工具）。
/// ★ 恢复智能调度：0 = 彻底禁用；N>0 = 每轮按「用户消息 × 技能索引」相关度打分，
///   只暴露最相关的 top-N 技能（检索式工具选择）。
///   之前设为 0 是因为 DeepSeek v4-flash 对 87+ 技能 schema 困惑（乱调工具）。
///   现已修复 SseStream 丢字 / ToolCall index / 历史污染等根因，恢复按需调度。
///   若模型仍乱调，可调回 0 或降到 3。
pub const DEFAULT_TOP_N: usize = 5;

/// 核心内置工具白名单：32 个工具（22 内置 + 10 技能）对 DeepSeek v4-flash 负担过大，
/// 模型会困惑导致工具调用输出坏 JSON / 复述 system prompt。只保留高频核心工具。
const CORE_BUILTIN: &[&str] = &[
    "terminal",
    "file_read",
    "file_write",
    "list_dir",
    "echo",
    "get_time",
    "analyze_image",
    "ocr",
    "search_text",
    "window_minimize",
    "window_maximize",
    "window_get_state",
    "window_screenshot",
    "generate_image",
    "git_status",
    "calculate",
];

/// 检索式工具选择：核心内置工具（白名单）全保留，skillhub 技能按相关度取 top_n。
///
/// 返回顺序：先固定工具（builtin/mcp/...），再命中的技能（按分数降序）。
pub fn select_tools(defs: &[UnifiedToolDef], prompt: &str, top_n: usize) -> Vec<UnifiedToolDef> {
    let mut fixed: Vec<UnifiedToolDef> = defs
        .iter()
        .filter(|d| d.source != "skillhub")
        // ★ 内置工具白名单：只对 builtin 源应用核心工具过滤（避免 30+ 工具让模型困惑）；
        //   用户配置的 MCP / 其他外部工具始终保留（显式接入，不应被静默丢弃）
        .filter(|d| d.source != "builtin" || CORE_BUILTIN.contains(&d.name.as_str()))
        .cloned()
        .collect();

    // skillhub 技能按相关度打分排序
    let prompt_grams: HashSet<String> = ngrams(prompt).into_iter().collect();
    let mut skills: Vec<(usize, &UnifiedToolDef)> = defs
        .iter()
        .filter(|d| d.source == "skillhub")
        .map(|d| (score(d, &prompt_grams), d))
        .collect();
    skills.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.name.cmp(&b.1.name))
    });

    for (_, d) in skills.into_iter().take(top_n) {
        fixed.push(d.clone());
    }
    fixed
}

/// 打分：技能索引（name + description）命中 prompt n-gram 的数量。
fn score(def: &UnifiedToolDef, prompt_grams: &HashSet<String>) -> usize {
    let index_text = format!("{} {}", def.name, def.description).to_lowercase();
    prompt_grams
        .iter()
        .filter(|g| index_text.contains(g.as_str()))
        .count()
}

/// 提取 n-gram：英文/数字单词（≥2 字符）+ 中文连续段 2~4 字。
fn ngrams(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();

    // 英文 / 数字单词
    for w in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        if w.len() >= 2 {
            out.push(w.to_string());
        }
    }

    // 中文连续段 2~4-gram
    let chars: Vec<char> = lower.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].is_ascii() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && !chars[j].is_ascii() {
            j += 1;
        }
        let seg: Vec<char> = chars[i..j].to_vec();
        if seg.len() >= 2 {
            for n in 2..=4usize {
                if seg.len() >= n {
                    for k in 0..=(seg.len() - n) {
                        out.push(seg[k..k + n].iter().collect());
                    }
                }
            }
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::def::ToolParamDef;

    fn skill(source: &str, name: &str, desc: &str) -> UnifiedToolDef {
        UnifiedToolDef::new(source, name, desc, Vec::<ToolParamDef>::new()).unwrap()
    }

    #[test]
    fn keeps_fixed_and_limits_skills() {
        let defs = vec![
            skill("builtin", "file_read", "读取本地文件"),
            skill("mcp", "fs", "文件系统"),
            skill("skillhub", "excel-auto-zh", "Excel/WPS 表格自动化处理工具"),
            skill("skillhub", "weather", "查询天气，无需 API Key"),
            skill("skillhub", "cnfinancialscraper", "中国金融机构数据爬取工具"),
            skill("skillhub", "github", "GitHub CLI 交互"),
            skill("skillhub", "ppt-generator", "生成 PPT 演示文稿"),
            skill("skillhub", "baidu-search", "百度搜索网页"),
        ];
        let out = select_tools(&defs, "帮我做一个 Excel 表格，统计销售数据", 3);
        // 固定工具全保留
        assert!(out.iter().any(|d| d.id == "builtin:file_read"));
        assert!(out.iter().any(|d| d.id == "mcp:fs"));
        // 最多 3 个技能
        let skills: Vec<_> = out.iter().filter(|d| d.source == "skillhub").collect();
        assert!(skills.len() <= 3);
        // Excel 技能应命中（与 prompt 相关）
        assert!(skills.iter().any(|d| d.id == "skillhub:excel-auto-zh"));
    }

    #[test]
    fn ngrams_chinese_and_english() {
        let gs = ngrams("做 Excel 表格");
        assert!(gs.iter().any(|g| g == "excel"));
        assert!(gs.iter().any(|g| g == "表格"));
        // 中文连续段 2~4-gram："表格" 的 2-gram 与 3-gram 均存在
        assert!(gs.iter().any(|g| g == "表格"));
        // 单字"做"（1 字符）不产出 gram
        assert!(!gs.iter().any(|g| g == "做"));
    }

    #[test]
    fn zero_prompt_still_returns_skills() {
        let defs = vec![
            skill("skillhub", "a", "aaa"),
            skill("skillhub", "b", "bbb"),
        ];
        let out = select_tools(&defs, "", 1);
        assert_eq!(out.len(), 1); // 保底取 1 个
    }
}
