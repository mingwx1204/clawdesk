//! 工作目录监听：notify 递归监听，事件去抖后推送 "workspace-changed"，
//! 前端收到事件后增量刷新文件树。

use crate::error::{AppError, AppResult};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub struct WatcherState(pub Mutex<Option<RecommendedWatcher>>);

impl Default for WatcherState {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

/// 监听目录（同时只保留一个监听，重复调用会先停掉旧的）
#[tauri::command]
pub fn watch_dir(app: AppHandle, state: tauri::State<'_, WatcherState>, path: String) -> AppResult<()> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(AppError::PathNotFound(path));
    }
    // 停掉旧监听
    *state.0.lock() = None;

    let app = Arc::new(app);
    let app_clone = app.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                // 只关心内容变化，过滤纯访问事件
                use notify::EventKind::*;
                match event.kind {
                    Create(_) | Modify(_) | Remove(_) => {
                        let _ = app_clone.emit("workspace-changed", ());
                    }
                    _ => {}
                }
            }
        },
        Config::default(),
    )
    .map_err(|e| AppError::Watch(e.to_string()))?;

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| AppError::Watch(e.to_string()))?;

    *state.0.lock() = Some(watcher);
    Ok(())
}

#[tauri::command]
pub fn unwatch_dir(state: tauri::State<'_, WatcherState>) -> AppResult<()> {
    *state.0.lock() = None;
    Ok(())
}
