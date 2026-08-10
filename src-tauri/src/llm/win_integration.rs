//! Windows 深度适配模块（项目 15，文档 §十四）。
//!
//! 设计说明：
//! - 全部通过系统命令 / PowerShell 实现，**不引入新依赖**（保持最小体积）：
//!   - 资源管理器联动：`explorer.exe /select,<path>` 打开文件所在文件夹并选中；
//!   - 系统剪贴板：`Set-Clipboard` / `Get-Clipboard`（文本与图片路径通用）；
//!   - 系统通知：WinForms `NotifyIcon.ShowBalloonTip`（Windows 原生气泡通知）；
//!   - 开机自启：注册表 `HKCU\...\Run` 键（HKCU 免管理员权限）；
//! - 所有调用失败返回可读中文错误（供前端 toast / 调试日志）；
//! - 空实现 / 非 Windows 平台返回错误（不 panic）。

use std::process::Command;

/// 在 Windows 资源管理器中打开文件所在文件夹（并选中该文件）。
pub fn open_in_explorer(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("路径不能为空".into());
    }
    let mut cmd = Command::new("explorer.exe");
    crate::executors::builtin::terminal::hide_console(&mut cmd);
    let out = cmd
        .arg(format!("/select,{}", path.trim()))
        .output()
        .map_err(|e| format!("启动资源管理器失败: {}", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "资源管理器返回错误: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 写入系统剪贴板（文本 / 路径）。
pub fn clipboard_set(text: &str) -> Result<(), String> {
    // 通过 PowerShell Set-Clipboard 写入（避免引入剪贴板依赖）
    let script = format!(
        "$t = [Console]::In.ReadToEnd(); Set-Clipboard -Value $t"
    );
    let mut cmd = Command::new("powershell");
    crate::executors::builtin::terminal::hide_console(&mut cmd);
    let mut child = cmd
        .args(["-NoProfile", "-Command", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 PowerShell 失败: {}", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("等待剪贴板写入失败: {}", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "剪贴板写入失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 读取系统剪贴板文本（无内容返回空串）。
pub fn clipboard_get() -> Result<String, String> {
    let mut cmd = Command::new("powershell");
    crate::executors::builtin::terminal::hide_console(&mut cmd);
    let out = cmd
        .args(["-NoProfile", "-Command", "Get-Clipboard -Raw"])
        .output()
        .map_err(|e| format!("读取剪贴板失败: {}", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "剪贴板读取失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 弹出 Windows 原生系统通知（WinForms 托盘气泡）。
///
/// 说明：应用未常驻托盘时，NotifyIcon 短生命周期气泡仍可显示系统通知；
/// 若用户系统禁用了应用通知，此调用静默失败（不影响主流程）。
pub fn notify(title: &str, body: &str) -> Result<(), String> {
    let esc_title = title.replace('\'', "''");
    let esc_body = body.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $n = New-Object System.Windows.Forms.NotifyIcon; \
         $n.Icon = [System.Drawing.SystemIcons]::Information; \
         $n.Visible = $true; \
         $n.ShowBalloonTip(5000, '{0}', '{1}', [System.Windows.Forms.ToolTipIcon]::Info); \
         Start-Sleep -Milliseconds 6000; \
         $n.Dispose()",
        esc_title, esc_body
    );
    let mut cmd = Command::new("powershell");
    crate::executors::builtin::terminal::hide_console(&mut cmd);
    let out = cmd
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script])
        .output()
        .map_err(|e| format!("发送系统通知失败: {}", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "系统通知失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 设置 / 取消开机自启（HKCU Run 键，免管理员权限）。
pub fn autostart_set(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "clawdesk.exe".to_string());
    let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let mut cmd = Command::new("reg");
    crate::executors::builtin::terminal::hide_console(&mut cmd);
    let out = if enabled {
        cmd.args(["add", run_key, "/v", "ClawDesk", "/t", "REG_SZ", "/d", &exe, "/f"])
            .output()
            .map_err(|e| format!("设置开机自启失败: {e}"))?
    } else {
        cmd.args(["delete", run_key, "/v", "ClawDesk", "/f"])
            .output()
            .map_err(|e| format!("取消开机自启失败: {e}"))?
    };
    if out.status.success() {
        eprintln!("[AUTOSTART] 开机自启: {}", if enabled { "已启用" } else { "已禁用" });
        Ok(())
    } else {
        Err(format!("注册表操作失败: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_explorer_rejects_empty() {
        let err = open_in_explorer("  ").unwrap_err();
        assert!(err.contains("不能为空"));
    }

    #[test]
    fn clipboard_get_returns_string_or_error() {
        // 读剪贴板：任何结果都不 panic（成功返回 String，失败返回 Err）
        match clipboard_get() {
            Ok(s) => assert!(s.len() <= 1_000_000),
            Err(e) => assert!(!e.is_empty()),
        }
    }

    #[test]
    fn notify_does_not_panic() {
        // 系统通知：失败（如无桌面会话）也应返回错误而非 panic
        let _ = notify("测试", "ClawDesk 自检通知");
    }
}
