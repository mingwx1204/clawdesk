//! LLM 流式请求：在 Rust 侧发起 HTTP，绕过 WebView 的 CORS/CSP 限制。
//! 通过 Tauri 事件把 SSE 增量实时推给前端；支持取消。

use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Deserialize, Serialize)]
pub struct ChatMessageDto {
    pub role: String,
    /// 纯文本 或 多模态内容数组（图片等）
    #[serde(deserialize_with = "de_content_any")]
    pub content: serde_json::Value,
}

/// 兼容 String 与数组两种 content 形态
fn de_content_any<'de, D>(d: D) -> Result<serde_json::Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: serde_json::Value = serde::Deserialize::deserialize(d)?;
    Ok(v)
}

#[derive(Debug, Deserialize)]
pub struct LlmRequest {
    #[serde(rename = "apiBase")]
    pub api_base: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    pub model: String,
    pub messages: Vec<ChatMessageDto>,
    pub temperature: f64,
    #[serde(rename = "maxTokens")]
    pub max_tokens: u32,
    #[serde(rename = "topP")]
    pub top_p: f64,
    /// 推理模式：fast | standard | deep
    #[serde(default)]
    pub mode: String,
    /// 是否 DeepSeek 端点（决定 thinking 参数）
    #[serde(rename = "isDeepSeek", default)]
    pub is_deepseek: bool,
}

#[derive(Debug, Serialize)]
struct ChatBody<'a> {
    model: &'a str,
    messages: &'a [ChatMessageDto],
    temperature: f64,
    max_tokens: u32,
    top_p: f64,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<serde_json::Value>,
}

/// 请求 id -> 取消信号
pub struct LlmState(pub Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>);

impl Default for LlmState {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

fn req_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("llm-{n:x}")
}

/// 发起流式对话。立即返回 requestId，增量通过
/// `llm-delta-{id}` / `llm-done-{id}` / `llm-error-{id}` 事件推送。
#[tauri::command]
pub async fn llm_chat_start(
    app: AppHandle,
    state: tauri::State<'_, LlmState>,
    req: LlmRequest,
) -> AppResult<String> {
    let id = req_id();
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    state.0.lock().insert(id.clone(), cancel_tx);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| AppError::Other(format!("HTTP 客户端初始化失败: {e}")))?;

    let base = req.api_base.trim_end_matches('/');
    let url = format!("{base}/chat/completions");

    // DeepSeek 思考模式：temperature/top_p 不生效，thinking 参数控制
    let is_deep = req.mode == "deep" && req.is_deepseek;
    let thinking = if req.is_deepseek {
        Some(if is_deep {
            serde_json::json!({"type": "enabled"})
        } else {
            serde_json::json!({"type": "disabled"})
        })
    } else {
        None
    };

    let body = ChatBody {
        model: req.model.trim(),
        messages: &req.messages,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        top_p: req.top_p,
        stream: true,
        thinking,
    };

    let mut builder = client.post(&url).header("Authorization", format!("Bearer {}", req.api_key.trim()));
    // 思考模式下不发送 temp/top_p，避免混淆（与浏览器路径行为一致）
    if is_deep {
        let mut body_map = serde_json::to_value(&body).unwrap_or_default();
        if let Some(obj) = body_map.as_object_mut() {
            obj.remove("temperature");
            obj.remove("top_p");
        }
        builder = builder.json(&body_map);
    } else {
        builder = builder.json(&body);
    }

    let resp = builder.send().await.map_err(|e| {
            state.0.lock().remove(&id);
            AppError::Other(format!("连接模型服务失败: {e}"))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        state.0.lock().remove(&id);
        return Err(AppError::Other(format!("请求失败: HTTP {status} {text}")));
    }

    let eid = id.clone();
    let app_state = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        loop {
            tokio::select! {
                _ = &mut cancel_rx => {
                    let _ = app_state.emit(&format!("llm-done-{eid}"), "");
                    break;
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                            // 按行解析 SSE
                            while let Some(pos) = buf.find('\n') {
                                let line = buf[..pos].trim().to_string();
                                buf = buf[pos + 1..].to_string();
                                if !line.starts_with("data:") { continue; }
                                let data = line[5..].trim();
                                if data == "[DONE]" {
                                    let _ = app_state.emit(&format!("llm-done-{eid}"), "");
                                    buf.clear();
                                    break;
                                }
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                    let delta = &json["choices"][0]["delta"];
                                    // 思考链（推理模型，如 DeepSeek V4-Pro / GLM 深度思考）
                                    if let Some(reasoning) = delta["reasoning_content"].as_str() {
                                        if !reasoning.is_empty() {
                                            let _ = app_state.emit(&format!("llm-reasoning-{eid}"), reasoning);
                                        }
                                    }
                                    if let Some(content) = delta["content"].as_str() {
                                        if !content.is_empty() {
                                            let _ = app_state.emit(&format!("llm-delta-{eid}"), content);
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            let _ = app_state.emit(&format!("llm-error-{eid}"), format!("读取流失败: {e}"));
                            break;
                        }
                        None => {
                            let _ = app_state.emit(&format!("llm-done-{eid}"), "");
                            break;
                        }
                    }
                }
            }
        }
        // 清理取消句柄
        if let Some(st) = app_state.try_state::<LlmState>() {
            st.0.lock().remove(&eid);
        }
    });

    Ok(id)
}

/// 中断进行中的流式请求
#[tauri::command]
pub fn llm_chat_cancel(state: tauri::State<'_, LlmState>, request_id: String) -> AppResult<()> {
    if let Some(tx) = state.0.lock().remove(&request_id) {
        let _ = tx.send(());
    }
    Ok(())
}

/// 查询账户余额（DeepSeek 兼容的 /user/balance 端点），返回原始 JSON。
/// 端点不存在时返回错误，前端降级隐藏。
#[tauri::command]
pub async fn llm_balance(api_base: String, api_key: String) -> AppResult<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::Other(format!("HTTP 客户端初始化失败: {e}")))?;
    let base = api_base.trim_end_matches('/');
    // DeepSeek 的余额端点在 API 根（不含 /v1）
    let root = base.strip_suffix("/v1").unwrap_or(base);
    let resp = client
        .get(format!("{root}/user/balance"))
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| AppError::Other(format!("查询余额失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Other(format!("余额查询不支持或失败: HTTP {}", resp.status())));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| AppError::Other(format!("余额响应解析失败: {e}")))
}
