//! 文件快照 IPC 命令（快照回滚面板数据源，项目 6）。

/// 列出全部文件修改快照（快照回滚面板数据源，项目 6）。
#[tauri::command]
pub fn snapshot_list() -> Vec<crate::executors::builtin::snapshot::SnapEntry> {
    crate::executors::builtin::snapshot::list_snapshots()
}

/// 回滚指定快照（覆盖原文件）；返回回滚结果。
#[tauri::command]
pub fn snapshot_restore(snapshot_id: String) -> Result<serde_json::Value, String> {
    crate::executors::builtin::snapshot::restore_snapshot(&snapshot_id)
        .map_err(|e| format!("回滚失败: {}", e))
}

/// 删除指定快照（文件 + 索引项）；返回是否成功。
#[tauri::command]
pub fn snapshot_delete(snapshot_id: String) -> bool {
    crate::executors::builtin::snapshot::delete_snapshot(&snapshot_id).unwrap_or(false)
}

/// 对比快照与当前文件的差异（回滚前审查）。
#[tauri::command]
pub fn snapshot_diff(snapshot_id: String) -> Result<serde_json::Value, String> {
    crate::executors::builtin::snapshot::diff_snapshot(&snapshot_id)
        .map_err(|e| format!("对比失败: {}", e))
}
