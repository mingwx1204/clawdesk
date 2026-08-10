//! 文件系统命令：目录树读取（性能关键路径）、文件预览、文件操作。
//!
//! 性能设计：
//! - 目录树用 walkdir 单次遍历，一次 IPC 返回整棵树（避免逐节点 IPC 往返），
//!   10000 文件场景下 Rust 侧遍历通常在 100ms 内完成；
//! - 默认跳过隐藏目录与常见巨型目录（node_modules / .git / target）；
//! - 文本预览分片读取，绝不把整个大文件读进内存。

use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize)]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// 仅目录有值
    pub children: Vec<FileNode>,
    /// 文件大小（字节）
    pub size: u64,
    /// 扩展名（小写，无点）
    pub ext: String,
}

/// 默认跳过的目录名：这些目录巨大且对 Agent 工作区浏览无意义
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", "dist", "build", "__pycache__"];

fn build_tree(dir: &Path, depth: u32, max_depth: u32, budget: &mut usize) -> Vec<FileNode> {
    let mut nodes: Vec<FileNode> = Vec::new();
    if depth > max_depth || *budget == 0 {
        return nodes;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return nodes,
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || (entry.path().is_dir() && SKIP_DIRS.contains(&name.as_str())) {
            continue;
        }
        *budget -= 1;
        let path = entry.path();
        let is_dir = path.is_dir();
        let (size, ext) = if is_dir {
            (0, String::new())
        } else {
            let meta = entry.metadata().ok();
            (
                meta.as_ref().map(|m| m.len()).unwrap_or(0),
                path.extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
            )
        };
        let children = if is_dir {
            build_tree(&path, depth + 1, max_depth, budget)
        } else {
            Vec::new()
        };
        nodes.push(FileNode {
            name,
            path: path.to_string_lossy().to_string(),
            is_dir,
            children,
            size,
            ext,
        });
    }
    // 目录在前，文件在后，各自按名称排序
    nodes.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    nodes
}

/// 读取目录树。max_nodes 防止病态大目录拖垮渲染（默认 20000）。
#[tauri::command]
pub fn read_dir_tree(path: String, max_depth: Option<u32>, max_nodes: Option<usize>) -> AppResult<Vec<FileNode>> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(AppError::PathNotFound(path));
    }
    let mut budget = max_nodes.unwrap_or(20_000);
    Ok(build_tree(&root, 0, max_depth.unwrap_or(12), &mut budget))
}

/// 统计目录下文件数量（用于显示"已截断"提示）
#[tauri::command]
pub fn count_dir_files(path: String) -> AppResult<u64> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(AppError::PathNotFound(path));
    }
    let mut count = 0u64;
    for e in WalkDir::new(&root).into_iter().filter_map(|e| e.ok()) {
        if e.file_type().is_file() {
            count += 1;
        }
    }
    Ok(count)
}

const MAX_TEXT_PREVIEW: u64 = 512 * 1024; // 512KB 分片上限

/// 文本文件预览：最多读前 512KB，UTF-8 失败时返回可读提示
#[tauri::command]
pub fn read_file_text(path: String) -> AppResult<String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(AppError::PathNotFound(path));
    }
    let mut f = fs::File::open(&p)?;
    let mut buf = vec![0u8; MAX_TEXT_PREVIEW as usize];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    match String::from_utf8(buf) {
        Ok(mut s) => {
            if n as u64 == MAX_TEXT_PREVIEW {
                s.push_str("\n\n…(文件过大，仅显示前 512KB)");
            }
            Ok(s)
        }
        Err(_) => Err(AppError::Other("该文件不是有效的 UTF-8 文本，无法预览".into())),
    }
}

/// 图片/二进制文件预览：返回 base64，限制 20MB
#[tauri::command]
pub fn read_file_base64(path: String) -> AppResult<String> {
    use base64::Engine;
    let p = PathBuf::from(&path);
    let meta = p.metadata().map_err(|_| AppError::PathNotFound(path.clone()))?;
    if meta.len() > 20 * 1024 * 1024 {
        return Err(AppError::Other("文件超过 20MB，不支持内置预览".into()));
    }
    let bytes = fs::read(&p)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// 写入文本文件（由 AI 工具调用触发）
#[tauri::command]
pub fn write_file_text(path: String, content: String) -> AppResult<()> {
    let p = PathBuf::from(&path);
    // 确保父目录存在
    if let Some(parent) = p.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(&p, &content)?;
    Ok(())
}

#[tauri::command]
pub fn rename_path(old_path: String, new_name: String) -> AppResult<()> {
    let old = PathBuf::from(&old_path);
    if !old.exists() {
        return Err(AppError::PathNotFound(old_path));
    }
    let new = old.with_file_name(&new_name);
    fs::rename(&old, &new)?;
    Ok(())
}

#[tauri::command]
pub fn delete_path(path: String) -> AppResult<()> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(AppError::PathNotFound(path));
    }
    if p.is_dir() {
        fs::remove_dir_all(&p)?;
    } else {
        fs::remove_file(&p)?;
    }
    Ok(())
}
