//! DeepSeek（OpenAI 兼容）HTTP 客户端。
//!
//! 安全契约：
//! - API Key 仅存于本结构内存字段，**绝不写入文件 / 日志 / 错误消息**；
//! - 每次调用通过 Authorization Bearer 头传递；
//! - 网络超时默认 60s，响应体截断防异常。

use serde_json::{json, Value};

use super::{build_system_prompt, ChatResponse, LlmMessage, Role};
use crate::llm::runner::ChatProvider;

/// DeepSeek 兼容端点。
const DEFAULT_ENDPOINT: &str = "https://api.deepseek.com/chat/completions";

/// 默认对话模型。
const DEFAULT_MODEL: &str = "deepseek-chat";

/// 单次 LLM 请求配置。
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub api_key: String,
    pub model: String,
    pub endpoint: String,
    /// 请求超时（秒）。
    pub timeout_secs: u64,
    /// 请求失败（HTTP 429 / 5xx / 网络错误）最大重试次数。
    pub max_retries: u32,
    /// 指数退避基础延迟（毫秒）。
    pub retry_base_delay_ms: u64,
}

impl LlmConfig {
    /// 从环境变量构造（不打印 key）。
    /// 说明：供 `#[ignore]` 端到端测试与命令行场景使用，故豁免 dead_code。
    #[allow(dead_code)]
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("DEEPSEEK_API_KEY").ok()?;
        if api_key.trim().is_empty() {
            return None;
        }
        Some(Self {
            api_key,
            model: DEFAULT_MODEL.into(),
            endpoint: DEFAULT_ENDPOINT.into(),
            timeout_secs: 60,
            max_retries: 3,
            retry_base_delay_ms: 600,
        })
    }

    /// 从运行时传入的 key 构造（内存态，不落盘）。
    pub fn with_key(api_key: String) -> Self {
        Self {
            api_key,
            model: DEFAULT_MODEL.into(),
            endpoint: DEFAULT_ENDPOINT.into(),
            timeout_secs: 60,
            max_retries: 3,
            retry_base_delay_ms: 600,
        }
    }
}

/// LLM 客户端（同步调用，由调用方放在合适的执行上下文）。
///
/// 实现 `Clone`：路由层在锁内快速取用配置副本（vision / main 均独立实例）。
#[derive(Clone)]
pub struct LlmClient {
    config: LlmConfig,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        Self { config }
    }

    /// 更新主模型配置（模型负载动态切换：V4-Pro ↔ V4-Flash，文档 §八.3）。
    ///
    /// 仅更新模型标识与端点，API Key 保持不变（Key 不落盘、不打印）。
    pub fn update_model(&mut self, model: &str, endpoint: Option<&str>) {
        if !model.trim().is_empty() {
            self.config.model = model.trim().to_string();
        }
        if let Some(ep) = endpoint {
            if !ep.trim().is_empty() {
                self.config.endpoint = ep.trim().to_string();
            }
        }
    }

    /// 替换 API Key（运行时配置切换，仅内存态）。
    pub fn update_api_key(&mut self, api_key: String) {
        self.config.api_key = api_key;
    }

    /// 当前配置快照（不含 key，供路由状态展示）。
    pub fn config_summary(&self) -> serde_json::Value {
        json!({
            "model": self.config.model,
            "endpoint": self.config.endpoint,
            "timeoutSecs": self.config.timeout_secs,
        })
    }

    /// 发起一次不带工具的对话请求（保留接口，暂未使用）。
    #[allow(dead_code)]
    pub fn chat(&self, messages: &[LlmMessage]) -> Result<ChatResponse, String> {
        let body = json!({
            "model": self.config.model,
            "messages": messages,
            "max_tokens": 2048,
        });
        self.post_json(&body).and_then(|text| {
            serde_json::from_str::<ChatResponse>(&text)
                .map_err(|e| format!("解析模型响应失败: {}", e))
        })
    }

    /// 发起一次带工具的对话请求。
    ///
    /// `tools` 为 OpenAI function calling 格式（由 `serialize_tools` 生成）。
    pub fn chat_with_tools(
        &self,
        messages: &[LlmMessage],
        tools: &Value,
    ) -> Result<ChatResponse, String> {
        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "max_tokens": 2048,
        });

        // system 提示（若消息列表没有 system 则注入）
        if !messages.iter().any(|m| matches!(m.role, Role::System)) {
            let system = LlmMessage {
                role: Role::System,
                content: build_system_prompt(),
                tool_calls: None,
                tool_call_id: None,
            };
            body["messages"]
                .as_array_mut()
                .unwrap()
                .insert(0, serde_json::to_value(&system).unwrap());
        }

        let resp_text = self.post_json(&body)?;
        serde_json::from_str::<ChatResponse>(&resp_text)
            .map_err(|e| format!("解析模型响应失败: {}", e))
    }

    /// 多模态视觉请求：携带 base64 图片（OpenAI vision 兼容格式）。
    ///
    /// 说明：DeepSeek 官方端点不支持视觉时会返回明确错误，由调用方
    /// 降级处理（analyze_image 工具内置降级：仅返回图像数据）。
    /// `mime` 如 `image/jpeg` / `image/png`。
    #[allow(dead_code)] // 预留：视觉模型路由接入后启用
    pub fn chat_vision(
        &self,
        image_b64: &str,
        mime: &str,
        text_prompt: &str,
    ) -> Result<ChatResponse, String> {
        let content = json!([
            { "type": "text", "text": text_prompt },
            {
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, image_b64) }
            }
        ]);
        let body = json!({
            "model": self.config.model,
            "messages": [ { "role": "user", "content": content } ],
            "max_tokens": 1024,
        });
        self.post_json(&body).and_then(|text| {
            serde_json::from_str::<ChatResponse>(&text)
                .map_err(|e| format!("解析模型响应失败: {}", e))
        })
    }

    /// POST JSON 到端点，返回响应文本（按配置指数退避重试）。
    fn post_json(&self, body: &Value) -> Result<String, String> {
        post_json_with_retry(
            &self.config.endpoint,
            &self.config.api_key,
            body,
            self.config.timeout_secs,
            self.config.max_retries,
            self.config.retry_base_delay_ms,
        )
    }
}

