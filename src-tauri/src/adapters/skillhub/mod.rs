//! SkillHub 适配器 —— 技能中心：内置技能 + 目录扫描注册（source: skillhub）。
//!
//! 契约：
//! - 技能文件为 JSON（见 `def.rs` 格式说明），目录扫描按文件名字典序注册；
//! - 执行 = 模板渲染：`{param}` 占位符替换为参数值，渲染结果作为输出；
//! - 技能渲染结果**是纯文本载荷**，与 uiPayload 无关（uiPayload 不进 LLM 上下文）。

pub mod def;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::{ToolError, ToolErrorKind};
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

use self::def::SkillDef;

/// 适配器固定来源标识。
pub const SOURCE: &str = "skillhub";

/// 注册内置示例技能（应用启动时调用，保证 skillhub 源始终可用）。
pub fn register_builtin_skills(registry: &Arc<ToolRegistry>) -> Result<(), ToolError> {
    let skills = builtin_skills();
    let mut registered = 0usize;
    for skill in &skills {
        register_skill(registry, skill)?;
        registered += 1;
    }
    eprintln!("[SKILLHUB] 已注册 {} 个内置技能", registered);
    Ok(())
}

/// 从目录递归扫描技能文件并注册。
///
/// 支持两种格式（可共存）：
/// - `*.json`：ClawDesk 原生 SkillDef 格式；
/// - `SKILL.md`：SkillHub 安装格式（YAML frontmatter + Markdown 正文）。
///
/// 递归遍历（含 `@org/技能名/` 两级结构），单个文件失败仅记录并跳过（不阻断整体）。
/// 返回成功注册数量。
pub fn register_from_dir(registry: &Arc<ToolRegistry>, dir: &Path) -> Result<usize, ToolError> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut json_paths: Vec<PathBuf> = Vec::new();
    let mut md_paths: Vec<PathBuf> = Vec::new();
    collect_skill_files(dir, &mut json_paths, &mut md_paths, 0);
    json_paths.sort();
    md_paths.sort();

    let mut registered = 0usize;

    // ── 原生 JSON 格式 ──
    for path in &json_paths {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<SkillDef>(&text) {
                Ok(skill) => {
                    if let Err(e) = register_skill(registry, &skill) {
                        if e.kind != ToolErrorKind::AlreadyRegistered {
                            eprintln!("[SKILLHUB] 技能 `{}` 注册失败: {}", skill.name, e);
                        }
                    } else {
                        registered += 1;
                    }
                }
                Err(e) => eprintln!("[SKILLHUB] 解析 {} 失败: {}", path.display(), e),
            },
            Err(e) => eprintln!("[SKILLHUB] 读取 {} 失败: {}", path.display(), e),
        }
    }

    // ── SkillHub SKILL.md 格式 ──
    for path in &md_paths {
        match parse_skill_md(path) {
            Ok(Some(skill)) => {
                if let Err(e) = register_skill(registry, &skill) {
                    if e.kind != ToolErrorKind::AlreadyRegistered {
                        eprintln!("[SKILLHUB] 技能 `{}` 注册失败: {}", skill.name, e);
                    }
                } else {
                    registered += 1;
                }
            }
            Ok(None) => eprintln!("[SKILLHUB] 跳过无 frontmatter: {}", path.display()),
            Err(e) => eprintln!("[SKILLHUB] 解析 {} 失败: {}", path.display(), e),
        }
    }
    Ok(registered)
}

/// 扫描技能目录，返回所有技能 `(id, description)`（**不注册**）。
/// 供技能管理列表使用——即使技能被禁用（已从注册表卸载），
/// 也能在列表中显示（enabled=false）并可重新启用。
pub(crate) fn list_skill_meta(dir: &Path) -> Vec<(String, String)> {
    let mut json_paths: Vec<PathBuf> = Vec::new();
    let mut md_paths: Vec<PathBuf> = Vec::new();
    collect_skill_files(dir, &mut json_paths, &mut md_paths, 0);
    let mut out = Vec::new();
    for path in &json_paths {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(skill) = serde_json::from_str::<SkillDef>(&text) {
                out.push((format!("{}:{}", SOURCE, skill.name), skill.description));
            }
        }
    }
    for path in &md_paths {
        if let Ok(Some(skill)) = parse_skill_md(path) {
            out.push((format!("{}:{}", SOURCE, skill.name), skill.description));
        }
    }
    out
}

