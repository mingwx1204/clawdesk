//! `builtin:terminal` —— PowerShell 终端执行工具。
//!
//! 设计说明：
//! - 执行 PowerShell 命令，返回 stdout / stderr / exit code；
//! - 高危命令拦截：格式化磁盘、删除系统文件、清空等（独立于 HighRiskGuard 的双保险）；
//! - 内置超时（30s）防命令挂死；
//! - 标记高危：执行前需用户确认（StepConfirm / YOLO 放行）。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Windows 下隐藏子进程控制台窗口（防止执行 shell 命令时闪现黑窗口）。
/// 通过 CREATE_NO_WINDOW (0x08000000) 标志实现；非 Windows 平台无操作。
#[cfg(windows)]
pub(crate) fn hide_console(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
pub(crate) fn hide_console(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd
}

/// 高危命令片段（命中即拦截）。
const DANGEROUS_MARKERS: &[&str] = &[
    "format ",
    "format:",
    "del /s",
    "rm -rf /",
    "Remove-Item -Recurse -Force C:\\Windows",
    "Clear-Content C:\\Windows",
    "diskpart",
    "shutdown /s",
    "reg delete HKLM",
];

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "terminal",
        "执行 PowerShell 命令，返回输出与退出码（高危命令自动拦截）",
        vec![ToolParamDef {
            name: "command".into(),
            param_type: "string".into(),
            description: "要执行的 PowerShell 命令".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?
    .high_risk();

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_lowercase();
            if command.is_empty() {
                return Ok(ToolResult::err("command 不能为空"));
            }
            // 高危命令拦截
            if DANGEROUS_MARKERS.iter().any(|m| command.contains(m)) {
                return Ok(ToolResult::err(format!(
                    "命令包含高危操作，已拦截：{}",
                    args.get("command").and_then(|v| v.as_str()).unwrap_or("")
                )));
            }
            match run_powershell(args.get("command").and_then(|v| v.as_str()).unwrap_or("")) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("命令执行失败: {}", e))),
            }
        })
    });

    registry.register(def, handler)
}

/// 执行 PowerShell（带超时）。
///
/// ★ 管道死锁修复（2026-08-12）：spawn 后**立即**起两个 reader 线程边执行边收集
///   stdout/stderr —— 旧实现等进程退出后才读，输出超 64KB 管道缓冲时子进程阻塞
///   写不出去 → 永不退出 → 30s 超时。超时 kill 时用 taskkill /T 杀进程树（含子进程）。
fn run_powershell(command: &str) -> Result<serde_json::Value, String> {
    let mut cmd = std::process::Command::new("powershell");
    hide_console(&mut cmd)
        .args(["-NoProfile", "-Command", command])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 PowerShell: {}", e))?;

    // 立即起 reader 线程（单管道最多保留 1MB，超出丢弃但继续排空，防止再次阻塞子进程）
    const MAX_CAPTURE: usize = 1024 * 1024;
    let (tx_out, rx_out) = std::sync::mpsc::channel::<Vec<u8>>();
    if let Some(mut stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 65536];
            loop {
                match std::io::Read::read(&mut stdout, &mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if buf.len() < MAX_CAPTURE {
                            let take = n.min(MAX_CAPTURE - buf.len());
                            buf.extend_from_slice(&chunk[..take]);
                        }
                    }
                }
            }
            let _ = tx_out.send(buf);
        });
    } else {
        let _ = tx_out.send(Vec::new());
    }
    let (tx_err, rx_err) = std::sync::mpsc::channel::<Vec<u8>>();
    if let Some(mut stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 65536];
            loop {
                match std::io::Read::read(&mut stderr, &mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if buf.len() < MAX_CAPTURE {
                            let take = n.min(MAX_CAPTURE - buf.len());
                            buf.extend_from_slice(&chunk[..take]);
                        }
                    }
                }
            }
            let _ = tx_err.send(buf);
        });
    } else {
        let _ = tx_err.send(Vec::new());
    }

    // 简单超时：轮询等待（PowerShell 进程）
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("等待进程失败: {}", e))? {
            let out = rx_out.recv().unwrap_or_default();
            let err = rx_err.recv().unwrap_or_default();
            let out_s = String::from_utf8_lossy(&out).into_owned();
            let err_s = String::from_utf8_lossy(&err).into_owned();
            return Ok(json!({
                "exitCode": status.code().unwrap_or(-1),
                "stdout": truncate(&out_s, 4000),
                "stderr": truncate(&err_s, 2000),
            }));
        }
        if start.elapsed() > CMD_TIMEOUT {
            kill_process_tree(child.id());
            let _ = child.wait(); // 回收僵尸进程
            return Err("命令执行超时（30s），已终止".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// 超时终止：Windows 用 taskkill /T /F 杀整棵进程树（含子进程），
/// 避免只杀父进程留下后台子进程继续运行；非 Windows 直接 SIGKILL。
fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

/// 截断（char 边界安全）。
fn truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let head: String = chars[..max_chars].iter().collect();
        format!("{}…(+{}chars)", head, chars.len() - max_chars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dangerous_commands_blocked() {
        let lower = "format c: /y".to_lowercase();
        assert!(DANGEROUS_MARKERS.iter().any(|m| lower.contains(m)));
        let ok = "Get-ChildItem C:\\Users".to_lowercase();
        assert!(!DANGEROUS_MARKERS.iter().any(|m| ok.contains(m)));
    }

    #[test]
    fn run_simple_command_ok() {
        let out = run_powershell("Write-Output 'hello'").unwrap();
        assert!(out["stdout"].as_str().unwrap().contains("hello"));
        assert_eq!(out["exitCode"], 0);
    }
}
