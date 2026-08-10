//! `builtin:chart` —— Mermaid 图表生成（对标大厂 Agent 的可视化能力）。
//!
//! 让 AI 生成 Mermaid 语法，保存为 .mmd 文件到附件目录。
//! 前端检测 .mmd → 渲染为 SVG/PNG（或导出 mermaid.ink 链接）。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const CHART_TYPES: &[(&str, &str)] = &[
    ("flowchart", "流程图"),
    ("sequence", "时序图"),
    ("class", "类图"),
    ("state", "状态图"),
    ("gantt", "甘特图"),
    ("pie", "饼图"),
    ("graph", "节点图"),
    ("mindmap", "思维导图"),
    ("timeline", "时间线"),
    ("block", "方块图"),
    ("xy", "XY 图表（折线/柱状/散点）"),
];

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "chart",
        "生成 Mermaid 图表（流程图/时序图/甘特图/饼图/思维导图等），保存到附件目录。",
        vec![
            ToolParamDef {
                name: "mermaid".into(),
                param_type: "string".into(),
                description: "Mermaid 语法文本（graph TD; A-->B; 等）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "title".into(),
                param_type: "string".into(),
                description: "图表标题（用作文件名）".into(),
                required: false,
                enum_values: None,
                default: Some(json!("chart")),
            },
        ],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        let mermaid = args.get("mermaid").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("chart").to_string();
        if mermaid.is_empty() {
            return Box::pin(async { Ok(ToolResult::err("mermaid 参数不能为空")) });
        }
        Box::pin(async move {
            match save_chart(&mermaid, &title) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("图表保存失败: {e}"))),
            }
        })
    });

    registry.register(def, handler)
}

fn save_chart(mermaid: &str, title: &str) -> Result<serde_json::Value, String> {
    let dir = crate::executors::builtin::attachment::attach_dir()
        .map_err(|e| format!("获取附件目录失败: {e}"))?;

    // 推断图表类型
    let first_line = mermaid.lines().next().unwrap_or("").trim().to_lowercase();
    let chart_type = CHART_TYPES.iter()
        .find(|(t, _)| first_line.starts_with(t) || first_line.starts_with(&format!("{} ", t)))
        .map(|(_, label)| *label)
        .unwrap_or("图表");

    // 清理文件名
    let safe_title: String = title.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect::<String>()
        .trim()
        .replace(' ', "-");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let filename = format!("chart_{}_{}.mmd", safe_title, ts);
    let path = dir.join(&filename);
    std::fs::write(&path, mermaid).map_err(|e| format!("写入失败: {e}"))?;

    // 生成 mermaid.ink 在线预览链接
    let encoded = base64_url_encode(mermaid);
    let preview_url = format!("https://mermaid.ink/svg/{}", encoded);

    Ok(json!({
        "saved": path.to_string_lossy().to_string(),
        "type": chart_type,
        "file": filename,
        "previewUrl": preview_url,
        "lines": mermaid.lines().count(),
        "chars": mermaid.len(),
    }))
}

/// base64url 编码（不含 = 填充，mermaid.ink 需要）
fn base64_url_encode(input: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input.as_bytes())
}
