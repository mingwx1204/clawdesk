use serde::{Deserialize, Serialize};

use super::error::ToolError;

/// 工具参数定义（JSON Schema 风格）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolParamDef {
    pub name: String,
    /// 参数类型：string / number / boolean / object / array。
    #[serde(rename = "type")]
    pub param_type: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// 统一工具定义 —— 所有工具（内置 / MCP / SkillHub / 窗口控制等）的唯一数据结构。
///
/// 契约（DEV_SPEC.md §5）：
/// - `id` 必须等于 `source:name`，注册时强制校验；
/// - `source` 为动态字符串，**禁止硬编码枚举**，运行时按需扩展；
/// - `ui_payload` 仅用于前端渲染，绝不混入 LLM 上下文。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedToolDef {
    /// 工具唯一 ID：`source:name`。
    pub id: String,
    /// 工具来源（builtin / mcp / skillhub / ...，运行时动态）。
    pub source: String,
    /// 工具名，不得包含 `:`。
    pub name: String,
    pub description: String,
    pub params: Vec<ToolParamDef>,
    /// 高危标记：安全中间件（阶段 2）消费，用于触发用户确认。
    #[serde(default)]
    pub is_high_risk: bool,
    #[serde(default)]
    pub version: String,
    /// 前端渲染载荷 —— 仅渲染通道使用，绝不进入 LLM 上下文。
    #[serde(default)]
    pub ui_payload: Option<serde_json::Value>,
    /// 扩展元数据。
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl UnifiedToolDef {
    /// 便捷构造：自动生成合规的 `id = source:name`，并拒绝非法输入。
    ///
    /// 说明：此构造器已做基本校验，但注册表仍会在 `register` 时
    /// 对已构造的 def 再执行一次 `validate_id`，防止手工构造绕过。
    pub fn new(
        source: &str,
        name: &str,
        description: &str,
        params: Vec<ToolParamDef>,
    ) -> Result<Self, ToolError> {
        if source.is_empty() || name.is_empty() {
            return Err(ToolError::invalid_def("source 与 name 均不能为空"));
        }
        if name.contains(':') {
            return Err(ToolError::invalid_def("name 不能包含 ':' 字符"));
        }
        let id = format!("{}:{}", source, name);
        Ok(Self {
            id,
            source: source.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            params,
            is_high_risk: false,
            version: "0.1.0".into(),
            ui_payload: None,
            metadata: serde_json::Map::new(),
        })
    }

    /// 校验 `id` 符合 `source:name` 规范 —— 注册时强制调用（DEV_SPEC.md §6）。
    pub fn validate_id(&self) -> Result<(), ToolError> {
        let expected = format!("{}:{}", self.source, self.name);
        if self.id != expected {
            return Err(ToolError::invalid_def(format!(
                "工具 id `{}` 不符合 source:name 规范，应为 `{}`",
                self.id, expected
            )));
        }
        Ok(())
    }

    /// 将定义标记为高危工具（供安全中间件拦截确认）。
    pub fn high_risk(mut self) -> Self {
        self.is_high_risk = true;
        self
    }

    /// 附加前端渲染载荷（仅渲染通道，见 DEV_SPEC.md §8）。
    pub fn with_ui_payload(mut self, payload: serde_json::Value) -> Self {
        self.ui_payload = Some(payload);
        self
    }

    /// 附加扩展元数据。
    pub fn with_metadata(mut self, key: &str, value: serde_json::Value) -> Self {
        self.metadata.insert(key.to_string(), value);
        self
    }
}
