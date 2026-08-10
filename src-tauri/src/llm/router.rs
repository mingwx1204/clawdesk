//! 多模型路由中间层（项目 4，文档 §八）。
//!
//! 设计说明：
//! - **职责分离**：文本推理 / 规划 / 工具选择固定走主模型（DeepSeek-V4），
//!   `analyze_image` 路由至视觉专用模型（GLM-5V 等），`generate_image`
//!   路由至绘图 API（Flux / SD 等），不打乱原有 ReAct 主循环结构；
//! - **统一 OpenAI 协议适配**（§八.4）：所有第三方模型统一走
//!   `POST {endpoint}` + Bearer key + JSON body，返回统一封装为
//!   `VisionResult` / `ImageResult` 固定结构体，DeepSeek 无需适配异构返回；
//! - **故障降级**（§八.2）：识图 / 生图第三方 API 限流、欠费、宕机时，
//!   返回 `degraded=true` 的标准化结果（含可读原因），工具层据此回传
//!   降级消息供 DeepSeek 调整方案（改文字描述替代图像操作），不中断任务；
//!   故障记录独立存入本地路由状态，可在设置面板查看模型接口运行状态；
//! - **主模型负载切换**（§八.3）：`set_main_model` 支持 V4-Pro ↔ V4-Flash
//!   动态切换，切换实时同步至 ReAct 上下文（环境快照展示当前模型）。

use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::client::{post_json_to, LlmClient};
use super::{ChatResponse, LlmMessage};
use crate::llm::runner::ChatProvider;

/// 全局路由单例：由 AppState 初始化，执行器层（analyze_image / generate_image）
/// 通过 `global()` 获取。设计原因：执行器注册签名固定（DEV_SPEC 执行器层
/// 只增不改），无法注入路由引用；全局单例保证不触碰 core / 注册链。
static GLOBAL: OnceLock<Arc<ModelRouter>> = OnceLock::new();

/// 初始化全局路由（应用启动时调用一次；重复调用忽略）。
pub fn init_global(router: Arc<ModelRouter>) {
    let _ = GLOBAL.set(router);
}

/// 获取全局路由（未初始化返回 None —— 执行器据此走降级路径）。
pub fn global() -> Option<Arc<ModelRouter>> {
    GLOBAL.get().cloned()
}

/// 绘图 API 配置（OpenAI images/generations 兼容端点）。
#[derive(Debug, Clone)]
pub struct ImageGenConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

/// 路由配置快照（不含任何 key，供前端 / 环境快照展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterStatus {
    pub main_model: String,
    pub vision_configured: bool,
    pub vision_model: Option<String>,
    pub image_configured: bool,
    pub image_model: Option<String>,
    /// 最近路由故障（上限 20 条，环形覆盖）。
    pub recent_faults: Vec<String>,
}

/// 视觉分析统一结果（§八.4 固定结构体）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionResult {
    /// 视觉模型返回的结构化文本（图像描述 / 文字识别 / 画面元素）。
    pub text: String,
    /// 实际使用的模型名。
    pub model: String,
    /// 是否降级（未配置视觉模型 / API 故障时为 true）。
    pub degraded: bool,
    /// 降级原因（degraded=true 时提供可读说明，供 DeepSeek 调整方案）。
    pub note: Option<String>,
}

/// 绘图统一结果（§八.4 固定结构体）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageResult {
    /// base64 编码的图像数据（无 data: 前缀），或 URL。
    pub data: String,
    /// "b64" 或 "url"。
    pub data_kind: String,
    /// 实际使用的模型名。
    pub model: String,
    /// 是否降级（未配置绘图 API / API 故障时为 true）。
    pub degraded: bool,
    /// 降级原因。
    pub note: Option<String>,
}

