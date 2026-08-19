//! Windows 集成 IPC 命令（项目 15：资源管理器/剪贴板/通知/开机自启）。

/// 在 Windows 资源管理器中打开文件所在文件夹（项目 15，§十四.3）。
#[tauri::command]
pub fn win_open_in_explorer(path: String) -> Result<(), String> {
    crate::llm::win_integration::open_in_explorer(&path)
}

/// 写入系统剪贴板（项目 15，§十四.2）。
#[tauri::command]
pub fn win_clipboard_set(text: String) -> Result<(), String> {
    crate::llm::win_integration::clipboard_set(&text)
}

/// 读取系统剪贴板文本（项目 15，§十四.2）。
#[tauri::command]
pub fn win_clipboard_get() -> Result<String, String> {
    crate::llm::win_integration::clipboard_get()
}

/// 弹出 Windows 原生系统通知（项目 15，§十四.4）。
#[tauri::command]
pub fn win_notify(title: String, body: String) -> Result<(), String> {
    crate::llm::win_integration::notify(&title, &body)
}

/// 设置 / 取消开机自启（项目 15，§十四.1）。
#[tauri::command]
pub fn win_autostart(enabled: bool) -> Result<(), String> {
    crate::llm::win_integration::autostart_set(enabled)
}
