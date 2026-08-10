//! `builtin:generate_image` —— 图像生成执行器（阶段 3 + 项目 4 绘图路由）。
//!
//! 设计说明：
//! - **优先路由至绘图 API**（项目 4 多模型路由：Flux / SD 系列，OpenAI
//!   images/generations 兼容端点），返回真实生成图像；
//! - 未配置绘图 API / API 故障 → **降级**为程序化占位生成器（prompt 哈希
//!   配色 + 同心圆图案），并在结果中注明降级原因，DeepSeek 据此如实告知；
//! - 图像保存到 `%TEMP%/clawdesk-generated/`，返回绝对路径 + dataUrl；
//! - 本工具参数契约（prompt / width / height）保持不变。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use image::{Rgba, RgbaImage};
use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// uiPayload 契约：仅前端渲染通道消费（DEV_SPEC.md §8）。
const UI_PAYLOAD: &str = r#"{"displayHint":{"icon":"🎨","tone":"accent","note":"当前为程序化占位图，真实引擎后续接入"}}"#;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "generate_image",
        "根据 prompt 生成图像：优先调用绘图 API（Flux/SD），未配置时降级为程序化占位图",
        vec![
            ToolParamDef {
                name: "prompt".into(),
                param_type: "string".into(),
                description: "图像描述（决定配色与图案）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "width".into(),
                param_type: "number".into(),
                description: "图像宽度（像素，默认 512）".into(),
                required: false,
                enum_values: None,
                default: Some(json!(512)),
            },
            ToolParamDef {
                name: "height".into(),
                param_type: "number".into(),
                description: "图像高度（像素，默认 512）".into(),
                required: false,
                enum_values: None,
                default: Some(json!(512)),
            },
        ],
    )?
    .with_ui_payload(serde_json::from_str(UI_PAYLOAD).unwrap());

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let prompt = args
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if prompt.is_empty() {
                return Ok(ToolResult::err("prompt 不能为空"));
            }

            let width = args
                .get("width")
                .and_then(|v| v.as_u64())
                .unwrap_or(512)
                .clamp(64, 2048) as u32;
            let height = args
                .get("height")
                .and_then(|v| v.as_u64())
                .unwrap_or(512)
                .clamp(64, 2048) as u32;

            match generate_image(prompt, width, height) {
                Ok((path, data_url, note)) => {
                    // ★ 完整 dataUrl 供前端直接显示图片（不再截断成残缺 base64）。
                    // 为防超大 base64 撑爆 LLM 上下文，note 中给出文件路径提示，
                    // LLM 侧仍只收到 path 文本（dataUrl 字段由前端消费，不进工具日志）。
                    Ok(ToolResult::ok(json!({
                        "prompt": prompt,
                        "width": width,
                        "height": height,
                        "path": path,
                        "dataUrl": data_url,
                        "note": note,
                    })))
                }
                Err(msg) => Ok(ToolResult::err(format!("图像生成失败: {}", msg))),
            }
        })
    });

    registry.register(def, handler)
}

/// 生成图像：返回 (文件绝对路径, dataUrl, note)。
///
/// 优先路由至绘图 API（项目 4 多模型路由）；未配置 / API 故障时
/// 降级为程序化占位图，并在 note 中注明降级原因（DeepSeek 据此如实告知）。
fn generate_image(prompt: &str, width: u32, height: u32) -> Result<(String, String, String), String> {
    // 1) 尝试路由至绘图 API（若已配置）
    if let Some(router) = crate::llm::router::global() {
        let out = router.image(prompt, width, height);
        if !out.degraded {
            let note = out
                .note
                .unwrap_or_else(|| format!("已通过绘图 API 生成（模型: {}）", out.model));
            return save_image_result(&out.data, prompt, &note);
        }
        // 绘图 API 故障：降级占位图，携带降级原因
        eprintln!("[IMAGE] 绘图 API 降级: {}", out.note.as_deref().unwrap_or("未知原因"));
    }

    // 2) 降级：程序化占位图
    let (path, data_url) = generate_placeholder(prompt, width, height)?;
    let note = "绘图 API 未配置或不可用，已降级为程序化占位图（可在设置中配置 Flux/SD 绘图服务启用真实生图）"
        .to_string();
    Ok((path, data_url, note))
}

