//! `builtin:analyze_image` —— 识图工具。
//!
//! 设计说明：
//! - 读取本地图片 → 压缩 → base64 → **路由至视觉专用模型**（GLM-5V 等，
//!   项目 4 多模型路由）；视觉模型返回结构化描述（图像内容/文字识别/画面元素）；
//! - base64 只发往视觉模型端点，**绝不回传 DeepSeek 主模型上下文**
//!   （超大 base64 会导致请求体爆炸、模型"思考中"卡死 —— 已修复的 BUG）；
//! - 未配置视觉模型 / 视觉 API 故障 → 降级返回结构化元信息 + 明确提示，
//!   DeepSeek 据此如实告知用户或改用文字描述；
//! - 路径安全：拒绝读取系统敏感目录（HighRiskGuard 统一校验 + 此处双保险）。

use std::path::Path;
use std::sync::Arc;

use image::imageops::FilterType;
use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// uiPayload 契约：仅前端渲染通道消费。
const UI_PAYLOAD: &str = r#"{"displayHint":{"icon":"🖼️","tone":"info","note":"识别本地图片内容"}}"#;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "analyze_image",
        "读取本地图片并路由至视觉模型解析内容（未配置视觉模型时返回图片元信息与降级提示）",
        vec![ToolParamDef {
            name: "image_path".into(),
            param_type: "string".into(),
            description: "本地图片绝对路径（支持 png/jpg/jpeg/bmp/webp）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?
    .with_ui_payload(serde_json::from_str(UI_PAYLOAD).unwrap());

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let image_path = args
                .get("image_path")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if image_path.is_empty() {
                return Ok(ToolResult::err("image_path 不能为空"));
            }
            // 防御性路径安全（HighRiskGuard 已统一校验，此处双保险）
            if is_sensitive_path(image_path) {
                return Ok(ToolResult::err("禁止读取系统敏感路径"));
            }

            match analyze_image(image_path) {
                Ok(output) => Ok(ToolResult::ok(output)),
                Err(msg) => Ok(ToolResult::err(format!("识图失败: {}", msg))),
            }
        })
    });

    registry.register(def, handler)
}

/// 识图主入口：优先视觉模型路由，未配置 / 失败时降级元信息。
fn analyze_image(image_path: &str) -> Result<serde_json::Value, String> {
    // 1) 读取并压缩图片（得到压缩后尺寸 + base64，供视觉模型 / 元信息共用）
    let (w, h, mime, b64) = read_and_compress(image_path)?;

    // 2) 路由至视觉模型（若已配置）
    if let Some(router) = crate::llm::router::global() {
        let prompt = "请用简体中文详细描述这张图片的内容：画面主体、场景、文字内容（若有）、颜色构成与整体风格。返回结构化描述。";
        let out = router.vision(&b64, &mime, prompt);
        if !out.degraded {
            return Ok(json!({
                "imagePath": image_path,
                "width": w,
                "height": h,
                "mimeType": mime,
                "model": out.model,
                "description": out.text,
                "note": "已通过视觉模型解析",
            }));
        }
        // 视觉模型故障：降级，把故障原因回传（供 DeepSeek 如实告知 / 调整方案）
        return Ok(json!({
            "imagePath": image_path,
            "width": w,
            "height": h,
            "mimeType": mime,
            "description": null,
            "note": out.note.unwrap_or_else(|| "视觉模型解析失败".to_string()),
        }));
    }

    // 3) 未配置视觉模型：降级元信息
    Ok(json!({
        "imagePath": image_path,
        "width": w,
        "height": h,
        "mimeType": mime,
        "description": null,
        "note": "当前未配置视觉模型（可在设置中配置 GLM-5V 等视觉 API 后启用真实识图）。请如实告知用户：可请用户直接描述图片内容。",
    }))
}

/// 读取图片 → 压缩（最长边 1024）→ 返回 (宽, 高, MIME, base64)。
/// base64 仅用于视觉模型请求，不进入 DeepSeek 上下文。
fn read_and_compress(image_path: &str) -> Result<(u32, u32, String, String), String> {
    let path = Path::new(image_path);
    if !path.exists() {
        return Err(format!("文件不存在: {}", image_path));
    }

    let img = image::open(path).map_err(|e| format!("无法解码图片（{}）: {}", image_path, e))?;
    let (orig_w, orig_h) = (img.width(), img.height());

    // 压缩（视觉模型输入过长边 1024，控制 base64 体积）
    let max_side = 1024u32;
    let (w, h) = if orig_w.max(orig_h) > max_side {
        let scale = max_side as f32 / orig_w.max(orig_h) as f32;
        (
            ((orig_w as f32 * scale) as u32).max(1),
            ((orig_h as f32 * scale) as u32).max(1),
        )
    } else {
        (orig_w, orig_h)
    };
    let resized = img.resize(w, h, FilterType::Lanczos3);

    // 编码 PNG → base64（与视觉端点协商统一 image/png）
    use base64::Engine;
    let mut png_buf = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png_buf);
        resized
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| format!("PNG 编码失败: {}", e))?;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_buf);
    let mime = "image/png".to_string();

    Ok((w, h, mime, b64))
}

/// 系统敏感路径检查（双保险：中间件 + 执行器内）。
pub fn is_sensitive_path(p: &str) -> bool {
    let lower = p.to_lowercase();
    let sensitive_markers = [
        "c:\\windows",
        "c:\\program files",
        "c:\\programdata",
        "/etc/",
        "/usr/",
        "/bin/",
        "/boot/",
        "/sys/",
        "/proc/",
        "/dev/",
        "\\.ssh",
        "/.ssh/",
    ];
    sensitive_markers.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_path_detection() {
        assert!(is_sensitive_path("C:\\Windows\\System32\\config\\SAM"));
        assert!(is_sensitive_path("/etc/shadow"));
        assert!(is_sensitive_path("C:\\Program Files\\x"));
        assert!(is_sensitive_path("/home/user/.ssh/id_rsa"));
        assert!(!is_sensitive_path("C:\\Users\\me\\Pictures\\cat.jpg"));
        assert!(!is_sensitive_path("D:\\work\\photo.png"));
    }

    #[test]
    fn missing_file_returns_error() {
        let err = analyze_image("D:/definitely-no-such-file-xyz.png").unwrap_err();
        assert!(err.contains("文件不存在"));
    }

    /// 离线：生成临时图片 → 未配置视觉模型时降级返回元信息 + 降级提示。
    #[test]
    fn process_local_image_ok() {
        let dir = std::env::temp_dir().join(format!("clawdesk-ai-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("test.png");
        image::RgbaImage::from_pixel(64, 64, image::Rgba([200u8, 30, 30, 255]))
            .save(&p)
            .unwrap();

        let out = analyze_image(p.to_str().unwrap()).unwrap();
        assert_eq!(out["width"], 64);
        assert_eq!(out["height"], 64);
        assert!(out["note"].as_str().unwrap().contains("未配置视觉模型"));
        // 关键：base64 不进入结果（不撑爆 DeepSeek 上下文）
        assert!(out.get("imageBase64").is_none());
        assert!(out.get("dataUrl").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
