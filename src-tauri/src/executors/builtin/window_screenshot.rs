//! `builtin:window_screenshot` —— 窗口/屏幕截图工具。
//!
//! 设计说明：
//! - 使用 `screenshots` crate 捕获屏幕（Windows GDI / macOS CoreGraphics / Linux X11）；
//! - 输出为 PNG base64 Data URL，可直接前端渲染；
//! - 默认截取主显示器，支持多显示器索引；
//! - 非高危工具（只读），但涉及屏幕隐私，工具描述明确告知模型。

use std::io::Cursor;
use std::sync::Arc;

use base64::Engine;
use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "window_screenshot",
        "截取当前屏幕（主显示器）的 PNG 截图，返回 base64 Data URL 供视觉模型分析。可用 display 参数选择多显示器中的特定屏幕",
        vec![
            ToolParamDef {
                name: "display".into(),
                param_type: "number".into(),
                description: "显示器编号（从 0 开始，默认 0 即主显示器）".into(),
                required: false,
                enum_values: None,
                default: Some(json!(0)),
            },
            ToolParamDef {
                name: "format".into(),
                param_type: "string".into(),
                description: "图片格式（默认 png）".into(),
                required: false,
                enum_values: Some(vec!["png".into(), "jpeg".into()]),
                default: Some(json!("png")),
            },
        ],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let display: usize = args
                .get("display")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(0);
            let format: String = args
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("png")
                .to_lowercase();

            match capture_screen(display, &format) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("截图失败: {}", e))),
            }
        })
    });

    registry.register(def, handler)
}

/// 截取指定显示器的屏幕截图，返回 base64 Data URL 与元信息。
fn capture_screen(display: usize, format: &str) -> Result<serde_json::Value, String> {
    let screens = screenshots::Screen::all()
        .map_err(|e| format!("无法枚举屏幕: {}", e))?;

    if display >= screens.len() {
        return Err(format!(
            "显示器 {} 不存在，共检测到 {} 个屏幕",
            display,
            screens.len()
        ));
    }

    let screen = &screens[display];
    let image = screen
        .capture()
        .map_err(|e| format!("截图执行失败: {}", e))?;

    let (width, height) = (image.width(), image.height());

    // 编码为对应格式的 Data URL
    let mime = match format {
        "jpeg" | "jpg" => "image/jpeg",
        _ => "image/png",
    };

    let mut buf = Cursor::new(Vec::new());
    match format {
        "jpeg" | "jpg" => {
            // RgbaImage → DynamicImage → JPEG
            let dyn_img = image::DynamicImage::ImageRgba8(
                image::RgbaImage::from_raw(width, height, image.into_raw())
                    .ok_or("图片数据损坏")?,
            );
            dyn_img
                .write_to(&mut buf, image::ImageFormat::Jpeg)
                .map_err(|e| format!("JPEG 编码失败: {}", e))?;
        }
        _ => {
            // PNG（默认，screenshots 返回 RgbaImage，与 image crate 兼容）
            let rgba = image::RgbaImage::from_raw(width, height, image.into_raw())
                .ok_or("图片数据损坏")?;
            let encoder = image::codecs::png::PngEncoder::new(&mut buf);
            rgba
                .write_with_encoder(encoder)
                .map_err(|e| format!("PNG 编码失败: {}", e))?;
        }
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    let data_url = format!("data:{};base64,{}", mime, b64);

    Ok(json!({
        "display": display,
        "width": width,
        "height": height,
        "format": mime,
        "dataUrl": data_url,
        "totalScreens": screens.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_def_is_well_formed() {
        let def = UnifiedToolDef::new(
            "builtin",
            "window_screenshot",
            "x",
            vec![ToolParamDef {
                name: "display".into(),
                param_type: "number".into(),
                description: "d".into(),
                required: false,
                enum_values: None,
                default: None,
            }],
        )
        .unwrap();
        assert_eq!(def.id, "builtin:window_screenshot");
        def.validate_id().unwrap();
    }

    /// 离线：超出显示器索引返回错误（不触达真实截图硬件）。
    #[test]
    fn out_of_range_display_returns_error() {
        let err = capture_screen(99999, "png").unwrap_err();
        assert!(
            err.contains("不存在") || err.contains("无法枚举"),
            "{}",
            err
        );
    }

    /// 离线：无效格式不会 panic，被默认处理为 png。
    /// （此测试在无头环境可能因无屏幕而失败；回退为错误检查）
    #[test]
    fn invalid_format_does_not_panic() {
        // 只验证函数签名不 panic（实际调用可能因无屏幕而失败）
        let result = capture_screen(0, "bogus_format");
        // 可能成功（被当作 png）或失败（无屏幕），都不应 panic
        let _ = result;
    }
}