/// 通用 OpenAI 兼容 HTTP POST（跨主模型 / 视觉模型 / 绘图 API 复用）。
///
/// 统一协议适配层（文档 §八.4）：抹平不同厂商端点差异，统一走
/// `POST {endpoint}` + `Authorization: Bearer {key}` + JSON body；
/// 所有第三方模型返回统一由调用方封装为固定结构体。
pub fn post_json_to(
    endpoint: &str,
    api_key: &str,
    body: &Value,
    timeout_secs: u64,
) -> Result<String, String> {
    // 默认 3 次重试（绘图 / 视觉等第三方端点同样受益）
    post_json_with_retry(endpoint, api_key, body, timeout_secs, 3, 600)
}

/// 带指数退避重试的 POST。
///
/// 仅对**可重试**错误重试：HTTP 429 / 5xx / 网络层错误；
/// 4xx 其它错误（参数错误等）直接返回，避免无意义重试。
fn post_json_with_retry(
    endpoint: &str,
    api_key: &str,
    body: &Value,
    timeout_secs: u64,
    max_retries: u32,
    base_delay_ms: u64,
) -> Result<String, String> {
    let mut attempt = 0u32;
    loop {
        match post_json_once(endpoint, api_key, body, timeout_secs) {
            Ok(s) => return Ok(s),
            Err((msg, retryable)) => {
                if !retryable || attempt >= max_retries {
                    return Err(msg);
                }
                attempt += 1;
                // 指数退避：base * 2^(attempt-1) + 抖动（避免同时重试风暴）
                let exp = 1u64 << attempt.saturating_sub(1).min(6);
                let jitter = (attempt as u64).wrapping_mul(37) % 50;
                let delay_ms = base_delay_ms.saturating_mul(exp).saturating_add(jitter);
                eprintln!(
                    "[LLM] 请求失败（可重试，{}/{})，{}ms 后重试: {}",
                    attempt,
                    max_retries,
                    delay_ms,
                    truncate(&msg, 120)
                );
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
        }
    }
}

/// 单次 POST（无重试）。Err 返回 (可读消息, 是否可重试)。
fn post_json_once(
    endpoint: &str,
    api_key: &str,
    body: &Value,
    timeout_secs: u64,
) -> Result<String, (String, bool)> {
    let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(timeout_secs.max(1)))
            .build();

    let body_str = serde_json::to_string(body)
        .map_err(|e| (format!("序列化请求失败: {}", e), false))?;

    match agent
        .post(endpoint)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", api_key))
        .send_string(&body_str)
    {
        Ok(resp) => {
            // ureq 2：into_reader() 返回 Box<dyn Read>，用 take 限制响应体大小
            let mut buf = Vec::new();
            resp.into_reader()
                .take(4 * 1024 * 1024) // 4 MiB 截断保护
                .read_to_end(&mut buf)
                .map_err(|e| (format!("读取响应失败: {}", e), false))?;
            String::from_utf8(buf).map_err(|e| (format!("响应非 UTF-8: {}", e), false))
        }
        Err(ureq::Error::Status(code, response)) => {
            // HTTP 错误时透传响应体，便于定位 API 拒绝原因
            let mut buf = Vec::new();
            let _ = response.into_reader().take(1024 * 1024).read_to_end(&mut buf);
            let msg = String::from_utf8_lossy(&buf);
            let retryable = code == 429 || (500..=599).contains(&code);
            Err((
                format!("请求模型失败: HTTP {}: {}", code, truncate(&msg, 500)),
                retryable,
            ))
        }
        // 网络 / 超时等传输层错误：可重试
        Err(other) => Err((format!("请求模型失败: {}", other), true)),
    }
}

/// 字符串截断：超长时保留头部并标注丢弃长度。
/// 注意：按 char 边界截断，避免 UTF-8 中文字符被字节切片切断导致 panic。
fn truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let head: String = chars[..max_chars].iter().collect();
        format!("{}…(+{}chars)", head, chars.len() - max_chars)
    }
}

/// ChatProvider 实现：run_tool_loop 通过 trait 调用真实客户端。
impl ChatProvider for LlmClient {
    fn chat(&self, messages: &[LlmMessage], tools: &Value) -> Result<ChatResponse, String> {
        self.chat_with_tools(messages, tools)
    }
}

// 为 read_to_end 引入 trait
use std::io::Read;

/// 便捷构造：环境变量 key（供 e2e 测试使用，故豁免 dead_code）。
#[allow(dead_code)]
pub fn client_from_env() -> Option<LlmClient> {
    LlmConfig::from_env().map(LlmClient::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 离线静态：无环境变量时不构造（不触网络）。
    #[test]
    fn from_env_returns_none_without_key() {
        // 测试前不设置环境变量；若 CI 已设置则跳过
        if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            return;
        }
        assert!(LlmConfig::from_env().is_none());
    }

    #[test]
    fn with_key_keeps_key_in_memory_only() {
        let cfg = LlmConfig::with_key("sk-test-placeholder".into());
        assert_eq!(cfg.api_key, "sk-test-placeholder");
    }

    #[test]
    fn truncate_shortens_long_strings() {
        assert_eq!(truncate("short", 10), "short");
        assert!(truncate(&"x".repeat(1000), 100).contains("+900"));
    }
}