/// 模型路由中间层。
pub struct ModelRouter {
    /// 主决策模型（DeepSeek-V4，文本推理 / 规划 / 工具选择）。
    main_client: RwLock<Option<LlmClient>>,
    /// 视觉专用模型（GLM-5V-Turbo 等，OpenAI vision 兼容）。
    vision_client: RwLock<Option<LlmClient>>,
    /// 绘图专用 API（Flux / SD 系列，OpenAI images/generations 兼容）。
    image_config: RwLock<Option<ImageGenConfig>>,
    /// 路由故障记录（环形，上限 20 条）。
    faults: RwLock<Vec<String>>,
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelRouter {
    pub fn new() -> Self {
        Self {
            main_client: RwLock::new(None),
            vision_client: RwLock::new(None),
            image_config: RwLock::new(None),
            faults: RwLock::new(Vec::new()),
        }
    }

    // ────────────────────────────────
    // 配置
    // ────────────────────────────────

    /// 确保主模型配置就绪：仅更新 API Key，**保留当前模型名**。
    ///
    /// 供 `agent_chat` 每次调用时刷新 key（key 由前端传入，仅内存态）；
    /// 不会覆盖用户通过 `router_set_main_model` 切换的模型（V4-Pro ↔ V4-Flash）。
    pub fn ensure_main_key(&self, api_key: String) {
        let mut slot = self.main_client.write().unwrap();
        match slot.as_mut() {
            Some(client) => client.update_api_key(api_key),
            None => *slot = Some(LlmClient::new(crate::llm::client::LlmConfig::with_key(api_key))),
        }
    }

    /// 配置主模型（key 仅内存态；重复调用更新模型名/端点）。
    pub fn set_main(&self, api_key: String, model: &str, endpoint: Option<&str>) {
        let mut slot = self.main_client.write().unwrap();
        match slot.as_mut() {
            Some(client) => client.update_api_key(api_key),
            None => *slot = Some(LlmClient::new(crate::llm::client::LlmConfig::with_key(api_key))),
        }
        if let Some(client) = slot.as_mut() {
            client.update_model(model, endpoint);
        }
    }

    /// 切换主模型（V4-Pro ↔ V4-Flash，§八.3）；未配置主模型时无实际客户端可切换。
    pub fn set_main_model(&self, model: &str) {
        let mut slot = self.main_client.write().unwrap();
        if let Some(client) = slot.as_mut() {
            client.update_model(model, None);
        }
        // 锁在作用域结束自动释放；主模型未配置 key 时仅静默忽略
    }

    /// 取主模型客户端副本（供 agent_subtask 子 Agent 复用；未配置返回 None）。
    pub fn main_client(&self) -> Option<LlmClient> {
        self.main_client.read().unwrap().clone()
    }

    /// 配置视觉模型（GLM-5V-Turbo 等 OpenAI vision 兼容端点）。
    pub fn configure_vision(&self, api_key: String, model: &str, endpoint: &str) {
        let mut cfg = crate::llm::client::LlmConfig::with_key(api_key);
        cfg.model = model.to_string();
        cfg.endpoint = endpoint.to_string();
        *self.vision_client.write().unwrap() = Some(LlmClient::new(cfg));
    }

    /// 配置绘图 API（Flux / SD 系列 OpenAI images/generations 兼容端点）。
    pub fn configure_image(&self, api_key: String, model: &str, endpoint: &str) {
        *self.image_config.write().unwrap() = Some(ImageGenConfig {
            endpoint: endpoint.to_string(),
            api_key,
            model: model.to_string(),
        });
    }

    /// 当前路由状态快照（不含 key，供前端设置面板 / 环境快照）。
    pub fn status(&self) -> RouterStatus {
        let main_model = self
            .main_client
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.config_summary()["model"].as_str().unwrap_or("").to_string())
            .unwrap_or_else(|| "（未配置）".to_string());
        let vision = self.vision_client.read().unwrap();
        let image = self.image_config.read().unwrap();
        let faults = self.faults.read().unwrap().clone();
        RouterStatus {
            main_model,
            vision_configured: vision.is_some(),
            vision_model: vision
                .as_ref()
                .map(|c| c.config_summary()["model"].as_str().unwrap_or("").to_string()),
            image_configured: image.is_some(),
            image_model: image.as_ref().map(|c| c.model.clone()),
            recent_faults: faults,
        }
    }

    /// 记录一次路由故障（环形上限 20 条）。
    fn record_fault(&self, category: &str, detail: &str) {
        let mut faults = self.faults.write().unwrap();
        faults.push(format!("[{}] {}", category, detail));
        if faults.len() > 20 {
            let excess = faults.len() - 20;
            faults.drain(0..excess);
        }
    }

    // ────────────────────────────────
    // 视觉路由（analyze_image 使用）
    // ────────────────────────────────

