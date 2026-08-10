//! SkillHub 技能定义结构 —— 从 JSON 文件解析技能。
//!
//! 技能文件格式（示例）：
//! ```json
//! {
//!   "name": "summarize",
//!   "description": "总结一段文本的核心要点",
//!   "params": [
//!     { "name": "text", "type": "string", "description": "待总结文本", "required": true }
//!   ],
//!   "template": "请对以下内容进行总结，输出要点列表：\n{text}"
//! }
//! ```
//!
//! `template` 中的 `{param}` 占位符在执行时被参数值替换，
//! 渲染结果作为工具输出返回（供前端展示 / 后续 LLM 消费）。

use serde::Deserialize;

/// 技能参数定义（与 UnifiedToolDef 的 ToolParamDef 对齐的子集）。
#[derive(Debug, Clone, Deserialize)]
pub struct SkillParamDef {
    pub name: String,
    #[serde(rename = "type", default = "default_type")]
    pub param_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

fn default_type() -> String {
    "string".into()
}

/// 技能定义（source: skillhub）。
#[derive(Debug, Clone, Deserialize)]
pub struct SkillDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub params: Vec<SkillParamDef>,
    /// 模板文本，`{param}` 占位符在执行时替换。
    pub template: String,
    /// 是否高危（默认 false，可被安全中间件消费）。
    #[serde(default)]
    pub is_high_risk: bool,
}

impl SkillDef {
    /// 渲染模板：将 `{param}` 占位符替换为参数值。
    ///
    /// 未提供的参数占位符保留原文（便于识别遗漏）；`{...}` 之外的内容原样保留。
    pub fn render(&self, args: &serde_json::Value) -> String {
        let mut out = self.template.clone();
        for param in &self.params {
            let placeholder = format!("{{{}}}", param.name);
            let value = args
                .get(&param.name)
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| format!("<{}(未提供)>", param.name));
            out = out.replace(&placeholder, &value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_from_json() {
        let json = r#"{
            "name": "summarize",
            "description": "总结文本",
            "params": [
                { "name": "text", "type": "string", "description": "内容", "required": true }
            ],
            "template": "总结：{text}"
        }"#;
        let skill: SkillDef = serde_json::from_str(json).unwrap();
        assert_eq!(skill.name, "summarize");
        assert_eq!(skill.params.len(), 1);
        assert!(skill.params[0].required);
    }

    #[test]
    fn render_replaces_placeholders() {
        let skill = SkillDef {
            name: "t".into(),
            description: "d".into(),
            params: vec![
                SkillParamDef {
                    name: "a".into(),
                    param_type: "string".into(),
                    description: String::new(),
                    required: true,
                },
                SkillParamDef {
                    name: "b".into(),
                    param_type: "number".into(),
                    description: String::new(),
                    required: false,
                },
            ],
            template: "a={a} b={b} 固定文本".to_string(),
            is_high_risk: false,
        };
        let out = skill.render(&serde_json::json!({ "a": "hello", "b": 42 }));
        assert_eq!(out, "a=hello b=42 固定文本");
    }

    #[test]
    fn render_missing_param_keeps_marker() {
        let skill = SkillDef {
            name: "t".into(),
            description: "d".into(),
            params: vec![SkillParamDef {
                name: "x".into(),
                param_type: "string".into(),
                description: String::new(),
                required: true,
            }],
            template: "x={x}".to_string(),
            is_high_risk: false,
        };
        let out = skill.render(&serde_json::json!({}));
        assert!(out.contains("未提供"));
    }
}
