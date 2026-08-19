//! 日志查询 & 健康自检 IPC 命令（项目 12/13/14）。

/// 读取最近 N 行日志（三级日志体系查询：debug=调试 / audit=审计，项目 12）。
/// kind 支持 "debug" / "audit"（旧值 agent/engine/settings 兼容映射到 debug）。
#[tauri::command]
pub fn logs_tail(kind: String, lines: Option<usize>) -> Vec<String> {
    let kind = if kind.eq_ignore_ascii_case("audit") {
        crate::llm::logging::LogKind::Audit
    } else {
        crate::llm::logging::LogKind::Debug
    };
    crate::llm::logging::tail(kind, lines.unwrap_or(100).min(500))
}

/// 日志文件大小（字节，供自检/面板展示，项目 12）。
#[tauri::command]
pub fn logs_size(kind: String) -> u64 {
    let kind = if kind.eq_ignore_ascii_case("audit") {
        crate::llm::logging::LogKind::Audit
    } else {
        crate::llm::logging::LogKind::Debug
    };
    crate::llm::logging::size(kind)
}

/// 查询最近一次未捕获异常（全局异常捕获兜底，项目 13）。
/// 前端启动后轮询：有异常弹中文报错 + 自动取消任务。
#[tauri::command]
pub fn app_last_error() -> Option<serde_json::Value> {
    crate::llm::error_guard::last_error()
}

/// 执行启动健康自检（项目 14，§十三.4）：SQLite / MCP / API / 目录。
/// 前端启动时调用，失败项弹窗展示中文修复方案。
#[tauri::command]
pub fn self_check_run() -> serde_json::Value {
    let items = crate::llm::self_check::run_all();
    if crate::llm::self_check::has_failure(&items) {
        crate::llm::logging::debug("self_check", "启动自检存在失败项");
    }
    // 失败项写入调试日志
    for item in &items {
        if item.status == "fail" {
            crate::llm::logging::debug("self_check", &format!("{}: {}", item.name, item.detail));
        }
    }
    crate::llm::self_check::summary(&items)
}
