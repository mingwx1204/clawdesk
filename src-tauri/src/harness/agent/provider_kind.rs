//! ProviderKind 本地替代 —— 原 codewhale_config::ProviderKind（config crate 禁移）。
//! 仅保留 agent/src/lib.rs 实际使用的 30 个变体，字段语义与 config 一致。

use serde::{Deserialize, Serialize};

/// 模型提供方标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    Anthropic,
    Arcee,
    Atlascloud,
    Deepinfra,
    Deepseek,
    Fireworks,
    Huggingface,
    LongCat,
    Meta,
    Minimax,
    MinimaxAnthropic,
    Moonshot,
    Novita,
    NvidiaNim,
    Ollama,
    Openai,
    OpenaiCodex,
    OpencodeGo,
    Openmodel,
    Openrouter,
    Sakana,
    Sglang,
    Siliconflow,
    Stepfun,
    Together,
    Vllm,
    Volcengine,
    WanjieArk,
    Xai,
    XiaomiMimo,
    Zai,
}

impl ProviderKind {
    /// 稳定的小写标识（与 config crate 的 as_str 语义一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Arcee => "arcee",
            Self::Atlascloud => "atlascloud",
            Self::Deepinfra => "deepinfra",
            Self::Deepseek => "deepseek",
            Self::Fireworks => "fireworks",
            Self::Huggingface => "huggingface",
            Self::LongCat => "longcat",
            Self::Meta => "meta",
            Self::Minimax => "minimax",
            Self::MinimaxAnthropic => "minimax-anthropic",
            Self::Moonshot => "moonshot",
            Self::Novita => "novita",
            Self::NvidiaNim => "nvidia-nim",
            Self::Ollama => "ollama",
            Self::Openai => "openai",
            Self::OpenaiCodex => "openai-codex",
            Self::OpencodeGo => "opencode-go",
            Self::Openmodel => "openmodel",
            Self::Openrouter => "openrouter",
            Self::Sakana => "sakana",
            Self::Sglang => "sglang",
            Self::Siliconflow => "siliconflow",
            Self::Stepfun => "stepfun",
            Self::Together => "together",
            Self::Vllm => "vllm",
            Self::Volcengine => "volcengine",
            Self::WanjieArk => "wanjie-ark",
            Self::Xai => "xai",
            Self::XiaomiMimo => "xiaomi-mimo",
            Self::Zai => "zai",
        }
    }
}

/// opencode-go 聊天模型规范 ID 解析（本地替代 codewhale_config::opencode_go_chat_model_id）。
/// 命中 opencode-go Chat allowlist 的模型名/别名（含 `opencode-go/` 前缀）→ 规范 ID；否则 None。
pub fn opencode_go_chat_model_id(name: &str) -> Option<String> {
    const GO_CHAT_IDS: &[&str] = &[
        "deepseek-v4-pro",
        "grok-4.5",
        "glm-5.2",
        "glm-5.1",
        "kimi-k3",
        "kimi-k2.7-code",
        "kimi-k2.6",
        "deepseek-v4-flash",
        "mimo-v2.5",
        "mimo-v2.5-pro",
    ];
    let normalized = name.trim().to_ascii_lowercase();
    let bare = normalized.strip_prefix("opencode-go/").unwrap_or(&normalized);
    GO_CHAT_IDS
        .iter()
        .find(|id| id.eq_ignore_ascii_case(bare))
        .map(|id| (*id).to_string())
}
