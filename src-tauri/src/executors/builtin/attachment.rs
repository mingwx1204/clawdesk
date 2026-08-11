//! `builtin:attachment_save` —— 保存用户拖入/选择的附件文件到本地附件目录。
//!
//! 设计说明（拓展机制，不改动 core / agent）：
//! - 供前端"拖入任意文件"能力使用：前端读取文件为 base64，经本工具写入本地磁盘；
//! - 保存目录：`<ClawDesk 数据目录>\attachments`（数据目录优先 D:\ClawDeskData，
//!   避免被系统临时清理误删）；
//! - 文件名净化：仅取 `file_name()` 组件，防路径穿越；
//! - 大小限制 20MB（base64 解码后），防大文件撑爆磁盘；
//! - agent 侧无需改动：路径经 prompt 注入后，LLM 可用已有的 `file_read` 工具读取内容。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// 附件保存目录名（位于 ClawDesk 数据目录下）。
pub const ATTACH_DIR_NAME: &str = "attachments";
/// 单文件大小上限（字节）。
const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// 附件保存目录的绝对路径（不存在则创建）。
///
/// ★ 持久化路径：`<ClawDesk 数据目录>\attachments`（数据目录优先 D:\ClawDeskData，
/// 不再使用系统临时目录 —— 临时目录会被磁盘清理误删历史附件/导出文件）。
/// 该目录同时承载：上传附件、导出对话（export_*.md）、微信接收媒体（inbound/）等。
pub fn attach_dir() -> Result<std::path::PathBuf, String> {
    let dir = crate::llm::settings::clawdesk_dir().join(ATTACH_DIR_NAME);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建附件目录失败: {e}"))?;
    Ok(dir)
}

/// 最近一次旧文件清理时间（毫秒，全局限频：1 小时最多扫描一次）
static LAST_CLEANUP_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 清理目录中超过 max_age_days 天未修改的旧文件（只删直接子文件，不递归、不删目录）。
/// 低频执行：min_interval_hours 小时内最多真正扫描一次（多线程并发写盘时仅一个执行）。
pub fn cleanup_old_files(dir: &std::path::Path, max_age_days: u64, min_interval_hours: u64) {
    use std::sync::atomic::Ordering as AtOrd;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_CLEANUP_MS.load(AtOrd::Relaxed);
    if now.saturating_sub(last) < min_interval_hours * 3600_000 {
        return;
    }
    if LAST_CLEANUP_MS
        .compare_exchange(last, now, AtOrd::Relaxed, AtOrd::Relaxed)
        .is_err()
    {
        return;
    }
    let cutoff = now.saturating_sub(max_age_days * 24 * 3600_000);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            continue;
        }
        let Ok(modified) = meta.modified() else { continue };
        let mtime = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if mtime < cutoff {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "attachment_save",
        "保存附件文件到本地附件目录（前端拖入/上传文件使用），返回文件绝对路径",
        vec![
            ToolParamDef {
                name: "name".into(),
                param_type: "string".into(),
                description: "原始文件名（自动净化，仅保留文件名部分）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "data".into(),
                param_type: "string".into(),
                description: "文件内容（base64 编码，上限 20MB）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
        ],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let data = args.get("data").and_then(|v| v.as_str()).unwrap_or_default();
            if name.is_empty() || data.is_empty() {
                return Ok(ToolResult::err("name 与 data 不能为空"));
            }
            match save_attachment(name, data) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(e)),
            }
        })
    });

    registry.register(def, handler)
}

fn save_attachment(name: &str, data: &str) -> Result<serde_json::Value, String> {
    use base64::Engine as _;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("base64 解码失败: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!(
            "文件过大（{}MB，上限 20MB）",
            bytes.len() / 1024 / 1024
        ));
    }

    // 文件名净化：仅保留文件名部分（`file_name()` 天然剔除路径前缀与 `..`）
    let safe = std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("attachment.bin")
        .replace(['/', '\\', '\0'], "_");

    let dir = attach_dir()?;
    let path = dir.join(&safe);
    // 同名冲突加时间戳后缀，避免覆盖
    let final_path = if path.exists() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        dir.join(if ext.is_empty() {
            format!("{stem}_{ts}")
        } else {
            format!("{stem}_{ts}.{ext}")
        })
    } else {
        path
    };

    std::fs::write(&final_path, &bytes).map_err(|e| format!("写入附件失败: {e}"))?;
    Ok(json!({
        "path": final_path.to_string_lossy(),
        "name": safe,
        "size": bytes.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn save_and_return_path() {
        let v = save_attachment("note.txt", &b64("hello clawdesk")).unwrap();
        assert_eq!(v["name"], "note.txt");
        let p = std::path::Path::new(v["path"].as_str().unwrap());
        assert!(p.exists());
        std::fs::remove_file(p).ok();
    }

    #[test]
    fn strip_path_traversal() {
        let v = save_attachment("../../evil.txt", &b64("x")).unwrap();
        assert_eq!(v["name"], "evil.txt");
        assert!(!v["path"].as_str().unwrap().contains(".."));
        std::fs::remove_file(v["path"].as_str().unwrap()).ok();
    }

    #[test]
    fn reject_oversize() {
        let big = vec![0u8; (MAX_BYTES + 1) as usize];
        let data = base64::engine::general_purpose::STANDARD.encode(&big);
        assert!(save_attachment("big.bin", &data).is_err());
    }
}
