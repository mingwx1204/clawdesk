//! builtin 源文件操作工具集：`file_read` / `file_write` / `list_dir`。
//!
//! 设计说明：
//! - `file_write` 写前自动备份原文件到 `%APPDATA%/clawdesk/snapshots/`（文档 §7.1 快照备份）；
//! - 文件读写限定安全：拒绝系统敏感路径（HighRiskGuard 统一拦截 + 此处双保险）；
//! - 文本读取限制 1MB，防大文件撑爆上下文。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const MAX_READ_BYTES: u64 = 1024 * 1024; // 1MB

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    register_read(registry)?;
    register_write(registry)?;
    register_list_dir(registry)?;
    Ok(())
}

fn path_param(name: &str, desc: &str) -> ToolParamDef {
    ToolParamDef {
        name: name.into(),
        param_type: "string".into(),
        description: desc.into(),
        required: true,
        enum_values: None,
        default: None,
    }
}

fn register_read(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "file_read",
        "读取本地文本文件内容（限制 1MB），返回内容与行数",
        vec![path_param("path", "文件绝对路径")],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
            if path.is_empty() {
                return Ok(ToolResult::err("path 不能为空"));
            }
            if super::analyze_image::is_sensitive_path(path) {
                return Ok(ToolResult::err("禁止读取系统敏感路径"));
            }
            match read_file(path) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("读取失败: {}", e))),
            }
        })
    });
    registry.register(def, handler)
}

fn register_write(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "file_write",
        "写入文本到文件（写前自动备份原文件到快照目录）",
        vec![
            path_param("path", "文件绝对路径"),
            ToolParamDef {
                name: "content".into(),
                param_type: "string".into(),
                description: "要写入的文本内容".into(),
                required: true,
                enum_values: None,
                default: None,
            },
        ],
    )?
    .high_risk(); // 写文件属高危：需用户确认（StepConfirm / 前端）
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or_default();
            if path.is_empty() || content.is_empty() {
                return Ok(ToolResult::err("path 与 content 不能为空"));
            }
            if super::analyze_image::is_sensitive_path(path) {
                return Ok(ToolResult::err("禁止写入系统敏感路径"));
            }
            match write_file(path, content) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("写入失败: {}", e))),
            }
        })
    });
    registry.register(def, handler)
}

fn register_list_dir(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "list_dir",
        "列出目录条目，format 参数可指定输出格式（table/json/plain）",
        vec![
            path_param("path", "目录绝对路径"),
            ToolParamDef {
                name: "format".into(),
                param_type: "string".into(),
                description: "输出格式：table（默认，对齐表格，含类型/大小/修改时间/名称）、json（原始数据）、plain（每行一个名字）".into(),
                required: false,
                enum_values: Some(vec!["table".into(), "json".into(), "plain".into()]),
                default: Some("table".into()),
            },
        ],
    )?;
    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
            if path.is_empty() {
                return Ok(ToolResult::err("path 不能为空"));
            }
            let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("table");
            match list_dir(path, format) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("遍历失败: {}", e))),
            }
        })
    });
    registry.register(def, handler)
}

fn read_file(path: &str) -> Result<serde_json::Value, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("无法访问: {}", e))?;
    if !meta.is_file() {
        return Err("不是文件".into());
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(format!("文件过大（{}KB，限制 1MB），请用终端工具分段读取", meta.len() / 1024));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {}", e))?;
    // ★ ZIP 压缩包支持：列出内容 + 自动读取第一个文本/PDF 文件。
    //   超过 2MB 明确提示不支持（与微信收包一致，防解压炸弹）。
    if bytes.len() <= 2 * 1024 * 1024 && bytes.starts_with(b"PK\x03\x04") {
        return read_zip_archive(path, &bytes);
    }
    if bytes.starts_with(b"PK") && bytes.len() > 2 * 1024 * 1024 {
        return Ok(json!({
            "path": path,
            "bytes": bytes.len(),
            "lines": 0,
            "content": "[压缩包超过 2MB，不支持自动解压查看。请拆分后重发或直接发送内部文件]",
        }));
    }
    // ★ PDF 支持：按文件头识别 PDF（%PDF-），提取文本而非返回乱码二进制。
    //   纯 Rust 轻量解析：只提取 Tj/TJ 文本流，足够 AI 理解内容；加密/复杂版式
    //   会提取失败并给出明确提示（不 panic）。
    if bytes.starts_with(b"%PDF") {
        return extract_pdf_text(path, &bytes);
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| "二进制文件（非 UTF-8 文本），无法直接读取，请用其他工具处理".to_string())?;
    Ok(json!({
        "path": path,
        "bytes": meta.len(),
        "lines": content.lines().count(),
        "content": content,
    }))
}

