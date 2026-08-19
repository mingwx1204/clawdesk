//! 沙箱授权根目录 IPC 命令。

use tauri::State;

use crate::commands::AppState;

/// 列出当前沙箱授权根目录。
#[tauri::command]
pub fn sandbox_roots(state: State<'_, AppState>) -> Vec<String> {
    state.sandbox.roots()
}

/// 添加一个沙箱授权根目录；返回是否新增成功（重复添加返回 false）。
/// 成功后持久化到 settings.sandboxRoots（重启自动恢复）。
#[tauri::command]
pub fn sandbox_add_root(state: State<'_, AppState>, path: String) -> bool {
    let ok = state.sandbox.add_root(&path);
    if ok {
        let mut cur = state.settings.get();
        if !cur.sandbox_roots.contains(&path) {
            cur.sandbox_roots.push(path.clone());
            let _ = state
                .settings
                .apply(serde_json::json!({ "sandboxRoots": cur.sandbox_roots }));
        }
        eprintln!("[SANDBOX] 已添加授权根: {}", path);
    }
    ok
}

/// 移除一个沙箱授权根目录；返回是否移除成功。成功后同步持久化。
#[tauri::command]
pub fn sandbox_remove_root(state: State<'_, AppState>, path: String) -> bool {
    let ok = state.sandbox.remove_root(&path);
    if ok {
        let mut cur = state.settings.get();
        cur.sandbox_roots.retain(|r| r != &path);
        let _ = state
            .settings
            .apply(serde_json::json!({ "sandboxRoots": cur.sandbox_roots }));
        eprintln!("[SANDBOX] 已移除授权根: {}", path);
    }
    ok
}
