//! 项目进度统计：工作目录文件规模 + 最近修改文件列表。
//! walkdir 单次遍历，按修改时间取 Top 20。

use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Debug, Serialize)]
pub struct RecentFile {
    pub path: String,
    pub modified: u64,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct ProjectStats {
    pub total_files: u64,
    pub total_dirs: u64,
    pub total_size: u64,
    pub recent: Vec<RecentFile>,
}

const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", "dist", "build", "__pycache__"];

#[tauri::command]
pub fn project_stats(path: String) -> AppResult<ProjectStats> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(AppError::PathNotFound(path));
    }
    let mut total_files = 0u64;
    let mut total_dirs = 0u64;
    let mut total_size = 0u64;
    let mut recent: Vec<RecentFile> = Vec::new();

    let walker = WalkDir::new(&root).into_iter();
    for entry in walker
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(name.starts_with('.') || (e.file_type().is_dir() && SKIP_DIRS.contains(&name.as_ref())))
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_dir() {
            total_dirs += 1;
            continue;
        }
        total_files += 1;
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len();
        total_size += size;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        recent.push(RecentFile {
            path: entry.path().to_string_lossy().to_string(),
            modified,
            size,
        });
    }
    // 最近修改 Top 20
    recent.sort_by(|a, b| b.modified.cmp(&a.modified));
    recent.truncate(20);

    Ok(ProjectStats {
        total_files,
        total_dirs,
        total_size,
        recent,
    })
}