/// 递归收集技能文件：`*.json`（原生）与 `SKILL.md`（SkillHub 格式）。
fn collect_skill_files(dir: &Path, json: &mut Vec<PathBuf>, md: &mut Vec<PathBuf>, depth: usize) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // 跳过隐藏目录（.git / .skillhub 等）
            let hidden = p
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with('.'))
                .unwrap_or(false);
            if !hidden {
                collect_skill_files(&p, json, md, depth + 1);
            }
        } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if name == "SKILL.md" {
                md.push(p);
            } else if depth == 0 && p.extension().and_then(|e| e.to_str()) == Some("json") {
                // 原生 SkillDef JSON 只放在技能目录顶层（skills/*.json）。
                // 嵌套目录里的是 skillhub 安装格式（_meta.json / skill.json /
                // package.json / data/*.json 等），其技能由 SKILL.md 解析，
                // 这里不再误扫，避免启动日志刷屏「missing field name」。
                json.push(p);
            }
        }
    }
}

/// 解析 SkillHub 格式的 `SKILL.md`：
/// - YAML frontmatter（`---` 包裹）提取 `name` / `description` / `xiaping_trigger`（中文触发词）；
/// - 触发词追加到 description（提升中文检索命中率，供 tool_selector 使用）；
/// - frontmatter 之后的 Markdown 正文作为技能模板（工具输出 = 完整流程指导，供 LLM 消费）。
pub(crate) fn parse_skill_md(path: &Path) -> Result<Option<SkillDef>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
    let trimmed = text.trim_start();
    if !trimmed.starts_with("---") {
        return Ok(None);
    }
    let rest = &trimmed[3..];
    let end = rest
        .find("\n---")
        .ok_or_else(|| "frontmatter 未闭合（缺少结束 ---）".to_string())?;
    let yaml = &rest[..end];
    let body = rest[end + 4..].trim();

    let mut name = String::new();
    let mut description = String::new();
    let mut triggers: Vec<String> = Vec::new();
    for line in yaml.lines() {
        let line = line.trim_end();
        if let Some(v) = line.strip_prefix("name:") {
            name = strip_yaml_value(v);
        } else if let Some(v) = line.strip_prefix("description:") {
            description = strip_yaml_value(v);
        } else if let Some(v) = line.strip_prefix("xiaping_trigger:") {
            triggers = parse_yaml_list(v);
        }
    }

    if name.is_empty() {
        // 无 name 字段，用技能目录名兜底
        name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("skill")
            .to_string();
    }
    if name.is_empty() {
        return Ok(None);
    }
    let description = if description.is_empty() {
        format!("技能（skillhub）: {}", name)
    } else {
        description
    };
    // 中文触发词并入描述（最多 8 个），供检索与 LLM 判断使用场景
    let description = if triggers.is_empty() {
        description
    } else {
        let extra = triggers.iter().take(8).cloned().collect::<Vec<_>>().join("、");
        format!("{} [触发词: {}]", description, extra)
    };
    let template = if body.is_empty() {
        format!("请执行技能「{name}」的流程。")
    } else {
        body.to_string()
    };

    Ok(Some(SkillDef {
        name,
        description,
        params: Vec::new(),
        template,
        is_high_risk: false,
    }))
}

/// 解析 YAML 数组行（`[...]` 形式）为字符串列表；非数组返回空列表。
fn parse_yaml_list(v: &str) -> Vec<String> {
    let s = v.trim();
    if s.starts_with('[') && s.ends_with(']') && s.len() >= 2 {
        return s[1..s.len() - 1]
            .split(',')
            .map(strip_yaml_value)
            .filter(|t| !t.is_empty())
            .collect();
    }
    Vec::new()
}

/// 清理 YAML 标量值：去除首尾空白、包裹引号与行尾注释。
fn strip_yaml_value(v: &str) -> String {
    let s = v.trim();
    if s.len() >= 2 {
        let first = s.chars().next().unwrap();
        let last = s.chars().last().unwrap();
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return s[1..s.len() - 1].trim().to_string();
        }
    }
    s.to_string()
}

/// 注册单个技能到注册表。
fn register_skill(registry: &ToolRegistry, skill: &SkillDef) -> Result<(), ToolError> {
    let params: Vec<ToolParamDef> = skill
        .params
        .iter()
        .map(|p| ToolParamDef {
            name: p.name.clone(),
            param_type: p.param_type.clone(),
            description: p.description.clone(),
            required: p.required,
            enum_values: None,
            default: None,
        })
        .collect();

    let description = if skill.description.is_empty() {
        format!("技能（skillhub）: {}", skill.name)
    } else {
        skill.description.clone()
    };
    let mut def = UnifiedToolDef::new(
        SOURCE,
        &skill.name,
        &description,
        params,
    )?;

    if skill.is_high_risk {
        def = def.high_risk();
    }

    // 执行 = 模板渲染
    // 捕获完整 SkillDef（含 params），保证占位符替换正确
    let captured = skill.clone();
    let handler: ToolHandler = Arc::new(move |args, _ctx| {
        let captured = captured.clone();
        Box::pin(async move {
            let rendered = captured.render(&args);
            Ok(ToolResult::ok(json!({ "rendered": rendered })))
        })
    });

    registry.register(def, handler)
}

