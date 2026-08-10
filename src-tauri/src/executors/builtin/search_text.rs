//! `builtin:search_text` —— 全局文本检索工具。
//!
//! 设计说明：
//! - 递归遍历目录，搜索文件内容中的匹配文本；
//! - 防爆炸：最大文件 1MB、最大结果 500 条、最大深度 20 层；
//! - 敏感路径双保险拦截（复用 analyze_image::is_sensitive_path）；
//! - 非高危工具（只读）。

use std::path::Path;
use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const MAX_FILE_BYTES: u64 = 1024 * 1024; // 1MB
const MAX_RESULTS: usize = 500;
const MAX_DEPTH: u32 = 20;

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "search_text",
        "在目录下递归搜索文件内容中的匹配文本，返回匹配文件路径、行号、内容摘要",
        vec![
            ToolParamDef {
                name: "path".into(),
                param_type: "string".into(),
                description: "搜索根目录的绝对路径".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "pattern".into(),
                param_type: "string".into(),
                description: "要搜索的文本（子字符串匹配）".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "file_glob".into(),
                param_type: "string".into(),
                description: "可选的文件扩展名过滤，如 *.rs 或 rs".into(),
                required: false,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "case_sensitive".into(),
                param_type: "boolean".into(),
                description: "是否大小写敏感（默认 false 即不区分）".into(),
                required: false,
                enum_values: None,
                default: Some(json!(false)),
            },
        ],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
            let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or_default();
            let file_glob = args.get("file_glob").and_then(|v| v.as_str()).map(|s| s.to_string());
            let case_sensitive = args
                .get("case_sensitive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if path.is_empty() || pattern.is_empty() {
                return Ok(ToolResult::err("path 与 pattern 不能为空"));
            }
            if super::analyze_image::is_sensitive_path(path) {
                return Ok(ToolResult::err("禁止搜索系统敏感路径"));
            }

            match search_in_dir(path, pattern, case_sensitive, file_glob.as_deref()) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("搜索失败: {}", e))),
            }
        })
    });

    registry.register(def, handler)
}

/// 遍历目录，在文件中搜索 pattern 子字符串匹配。
fn search_in_dir(
    root: &str,
    pattern: &str,
    case_sensitive: bool,
    file_glob: Option<&str>,
) -> Result<serde_json::Value, String> {
    if pattern.is_empty() {
        return Err("pattern 不能为空".into());
    }
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Err(format!("路径不存在或不是目录: {}", root));
    }
    // 敏感路径双保险
    if super::analyze_image::is_sensitive_path(root) {
        return Err("禁止搜索系统敏感路径".into());
    }

    let pattern_lower = if !case_sensitive {
        pattern.to_lowercase()
    } else {
        String::new()
    };
    let glob_ext = file_glob.map(|g| {
        g.trim_start_matches("*.")
            .trim_start_matches('.')
            .to_lowercase()
    });

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut files_scanned: u32 = 0;

    walk_dir(
        root_path,
        0,
        pattern,
        pattern_lower.as_str(),
        case_sensitive,
        glob_ext.as_deref(),
        &mut results,
        &mut files_scanned,
    )?;

    Ok(json!({
        "path": root,
        "pattern": pattern,
        "caseSensitive": case_sensitive,
        "matchCount": results.len(),
        "filesScanned": files_scanned,
        "matches": results,
    }))
}

/// 递归遍历（手动实现，不额外引入 walkdir）。
fn walk_dir(
    dir: &Path,
    depth: u32,
    pattern: &str,
    pattern_lower: &str,
    case_sensitive: bool,
    glob_ext: Option<&str>,
    results: &mut Vec<serde_json::Value>,
    files_scanned: &mut u32,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    if results.len() >= MAX_RESULTS {
        return Ok(());
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()), // 无权限目录静默跳过
    };

    for entry in entries.flatten() {
        if results.len() >= MAX_RESULTS {
            return Ok(());
        }

        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // 跳过隐藏文件/目录
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            walk_dir(
                &path,
                depth + 1,
                pattern,
                pattern_lower,
                case_sensitive,
                glob_ext,
                results,
                files_scanned,
            )?;
        } else if path.is_file() {
            // glob 扩展名过滤
            if let Some(ref ext) = glob_ext {
                if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase())
                    .as_deref()
                    != Some(ext)
                {
                    continue;
                }
            }

            // 文件大小检查
            let meta = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }

            *files_scanned += 1;

            // 读取并在行级搜索
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let file_path = path.to_string_lossy().to_string();
            for (line_no, line) in content.lines().enumerate() {
                if results.len() >= MAX_RESULTS {
                    break;
                }
                let haystack = if case_sensitive {
                    line.to_string()
                } else {
                    line.to_lowercase()
                };
                let needle = if case_sensitive { pattern } else { pattern_lower };
                if haystack.contains(needle) {
                    results.push(json!({
                        "file": file_path,
                        "line": line_no + 1,
                        "content": truncate_line(line, 200),
                    }));
                }
            }
        }
    }
    Ok(())
}

/// 截断单行（char 边界安全）。
fn truncate_line(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let head: String = chars[..max_chars].iter().collect();
        format!("{}…", head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 唯一临时目录（按测试名区分，避免并行测试共享 process::id 目录互相清理）。
    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("clawdesk-st-{}-{}", std::process::id(), name))
    }

    /// 创建测试目录结构：
    /// tmp/
    ///   a.txt    "hello world\nfoo bar\nHELLO again"
    ///   b.rs     "fn main() {\n    println!(\"hello\");\n}"
    ///   sub/
    ///     c.txt  "nope"
    #[test]
    fn search_text_finds_matches() {
        let dir = temp_dir("finds");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "hello world\nfoo bar\nHELLO again").unwrap();
        std::fs::write(dir.join("b.rs"), "fn main() {\n    println!(\"hello\");\n}").unwrap();
        std::fs::write(dir.join("sub").join("c.txt"), "nope").unwrap();

        let out = search_in_dir(dir.to_str().unwrap(), "hello", false, None).unwrap();
        assert_eq!(out["matchCount"], 3); // a.txt line1 + a.txt line3 + b.rs line2

        // 大小写敏感
        let out2 = search_in_dir(dir.to_str().unwrap(), "HELLO", true, None).unwrap();
        assert_eq!(out2["matchCount"], 1); // a.txt line3 only

        // glob 过滤：仅 .rs
        let out3 = search_in_dir(dir.to_str().unwrap(), "hello", false, Some("rs")).unwrap();
        assert_eq!(out3["matchCount"], 1); // b.rs only

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_text_empty_params_rejected() {
        let dir = temp_dir("empty");
        let _ = std::fs::remove_dir_all(&dir);
        let out1 = search_in_dir("", "hello", false, None).unwrap_err();
        assert!(out1.contains("不存在"), "{}", out1);
        std::fs::create_dir_all(&dir).unwrap();
        let out2 = search_in_dir(dir.to_str().unwrap(), "", false, None).unwrap_err();
        assert!(out2.contains("pattern"), "{}", out2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_text_sensitive_path_blocked() {
        let out = search_in_dir("C:\\Windows\\System32", "test", false, None).unwrap_err();
        assert!(out.contains("敏感路径"), "{}", out);
    }
}