/// 轻量 PDF 文本提取：解析对象流中的 Tj / TJ 文本运算符。
/// 覆盖绝大多数正常导出的 PDF；扫描型 PDF（纯图片）返回提示让 AI 转图片识别。
fn extract_pdf_text(path: &str, bytes: &[u8]) -> Result<serde_json::Value, String> {
    let mut text = String::new();
    let mut i = 0usize;
    let mut captured_chars = 0usize;
    const MAX_CAPTURE: usize = 300_000; // 防超大 PDF 撑爆上下文（约 30 万字符）
    while i + 8 < bytes.len() && captured_chars < MAX_CAPTURE {
        // 文本流标识：BT ... ET 中的 ( 开头的字面量，或 <...> 十六进制串
        if bytes[i] == b'(' {
            // 扫描到右括号（支持转义）
            let mut j = i + 1;
            let mut depth = 1usize;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'\\' => j += 2, // 跳过转义
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => j += 1,
                }
                if depth == 0 {
                    let raw = &bytes[i + 1..j];
                    let s = raw
                        .iter()
                        .map(|&b| match b {
                            b'\\' => '\\',
                            b'(' => '(',
                            b')' => ')',
                            0x0A => ' ',
                            0x0D => ' ',
                            b if b < 0x20 => ' ',
                            b => b as char,
                        })
                        .collect::<String>();
                    text.push_str(&s);
                    text.push(' ');
                    captured_chars += s.len();
                    break;
                }
            }
            i = j.saturating_add(1);
        } else if bytes[i] == b'<' {
            // 十六进制字符串 <48656C6C6F> 只提取可打印 ASCII
            let mut j = i + 1;
            let mut hex = Vec::new();
            while j < bytes.len()
                && bytes[j] != b'>'
                && bytes[j] != b' '
                && bytes[j] != b'\n'
                && bytes[j] != b'\r'
            {
                hex.push(bytes[j]);
                j += 1;
            }
            if bytes.get(j) == Some(&b'>') && !hex.is_empty() && hex.len() % 2 == 0 {
                let mut s = String::new();
                for pair in hex.chunks(2) {
                    if let (Some(&hi), Some(&lo)) = (pair.first(), pair.get(1)) {
                        let b = (hex_val(hi) << 4) | hex_val(lo);
                        if b >= 0x20 && b < 0x7F {
                            s.push(b as char);
                        }
                    }
                }
                if !s.trim().is_empty() {
                    text.push_str(&s);
                    text.push(' ');
                    captured_chars += s.len();
                }
                i = j.saturating_add(1);
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let content = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if content.trim().is_empty() {
        Ok(json!({
            "path": path,
            "bytes": bytes.len(),
            "lines": 0,
            "content": "[PDF 未提取到文本：可能是扫描件/图片型 PDF 或已加密。可改用 analyze_image 工具对 PDF 页面图片做 OCR 识别]",
        }))
    } else {
        Ok(json!({
            "path": path,
            "bytes": bytes.len(),
            "lines": content.len() / 50 + 1,
            "content": content,
        }))
    }
}

/// 读取 zip 压缩包：列出文件清单 + 尝试读取第一个文本/PDF 内容。
/// 让 AI 不依赖外部工具就能"看见"压缩包内部。
fn read_zip_archive(path: &str, bytes: &[u8]) -> Result<serde_json::Value, String> {
    use std::io::Cursor;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("读取压缩包失败: {e}"))?;

    let mut file_list: Vec<String> = Vec::new();
    let mut first_text: Option<String> = None;
    let mut first_name = String::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取压缩包条目失败: {e}"))?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }
        file_list.push(format!("- {} ({}KB)", name, entry.size() / 1024));
        // 第一个文本类文件：读出内容给 AI（最多 50KB）
        if first_text.is_none() && entry.size() <= 50 * 1024 {
            let lower = name.to_lowercase();
            let is_text = lower.ends_with(".txt")
                || lower.ends_with(".md")
                || lower.ends_with(".json")
                || lower.ends_with(".log")
                || lower.ends_with(".csv")
                || lower.ends_with(".ini")
                || lower.ends_with(".cfg")
                || lower.ends_with(".yml")
                || lower.ends_with(".yaml")
                || lower.ends_with(".pdf")
                || lower.ends_with(".doc")
                || lower.ends_with(".docx")
                || lower.ends_with(".js")
                || lower.ends_with(".ts")
                || lower.ends_with(".py")
                || lower.ends_with(".html");
            if is_text {
                let mut buf = Vec::with_capacity(entry.size() as usize);
                std::io::copy(&mut entry, &mut buf)
                    .map_err(|e| format!("读取压缩包内文件失败: {e}"))?;
                let content = if lower.ends_with(".pdf") && buf.starts_with(b"%PDF") {
                    // PDF 走轻量提取
                    extract_pdf_text(&name, &buf)
                        .ok()
                        .and_then(|v| v.get("content").and_then(|c| c.as_str()).map(String::from))
                        .unwrap_or_else(|| "[PDF 文本提取失败]".to_string())
                } else {
                    String::from_utf8_lossy(&buf).to_string()
                };
                first_text = Some(content);
                first_name = name;
            }
        }
    }

    let mut content = format!(
        "[ZIP 压缩包: {} 个文件，已自动解压读取]\n文件清单:\n{}",
        file_list.len(),
        file_list.join("\n")
    );
    if let Some(t) = first_text {
        content.push_str(&format!(
            "\n\n--- 已读取第一个文本文件 `{}` 的内容（节选 50KB）---\n{}",
            first_name,
            t.chars().take(50_000).collect::<String>()
        ));
    } else {
        content.push_str("\n\n[压缩包内无文本类文件。如需查看具体文件，可自行解压后用 file_read 逐个读取]");
    }
    Ok(json!({
        "path": path,
        "bytes": bytes.len(),
        "lines": content.len() / 50 + 1,
        "content": content,
    }))
}

