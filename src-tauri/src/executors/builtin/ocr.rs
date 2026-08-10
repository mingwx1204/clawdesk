//! `builtin:ocr` —— 光学字符识别执行器（阶段 3）。
//!
//! 设计说明：
//! - 接收图片 base64（前端文件选择 → FileReader 转 base64 后传入），
//!   解码写入临时文件，调用系统 Tesseract CLI 识别；
//! - 若系统未安装 Tesseract 或识别失败，返回明确的 error 态（可降级提示）；
//! - 真实 OCR 引擎（Windows OCR / MCP OCR 服务）可在后续阶段替换实现，
//!   本工具的参数契约（image_base64 / lang）保持不变。
//!
//! Tesseract 依赖：https://github.com/tesseract-ocr/tesseract
//! Windows 安装后需确保 `tesseract.exe` 在 PATH 中。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// uiPayload 契约：仅前端渲染通道消费（DEV_SPEC.md §8）。
const UI_PAYLOAD: &str = r#"{"displayHint":{"icon":"🔍","tone":"info","note":"需要系统安装 Tesseract OCR 引擎"}}"#;

/// 支持的识别语言（ISO 639-1 代码，需 tesseract 语言包支持）。
const DEFAULT_LANG: &str = "eng";

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "ocr",
        "识别图片中的文字（base64 输入，需系统 Tesseract 引擎）",
        vec![
            ToolParamDef {
                name: "image_base64".into(),
                param_type: "string".into(),
                description: "图片文件的 base64 编码（不含 data: 前缀）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "lang".into(),
                param_type: "string".into(),
                description: "识别语言（默认 eng）".into(),
                required: false,
                enum_values: None,
                default: Some(json!(DEFAULT_LANG)),
            },
        ],
    )?
    .with_ui_payload(serde_json::from_str(UI_PAYLOAD).unwrap());

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let b64 = args
                .get("image_base64")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if b64.is_empty() {
                return Ok(ToolResult::err("image_base64 不能为空"));
            }

            let lang = args
                .get("lang")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_LANG);

            match run_ocr(b64, lang) {
                Ok(text) => Ok(ToolResult::ok(json!({
                    "lang": lang,
                    "text": text,
                    "chars": text.chars().count(),
                }))),
                Err(msg) => Ok(ToolResult::err(format!("OCR 失败: {}", msg))),
            }
        })
    });

    registry.register(def, handler)
}

/// 执行 OCR：base64 → 临时文件 → tesseract CLI → 文本。
fn run_ocr(image_base64: &str, lang: &str) -> Result<String, String> {
    use base64::Engine;

    // 0) 防御性空检查（handler 层已检查，此处保证函数自身健壮）
    if image_base64.trim().is_empty() {
        return Err("image_base64 不能为空".into());
    }

    // 1) 解码 base64
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    // 2) 写临时文件
    let tmp_dir = std::env::temp_dir().join("clawdesk-ocr");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("创建临时目录失败: {}", e))?;
    let img_path: PathBuf = tmp_dir.join(format!(
        "ocr_{}.png",
        chrono::Local::now().format("%Y%m%d_%H%M%S%3f")
    ));
    std::fs::write(&img_path, &bytes).map_err(|e| format!("写入临时文件失败: {}", e))?;

    // 3) 调 tesseract CLI
    let result = run_tesseract(&img_path, lang);

    // 清理临时文件（无论成败）
    let _ = std::fs::remove_file(&img_path);

    result
}

/// 调用 tesseract CLI：`tesseract <image> stdout -l <lang>`。
///
/// 健壮性设计：
/// - 最多重试 `MAX_ATTEMPTS`（3）次，失败间指数退避（150ms → 300ms）；
/// - 外部进程 stderr 截断（默认 500 字符），避免异常输出刷屏；
/// - stdout（识别文本）完整保留，但超过 1 MiB 视为异常拒绝。
fn run_tesseract(image_path: &std::path::Path, lang: &str) -> Result<String, String> {
    const MAX_ATTEMPTS: u32 = 3;
    const MAX_STDERR: usize = 500;
    const MAX_STDOUT: usize = 1024 * 1024; // 1 MiB

    let mut last_err = String::from("未知错误");
    for attempt in 1..=MAX_ATTEMPTS {
        match invoke_tesseract_once(image_path, lang, MAX_STDERR, MAX_STDOUT) {
            Ok(text) => return Ok(text),
            Err(e) => {
                last_err = e;
                if attempt < MAX_ATTEMPTS {
                    // 指数退避：150ms → 300ms
                    std::thread::sleep(std::time::Duration::from_millis(150 * attempt as u64));
                }
            }
        }
    }
    Err(format!(
        "tesseract 在 {} 次尝试后仍失败: {}",
        MAX_ATTEMPTS, last_err
    ))
}

/// 单次 tesseract 调用。
fn invoke_tesseract_once(
    image_path: &std::path::Path,
    lang: &str,
    max_stderr: usize,
    max_stdout: usize,
) -> Result<String, String> {
    let mut cmd = std::process::Command::new("tesseract");
    super::terminal::hide_console(&mut cmd);
    let output = cmd
        .arg(image_path.as_os_str())
        .arg("stdout")
        .arg("-l")
        .arg(lang)
        .output()
        .map_err(|e| {
            format!(
                "无法启动 tesseract（{}）。请安装 Tesseract OCR 并确保 tesseract.exe 在 PATH 中",
                e
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tesseract 退出码 {}: {}",
            output.status,
            truncate(&stderr, max_stderr)
        ));
    }

    if output.stdout.len() > max_stdout {
        return Err(format!("tesseract 输出超过 {} 字节，疑似异常", max_stdout));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err("未识别到任何文字（图片可能为空或质量过低）".into());
    }
    Ok(text)
}

/// 字符串截断：超长时保留头部并标注丢弃长度。
/// ★ 按字符截取（不按字节），避免中文等多字节字符被切到中间 panic。
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{}…(+{}chars)", head, s.chars().count().saturating_sub(max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_base64_is_rejected() {
        // 通过 handler 直接调用验证参数校验
        let err = run_ocr("", "eng").unwrap_err();
        assert!(err.contains("base64"));
    }

    #[test]
    fn invalid_base64_is_rejected() {
        let err = run_ocr("!!!not-base64!!!", "eng").unwrap_err();
        assert!(err.contains("base64 解码失败"));
    }
}
