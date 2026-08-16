//! 技能生成器 —— 用 LLM 自动创建 SKILL.md，注册进工具库。
//!
//! 流程：
//!   1. 收集需要进化的任务（从 tracker 拿到低成功率/高频工具）
//!   2. 构造 LLM prompt（描述当前工具、失败模式、改进方向）
//!   3. LLM 生成 SKILL.md 格式的技能定义
//!   4. 保存到技能库目录 + 自动注册

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::tool::error::ToolError;
use crate::core::tool::registry::ToolRegistry;

use super::tracker::ToolStats;

/// 一轮进化的报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolveReport {
    pub candidates_count: usize,
    pub generated_count: usize,
    pub registered_count: usize,
    pub skills: Vec<String>,
    pub generated_skills: Vec<GeneratedSkill>, // 完整技能定义（供注册）
    pub summary: String,
}

impl EvolveReport {
    pub fn empty() -> Self {
        Self {
            candidates_count: 0,
            generated_count: 0,
            registered_count: 0,
            skills: Vec::new(),
            generated_skills: Vec::new(),
            summary: "本轮无需进化".into(),
        }
    }
}

/// AI 生成的技能定义（兼容 SkillDef，可直接注册）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedSkill {
    pub name: String,
    pub description: String,
    pub template: String,
    #[serde(default)]
    pub since: String,
}

/// 技能生成器。
pub struct SkillGenerator {
    api_key: Arc<String>,
    base_url: String,
    model: String,
}

impl SkillGenerator {
    pub fn new(api_key: String, base_url: String, model: String) -> Self {
        Self {
            api_key: Arc::new(api_key),
            base_url,
            model,
        }
    }

    /// 调用 LLM 为候选任务生成改进技能。
    pub async fn generate_skills(&self, candidates: &[(String, ToolStats)]) -> Result<EvolveReport, String> {
        if candidates.is_empty() {
            return Ok(EvolveReport::empty());
        }

        // 构造 prompt
        let mut task_desc = String::new();
        for (id, stats) in candidates {
            task_desc.push_str(&format!(
                "- 工具 `{}`：执行 {} 次，成功率 {:.0}%，平均耗时 {}ms\n",
                id,
                stats.total,
                stats.success_rate() * 100.0,
                stats.avg_elapsed_ms as u64,
            ));
        }

        let prompt = format!(
            "你是一个 AI 工具优化专家。以下是当前系统中需要改进的工具列表及其执行统计：\n\
             \n\
             {}\n\
             \n\
             请分析这些工具的失败原因，并为每个工具生成一个改进版的 SKILL 定义。\n\
             每个技能定义必须包含：\n\
             1. 技能名称（纯小写英文，下划线分隔，如 improve_file_search）\n\
             2. 功能描述（中文，说明改进点和适用场景）\n\
             3. 执行模板（用 {{param}} 占位符，AI 执行时会替换参数）\n\
             \n\
             输出格式（严格 JSON 数组，不含 Markdown 代码块标记）：\n\
             [{{\"name\":\"技能名\",\"description\":\"描述\",\"template\":\"模板\"}}]\n\
             \n\
             每个技能只生成一个 JSON 对象。模板要实用、可执行、不超过 500 字。",
            task_desc
        );

        // 调用 LLM
        let result = self.call_llm(&prompt).await?;

        // 解析生成的技能
        let skills: Vec<GeneratedSkill> = if let Ok(arr) = serde_json::from_str::<Vec<GeneratedSkill>>(&result) {
            arr
        } else {
            // 尝试去掉可能的 Markdown 代码块
            let cleaned = result
                .trim()
                .trim_start_matches("```json")
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim();
            serde_json::from_str(cleaned).map_err(|e| format!("解析 LLM 输出失败: {}。原始输出: {}", e, cleaned.chars().take(300).collect::<String>()))?
        };

        let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        let skills_clone = skills.clone();
        Ok(EvolveReport {
            candidates_count: candidates.len(),
            generated_count: skills.len(),
            registered_count: 0,
            skills: names,
            generated_skills: skills_clone,
            summary: format!("已为 {} 个待改进工具生成 {} 个技能", candidates.len(), skills.len()),
        })
    }

    /// 把生成的技能保存到技能库目录并注册到 registry。
    pub fn save_and_register(
        &self,
        registry: &ToolRegistry,
        skill: &GeneratedSkill,
    ) -> Result<(), ToolError> {
        use crate::core::tool::def::UnifiedToolDef;
        use crate::core::tool::registry::ToolHandler;
        use crate::core::tool::result::ToolResult;
        let name = skill.name.clone();
        let template = skill.template.clone();
        let desc = skill.description.clone();

        // 构造 UnifiedToolDef
        let def = UnifiedToolDef::new(
            "skillhub", // 走 skillhub 源，自动加载
            &format!("skillhub:{}", name),
            &format!("[自进化] {}", desc),
            vec![],
        )?;

        let tmpl = template.clone();
        let handler: ToolHandler = Arc::new(move |args, _ctx| {
            let args = args.clone();
            let result = tmpl.clone();
            // 简单的 {param} 替换
            let rendered = if let serde_json::Value::Object(map) = &args {
                let mut r = result;
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        r = r.replace(&format!("{{{}}}", k), s);
                    } else {
                        r = r.replace(&format!("{{{}}}", k), &v.to_string());
                    }
                }
                r
            } else {
                result
            };
            Box::pin(async move {
                Ok(ToolResult::ok(serde_json::json!({ "result": rendered })))
            })
        });

        registry.register(def, handler)?;

        // 保存 SKILL.md 到技能目录
        let dir = self.skills_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}.md", name));
        let content = format!(
            "---\nname: {}\ndescription: {}\nsince: {}\n---\n\n{}",
            name, desc, skill.since, template
        );
        let _ = std::fs::write(&path, content);

        eprintln!("[SELF_EVOLVE] ✅ 技能已生成: {} ({})", name, path.display());
        Ok(())
    }

    /// 技能库目录（`<数据目录>/self_evolve_skills/`）。
    fn skills_dir(&self) -> std::path::PathBuf {
        crate::llm::settings::clawdesk_dir().join("self_evolve_skills")
    }

    /// 调用 LLM 生成技能。
    async fn call_llm(&self, prompt: &str) -> Result<String, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            // ★ IPv6 黑洞修复（2026-08-10）
            .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
            .build()
            .map_err(|e| format!("构建请求客户端失败: {e}"))?;

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": "你是 AI 工具优化专家。只输出 JSON 数组，不含 Markdown 标记。" },
                { "role": "user", "content": prompt }
            ],
            "max_tokens": 2048,
            "temperature": 0.3,
            "stream": false
        });

        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key.as_str()))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("LLM 请求失败: {e}"))?;

        let status = resp.status();
        let text = resp.text().await.map_err(|e| format!("读取响应失败: {e}"))?;

        if !status.is_success() {
            return Err(format!("LLM 返回 HTTP {}: {}", status.as_u16(), text.chars().take(300).collect::<String>()));
        }

        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("解析 LLM 响应失败: {}. 原始: {}", e, text.chars().take(200).collect::<String>()))?;

        let content = parsed["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        if content.is_empty() {
            return Err("LLM 返回了空内容".into());
        }

        Ok(content)
    }
}
