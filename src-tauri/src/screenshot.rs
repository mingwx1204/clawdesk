//! 截屏命令：截取主屏幕返回 PNG base64。
//! 区域裁剪在前端完成（截图后以全屏遮罩让用户框选，再 canvas 裁剪）。

use crate::error::{AppError, AppResult};

#[tauri::command]
pub fn capture_screen() -> AppResult<String> {
    use base64::Engine;
    use std::io::Cursor;

    let screens = screenshots::Screen::all().map_err(|e| AppError::Screenshot(e.to_string()))?;
    let screen = screens
        .first()
        .ok_or_else(|| AppError::Screenshot("未检测到显示器".into()))?;
    let image = screen.capture().map_err(|e| AppError::Screenshot(e.to_string()))?;

    // 编码为 PNG（screenshots 0.8 返回 image crate 的 RgbaImage）
    let mut buf: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| AppError::Screenshot(e.to_string()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf))
}