/// 十六进制字符转数值（PDF hex string 用）
fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// 写文件：先备份原文件到快照目录，再写入。
fn write_file(path: &str, content: &str) -> Result<serde_json::Value, String> {
    // 1) 备份原文件（若存在）
    let snapshot_dir = super::snapshot::snapshot_dir();
    std::fs::create_dir_all(&snapshot_dir).map_err(|e| format!("创建快照目录失败: {}", e))?;
    let file_name = PathBuf::from(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let backup = snapshot_dir.join(format!(
        "{}_{}.bak",
        chrono::Local::now().format("%Y%m%d_%H%M%S%3f"),
        file_name
    ));
    if std::path::Path::new(path).exists() {
        std::fs::copy(path, &backup).map_err(|e| format!("备份原文件失败: {}", e))?;
    }

    // 2) 写入
    std::fs::write(path, content).map_err(|e| format!("写入失败: {}", e))?;

    // 3) 写入快照索引（供 snapshot_list / snapshot_restore / snapshot_diff 使用）
    let mut snapshot_cleaned: Vec<String> = Vec::new();
    if backup.exists() {
        let size = std::fs::metadata(&backup).map(|m| m.len()).unwrap_or(0);
        let _ = super::snapshot::record_snapshot(path, backup.to_string_lossy().as_ref(), size);
        // 容量限制：超阈值清理最旧快照，清理记录回传（模型避免读取失效路径）
        let removed = super::snapshot::enforce_capacity(super::snapshot::DEFAULT_CAPACITY_BYTES);
        snapshot_cleaned = removed
            .iter()
            .map(|e| format!("{} (原文件: {})", e.snapshot, e.original))
            .collect();
    }

    Ok(json!({
        "path": path,
        "writtenBytes": content.len(),
        "backup": if backup.exists() { json!(backup.to_string_lossy().to_string()) } else { serde_json::Value::Null },
        "snapshotCleaned": snapshot_cleaned,
    }))
}

/// 目录条目（含修改时间）。
struct DirEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: String,
}