/// 保存绘图 API 返回的图像（base64 或 URL），返回 (路径, dataUrl, note)。
/// ★ 探测真实图片格式（PNG/JPEG/GIF/WebP）→ 正确扩展名 + data_url mime，
///   避免 JPEG 字节存成 .png 导致识图回读"PNG 签名无效"。
fn save_image_result(
    data: &str,
    prompt: &str,
    note: &str,
) -> Result<(String, String, String), String> {
    // 输出目录：优先 D 盘数据目录下 generated-images，避免塞 C 盘临时目录
    let out_dir = crate::llm::settings::clawdesk_dir().join("generated-images");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建输出目录失败: {}", e))?;

    // data 可能是 base64（OpenAI 格式）或图片 URL（智谱 CogView / SiliconFlow url 模式）
    use base64::Engine;
    let bytes: Vec<u8> = if data.starts_with("http://") || data.starts_with("https://") {
        // URL → 下载
        let resp = ureq::get(data)
            .timeout(std::time::Duration::from_secs(60))
            .call()
            .map_err(|e| format!("下载绘图 URL 失败: {}", e))?;
        let mut buf: Vec<u8> = Vec::new();
        use std::io::Read;
        resp.into_reader()
            .take(20 * 1024 * 1024) // 20MB 上限
            .read_to_end(&mut buf)
            .map_err(|e| format!("读取图片数据失败: {}", e))?;
        buf
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("绘图 API 返回 base64 解码失败: {}", e))?
    };

    // ★ 真实格式探测：按图片签名识别，杜绝扩展名/mime 与实际字节不符
    let (ext, mime) = detect_image_format(&bytes);
    let file_name = format!(
        "gen_{}_{}.{}",
        chrono::Local::now().format("%Y%m%d_%H%M%S"),
        short_hash(prompt),
        ext
    );
    let path: PathBuf = out_dir.join(&file_name);
    std::fs::write(&path, &bytes).map_err(|e| format!("保存图片失败: {}", e))?;
    let data_url = format!(
        "data:{};base64,{}",
        mime,
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );

    crate::llm::logging::debug(
        "generate_image",
        &format!("✅ 图片已保存 {} ({} 字节, {})", path.display(), bytes.len(), mime),
    );
    Ok((path.to_string_lossy().into_owned(), data_url, note.to_string()))
}

/// 按字节签名探测图片格式：返回 (扩展名, mime)。未知格式兜底 png。
fn detect_image_format(bytes: &[u8]) -> (&'static str, &'static str) {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ("png", "image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        ("jpg", "image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ("gif", "image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ("webp", "image/webp")
    } else if bytes.starts_with(b"BM") {
        ("bmp", "image/bmp")
    } else {
        ("png", "image/png")
    }
}

/// 生成程序化占位图：返回 (文件绝对路径, dataUrl)。
fn generate_placeholder(prompt: &str, width: u32, height: u32) -> Result<(String, String), String> {
    let img = render_placeholder(prompt, width, height);

    // 1) 编码 PNG → dataUrl
    let mut png_buf = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut png_buf);
        img.write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| format!("PNG 编码失败: {}", e))?;
    }
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_buf);
    let data_url = format!("data:image/png;base64,{}", b64);

    // 2) 保存文件
    let out_dir = std::env::temp_dir().join("clawdesk-generated");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建输出目录失败: {}", e))?;
    let file_name = format!(
        "gen_{}_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S"),
        short_hash(prompt)
    );
    let path: PathBuf = out_dir.join(&file_name);
    img.save(&path).map_err(|e| format!("保存图片失败: {}", e))?;

    Ok((path.to_string_lossy().into_owned(), data_url))
}

/// 程序化渲染：prompt 哈希驱动的配色渐变 + 同心圆图案。
fn render_placeholder(prompt: &str, width: u32, height: u32) -> RgbaImage {
    let seed = short_hash(prompt);
    // 注意：u64 乘法可能溢出（debug 模式 panic），使用 wrapping 运算
    let (h1, h2) = (
        (seed % 360) as f32,
        ((seed.wrapping_mul(7).wrapping_add(180)) % 360) as f32,
    );

    let mut img = RgbaImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let t = (x as f32 + y as f32) / (width as f32 + height as f32);
            // 对角渐变：h1 → h2
            let hue = h1 + (h2 - h1) * t;
            let (r, g, b) = hsl_to_rgb(hue, 0.55, 0.45 + 0.2 * t);
            img.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }

    // 同心圆图案：基于 seed 的 3 组半透明圆
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_r = (width.min(height) as f32) * 0.9;
    for band in 0..3u32 {
        let base_r = max_r * (0.25 + 0.22 * band as f32);
        let color = Rgba([255u8, 255u8, 255u8, 36u8]);
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                if (dist - base_r).abs() < 3.0 {
                    img.put_pixel(x, y, color);
                }
            }
        }
    }

    img
}

/// 简易 HSL → RGB（h: 0-360, s/l: 0-1），输出 0-255 通道。
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = ((h % 360.0 + 360.0) % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}

fn short_hash(input: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_conversion_is_valid() {
        let (r, g, b) = hsl_to_rgb(0.0, 0.0, 1.0);
        assert_eq!((r, g, b), (255, 255, 255));
        let (r, g, b) = hsl_to_rgb(0.0, 0.0, 0.0);
        assert_eq!((r, g, b), (0, 0, 0));
    }

    #[test]
    fn render_produces_correct_size() {
        let img = render_placeholder("一只蓝色的猫", 128, 64);
        assert_eq!(img.width(), 128);
        assert_eq!(img.height(), 64);
    }

    #[test]
    fn generate_returns_png_dataurl() {
        let (path, data_url, note) = generate_image("测试 prompt", 128, 128).unwrap();
        assert!(data_url.starts_with("data:image/png;base64,"));
        assert!(std::path::Path::new(&path).exists());
        // 未配置绘图 API（测试环境无全局路由）→ 降级占位图，note 说明降级
        assert!(note.contains("降级"), "{}", note);
        // 清理
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn placeholder_helper_returns_png() {
        let (path, data_url) = generate_placeholder("占位", 64, 64).unwrap();
        assert!(data_url.starts_with("data:image/png;base64,"));
        assert!(std::path::Path::new(&path).exists());
        let _ = std::fs::remove_file(&path);
    }
}