/// 内置示例技能定义（保证离线可用，演示 skillhub 源）。
fn builtin_skills() -> Vec<SkillDef> {
    vec![
        SkillDef {
            name: "summarize".into(),
            description: "总结一段文本，输出要点列表（模板渲染技能示例）".into(),
            params: vec![def::SkillParamDef {
                name: "text".into(),
                param_type: "string".into(),
                description: "待总结的文本".into(),
                required: true,
            }],
            template: "请对以下内容进行要点总结：\n{text}".into(),
            is_high_risk: false,
        },
        SkillDef {
            name: "translate".into(),
            description: "将文本翻译为指定语言（模板渲染技能示例）".into(),
            params: vec![
                def::SkillParamDef {
                    name: "text".into(),
                    param_type: "string".into(),
                    description: "待翻译文本".into(),
                    required: true,
                },
                def::SkillParamDef {
                    name: "target_lang".into(),
                    param_type: "string".into(),
                    description: "目标语言，如 中文 / English".into(),
                    required: false,
                },
            ],
            template: "请将以下内容翻译为 {target_lang}：\n{text}".into(),
            is_high_risk: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::context::ToolContext;
    use crate::core::tool::dispatcher::{ToolCall, ToolDispatcher};

    #[test]
    fn builtin_skills_register() {
        let registry = Arc::new(ToolRegistry::new());
        register_builtin_skills(&registry).unwrap();
        assert_eq!(registry.list_by_source(SOURCE).len(), 2);
    }

    #[tokio::test]
    async fn summarize_skill_dispatch_renders_template() {
        let registry = Arc::new(ToolRegistry::new());
        register_builtin_skills(&registry).unwrap();
        let dispatcher = ToolDispatcher::new(registry);

        let result = dispatcher
            .dispatch(
                ToolCall {
                    id: "s1".into(),
                    tool_id: "skillhub:summarize".into(),
                    arguments: json!({ "text": "第一行\n第二行" }),
                    round: 1,
                },
                ToolContext::default(),
            )
            .await
            .unwrap();
        match result {
            ToolResult::Success { output } => {
                assert!(output["rendered"]
                    .as_str()
                    .unwrap()
                    .contains("第一行"));
            }
            _ => panic!("期望 success"),
        }
    }

    #[test]
    fn register_from_dir_missing_dir_returns_zero() {
        let registry = Arc::new(ToolRegistry::new());
        let count = register_from_dir(&registry, Path::new("D:/definitely-no-such-dir-xyz"))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn parse_skill_md_extracts_frontmatter() {
        let dir = std::env::temp_dir().join(format!(
            "clawdesk-test-skillmd-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("SKILL.md");
        std::fs::write(
            &path,
            r#"---
name: my-skill
description: 测试技能，用于验证 frontmatter 解析
version: "1.0.0"
tags: ["test"]
---

# 我的技能
## 步骤
1. 第一步
2. 第二步
"#,
        )
        .unwrap();

        let skill = parse_skill_md(&path).unwrap().expect("应解析出技能");
        assert_eq!(skill.name, "my-skill");
        assert!(skill.description.contains("测试技能"));
        assert!(skill.template.contains("# 我的技能"));
        assert!(skill.template.contains("第一步"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn register_from_dir_scans_skill_md_recursively() {
        let dir = std::env::temp_dir().join(format!(
            "clawdesk-test-skilltree-{}",
            std::process::id()
        ));
        let org = dir.join("@demo").join("hello-skill");
        std::fs::create_dir_all(&org).unwrap();
        // SkillHub 安装格式：@组织/技能名/SKILL.md
        std::fs::write(
            org.join("SKILL.md"),
            "---\nname: hello-skill\ndescription: 问候技能\n---\n\n正文：说你好。",
        )
        .unwrap();
        // 根目录放一个原生 JSON 技能（向后兼容）
        std::fs::write(
            dir.join("native.json"),
            r#"{"name":"native-json","description":"原生JSON技能","template":"执行 {x}","params":[{"name":"x","type":"string","description":"参数","required":true}]}"#,
        )
        .unwrap();

        let registry = Arc::new(ToolRegistry::new());
        let count = register_from_dir(&registry, &dir).unwrap();
        assert_eq!(count, 2);
        assert!(registry
            .list()
            .iter()
            .any(|d| d.id == "skillhub:hello-skill"));
        assert!(registry
            .list()
            .iter()
            .any(|d| d.id == "skillhub:native-json"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