    /// 识图：路由至视觉专用模型。未配置视觉模型 / API 故障时返回降级结果
    /// （degraded=true + 可读原因），由 analyze_image 执行器回传 DeepSeek。
    pub fn vision(
        &self,
        image_b64: &str,
        mime: &str,
        prompt: &str,
    ) -> VisionResult {
        let Some(client) = self.vision_client.read().unwrap().clone() else {
            let note = "未配置视觉模型（可在设置中配置 GLM-5V 等视觉 API 后启用真实识图）".to_string();
            self.record_fault("vision", "未配置视觉模型");
            return VisionResult {
                text: String::new(),
                model: "（未配置）".into(),
                degraded: true,
                note: Some(note),
            };
        };

        let model = client.config_summary()["model"].as_str().unwrap_or("").to_string();
        match client.chat_vision(image_b64, mime, prompt) {
            Ok(resp) => {
                let text = super::extract_text(&resp);
                crate::llm::logging::debug(
                    "vision",
                    &format!("✅ 识图成功 model={} 图片大小={}KB 结果={}字符", model, image_b64.len() / 1024, text.chars().count()),
                );
                VisionResult {
                    text,
                    model,
                    degraded: false,
                    note: None,
                }
            }
            Err(e) => {
                crate::llm::logging::debug("vision", &format!("❌ 识图失败 model={} err={}", model, e));
                let note = format!("视觉模型调用失败: {}。请如实告知用户，可改用文字描述图片内容", e);
                self.record_fault("vision", &e);
                VisionResult {
                    text: String::new(),
                    model,
                    degraded: true,
                    note: Some(note),
                }
            }
        }
    }

    // ────────────────────────────────
    // 绘图路由（generate_image 使用）
    // ────────────────────────────────

    /// 生图：路由至绘图 API（OpenAI images/generations 兼容）。
    /// 未配置绘图 API / API 故障时返回降级结果，由 generate_image 执行器
    /// 回退占位图并告知 DeepSeek。
    pub fn image(&self, prompt: &str, width: u32, height: u32) -> ImageResult {
        let Some(cfg) = self.image_config.read().unwrap().clone() else {
            let note = "未配置绘图 API（可在设置中配置 Flux / SD 等绘图服务后启用真实生图）".to_string();
            self.record_fault("image", "未配置绘图 API");
            return ImageResult {
                data: String::new(),
                data_kind: "b64".into(),
                model: "（未配置）".into(),
                degraded: true,
                note: Some(note),
            };
        };

        let mut body = json!({
            "model": cfg.model,
            "prompt": prompt,
            "size": format!("{}x{}", width.max(64).min(2048), height.max(64).min(2048)),
            "n": 1,
        });
        // ★ 智谱（bigmodel.cn）CogView 不支持 response_format 参数（传了会 400）
        if !cfg.endpoint.contains("bigmodel.cn") {
            if let Some(obj) = body.as_object_mut() {
                obj.insert("response_format".into(), serde_json::Value::String("b64_json".into()));
            }
        }

        match post_json_to(&cfg.endpoint, &cfg.api_key, &body, 90) {
            Ok(text) => match parse_image_response(&text) {
                Ok((data, kind)) => {
                    crate::llm::logging::debug(
                        "image",
                        &format!("✅ 生图成功 model={} 数据={}字节 kind={}", cfg.model, data.len(), kind),
                    );
                    ImageResult {
                        data,
                        data_kind: kind,
                        model: cfg.model.clone(),
                        degraded: false,
                        note: None,
                    }
                }
                Err(e) => {
                    crate::llm::logging::debug("image", &format!("❌ 生图解析失败 model={} err={}", cfg.model, e));
                    let note = format!("绘图 API 返回解析失败: {}", e);
                    self.record_fault("image", &note);
                    ImageResult {
                        data: String::new(),
                        data_kind: "b64".into(),
                        model: cfg.model.clone(),
                        degraded: true,
                        note: Some(note),
                    }
                }
            },
            Err(e) => {
                crate::llm::logging::debug("image", &format!("❌ 生图调用失败 model={} err={}", cfg.model, e));
                let note = format!("绘图 API 调用失败: {}", e);
                self.record_fault("image", &e);
                ImageResult {
                    data: String::new(),
                    data_kind: "b64".into(),
                    model: cfg.model.clone(),
                    degraded: true,
                    note: Some(note),
                }
            }
        }
    }
}