/// 列出目录，支持指定输出格式：table（对齐表格，默认）/ json / plain。
fn list_dir(path: &str, format: &str) -> Result<serde_json::Value, String> {
    let entries: Vec<DirEntry> = std::fs::read_dir(path)
        .map_err(|e| format!("无法读取目录: {}", e))?
        .flatten()
        .take(200)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.path().is_dir();
            let meta = entry.metadata().ok();
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Local> = t.into();
                    dt.format("%Y-%m-%d %H:%M").to_string()
                })
                .unwrap_or_else(|| "-".into());
            DirEntry { name, is_dir, size, modified }
        })
        .collect();
    let dir_count = entries.iter().filter(|e| e.is_dir).count();
    let file_count = entries.len() - dir_count;
    let count = entries.len();

    // json 格式：返回原始条目数组
    if format == "json" {
        let arr: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "type": if e.is_dir { "dir" } else { "file" },
                    "size": e.size,
                    "modified": e.modified,
                })
            })
            .collect();
        return Ok(json!({
            "path": path, "format": "json",
            "entries": arr, "count": count,
            "dirCount": dir_count, "fileCount": file_count,
        }));
    }

    // plain：每行一个名字（目录带 / 后缀）
    let text = if format == "plain" {
        entries
            .iter()
            .map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" }))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        // table：目录在前、文件在后，右对齐大小，对齐修改时间
        let mut lines = vec![
            format!("{}  （{} 项：{} 目录 / {} 文件）", path, count, dir_count, file_count),
            format!("{:<4} {:>9}  {:<16} {}", "TYPE", "SIZE", "MODIFIED", "NAME"),
        ];
        for e in entries.iter().filter(|e| e.is_dir) {
            lines.push(format!("{:<4} {:>9}  {:<16} {}/", "dir", "-", e.modified, e.name));
        }
        for e in entries.iter().filter(|e| !e.is_dir) {
            lines.push(format!("{:<4} {:>9}  {:<16} {}", "file", format_size(e.size), e.modified, e.name));
        }
        lines.join("\n")
    };

    Ok(json!({
        "path": path,
        "format": if format == "plain" { "plain" } else { "table" },
        "text": text, "count": count,
        "dirCount": dir_count, "fileCount": file_count,
    }))
}

/// 人性化文件大小：B / KB / MB / GB。
fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

// 快照目录统一由 snapshot 模块提供（snapshot_dir / record_snapshot / enforce_capacity）

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        // write_file 会写入快照索引（项目 6）：与快照测试串行 + 独立覆盖层，
        // 避免并行运行时互相污染（快照测试断言失败 → 锁中毒连锁失败）。
        let _guard = super::super::snapshot::test_lock();
        let snap_override = std::env::temp_dir()
            .join(format!("clawdesk-fs-snap-{}", std::process::id()));
        super::super::snapshot::set_root_override(Some(snap_override.clone()));
        let dir = std::env::temp_dir().join(format!("clawdesk-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("test.txt");

        write_file(f.to_str().unwrap(), "hello").unwrap();
        let out = read_file(f.to_str().unwrap()).unwrap();
        assert_eq!(out["content"], "hello");

        // 再次写入 → 产生备份
        write_file(f.to_str().unwrap(), "world").unwrap();
        let snap = super::super::snapshot::snapshot_dir();
        let backups = std::fs::read_dir(&snap).unwrap().count();
        assert!(backups >= 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_missing_file_errors() {
        assert!(read_file("D:/no-such-file-xyz.txt").is_err());
    }

    #[test]
    fn list_dir_lists_entries() {
        let dir = std::env::temp_dir().join(format!("clawdesk-ls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        // table（默认格式）：返回排好版的 text，目录在前、含大小与修改时间
        let out = list_dir(dir.to_str().unwrap(), "table").unwrap();
        assert_eq!(out["format"], "table");
        assert!(out["count"].as_u64().unwrap() >= 2);
        assert_eq!(out["dirCount"].as_u64().unwrap(), 1);
        let text = out["text"].as_str().unwrap();
        assert!(text.contains("sub/"));
        assert!(text.contains("a.txt"));
        assert!(text.contains("1 B")); // a.txt 内容 1 字节
        // json：返回原始条目数组
        let j = list_dir(dir.to_str().unwrap(), "json").unwrap();
        assert_eq!(j["format"], "json");
        assert!(j["entries"].as_array().unwrap().len() >= 2);
        // plain：每行一个名字
        let p = list_dir(dir.to_str().unwrap(), "plain").unwrap();
        assert!(p["text"].as_str().unwrap().contains("a.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
