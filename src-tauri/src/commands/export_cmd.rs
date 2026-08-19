//! 一键导出 IPC 命令（项目 17，§十五.3）。

/// 一键导出完整项目成果（项目 17，§十五.3）：快照/日志/图像/会话/报告打包 zip。
#[tauri::command]
pub fn export_all() -> Result<String, String> {
    crate::llm::export::export_all()
}
