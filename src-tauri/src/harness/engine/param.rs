//! LLM 请求参数 —— 从 CodeWhale tui/src/client/chat.rs 剥离重构。
//! 内置 thinking_effort / temperature / max_tokens 的 OpenAI 兼容构造。

use serde::{Deserialize, Serialize};

/// 推理努力程度（DeepSeek thinking 模式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Off,
    Low,
    Medium,
    High,
    Max,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" => Self::Off,
            "low" | "minimal" => Self::Low,
            "high" => Self::High,
            "max" | "xhigh" => Self::Max,
            _ => Self::Medium,
        }
    }
}

impl Default for ReasoningEffort {
    fn default() -> Self {
        Self::Medium
    }
}

/// 模型请求参数（OpenAI /chat/completions 兼容子集）。
#[derive(Debug, Clone)]
pub struct ModelParams {
    pub model: String,
    pub reasoning_effort: ReasoningEffort,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
    pub presence_penalty: Option<f64>,
    pub frequency_penalty: Option<f64>,
    /// 是否流式（引擎默认 true）。
    pub stream: bool,
}

impl Default for ModelParams {
    fn default() -> Self {
        Self {
            model: "deepseek-chat".to_string(),
            reasoning_effort: ReasoningEffort::Medium,
            temperature: None,
            top_p: None,
            max_tokens: Some(8192),
            presence_penalty: None,
            frequency_penalty: None,
            stream: true,
        }
    }
}

impl ModelParams {
    /// 构造 API 请求 body 中的模型参数对象（不含 messages）。
    pub fn to_body_params(&self) -> serde_json::Value {
        let mut p = serde_json::json!({
            "model": self.model,
            "stream": self.stream,
        });
        if let Some(t) = self.temperature {
            p["temperature"] = serde_json::json!(t);
        }
        if let Some(v) = self.top_p {
            p["top_p"] = serde_json::json!(v);
        }
        if let Some(mt) = self.max_tokens {
            p["max_tokens"] = serde_json::json!(mt);
        }
        if let Some(v) = self.presence_penalty {
            p["presence_penalty"] = serde_json::json!(v);
        }
        if let Some(v) = self.frequency_penalty {
            p["frequency_penalty"] = serde_json::json!(v);
        }
        match self.reasoning_effort {
            ReasoningEffort::Off => {
                // 关闭思考：DeepSeek 接受的对象格式（布尔 thinking:false 不被支持，会忽略）
                p["thinking"] = serde_json::json!({"type": "disabled"});
            }
            effort => {
                p["reasoning_effort"] = serde_json::json!(effort.as_str());
            }
        }
        p
    }
}