/// 解析 OpenAI images/generations 响应：优先 b64_json，其次 url。
fn parse_image_response(text: &str) -> Result<(String, String), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("响应非 JSON: {}", e))?;
    let data = v
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .ok_or_else(|| "响应缺少 data 数组".to_string())?;

    if let Some(b64) = data.get("b64_json").and_then(|b| b.as_str()) {
        return Ok((b64.to_string(), "b64".into()));
    }
    if let Some(url) = data.get("url").and_then(|u| u.as_str()) {
        return Ok((url.to_string(), "url".into()));
    }
    Err("响应 data 项缺少 b64_json 或 url".into())
}

/// ChatProvider 实现：runner 直接以 ModelRouter 作为主模型提供者。
/// 文本推理 / 规划 / 工具选择固定走主模型（DeepSeek-V4），与图像路由隔离。
impl ChatProvider for ModelRouter {
    fn chat(&self, messages: &[LlmMessage], tools: &Value) -> Result<ChatResponse, String> {
        let guard = self.main_client.read().unwrap();
        let client = guard
            .as_ref()
            .ok_or_else(|| "主模型未配置（缺少 API Key）".to_string())?;
        client.chat_with_tools(messages, tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_without_config_reports_unconfigured() {
        let r = ModelRouter::new();
        let s = r.status();
        assert!(!s.vision_configured);
        assert!(!s.image_configured);
        assert!(s.vision_model.is_none());
        assert!(s.image_model.is_none());
    }

    #[test]
    fn configure_vision_then_status() {
        let r = ModelRouter::new();
        r.configure_vision("sk-v".into(), "glm-5v-turbo", "https://api.z.ai/chat/completions");
        let s = r.status();
        assert!(s.vision_configured);
        assert_eq!(s.vision_model.as_deref(), Some("glm-5v-turbo"));
    }

    #[test]
    fn configure_image_then_status() {
        let r = ModelRouter::new();
        r.configure_image("sk-i".into(), "flux-1", "https://api.siliconflow.cn/images/generations");
        let s = r.status();
        assert!(s.image_configured);
        assert_eq!(s.image_model.as_deref(), Some("flux-1"));
    }

    #[test]
    fn vision_without_config_degrades() {
        let r = ModelRouter::new();
        let out = r.vision("AAAA", "image/png", "描述图片");
        assert!(out.degraded);
        assert!(out.note.as_deref().unwrap().contains("未配置视觉模型"));
        // 故障被记录
        assert_eq!(r.status().recent_faults.len(), 1);
    }

    #[test]
    fn image_without_config_degrades() {
        let r = ModelRouter::new();
        let out = r.image("一只猫", 512, 512);
        assert!(out.degraded);
        assert!(out.note.as_deref().unwrap().contains("未配置绘图 API"));
        assert_eq!(r.status().recent_faults.len(), 1);
    }

    #[test]
    fn parse_image_response_b64() {
        let text = r#"{"data":[{"b64_json":"QUJD","revised_prompt":"x"}]}"#;
        let (data, kind) = parse_image_response(text).unwrap();
        assert_eq!(data, "QUJD");
        assert_eq!(kind, "b64");
    }

    #[test]
    fn parse_image_response_url() {
        let text = r#"{"data":[{"url":"https://example.com/a.png"}]}"#;
        let (data, kind) = parse_image_response(text).unwrap();
        assert_eq!(data, "https://example.com/a.png");
        assert_eq!(kind, "url");
    }

    #[test]
    fn parse_image_response_malformed() {
        assert!(parse_image_response("not json").is_err());
        assert!(parse_image_response(r#"{"data":[]}"#).is_err());
    }

    #[test]
    fn main_model_switch_updates_status() {
        let r = ModelRouter::new();
        r.set_main("sk-m".into(), "deepseek-chat", None);
        r.set_main_model("deepseek-reasoner");
        assert_eq!(r.status().main_model, "deepseek-reasoner");
    }

    #[test]
    fn chat_without_main_key_errors() {
        let r = ModelRouter::new();
        let err = r.chat(&[], &Value::Array(vec![])).unwrap_err();
        assert!(err.contains("未配置"));
    }
}
