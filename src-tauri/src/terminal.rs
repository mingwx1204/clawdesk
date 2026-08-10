//! 终端会话：基于 portable-pty 的真实 PTY，输出通过 Tauri 事件流式推给前端。
//! 前端负责 ANSI 渲染，Rust 侧只做原始字节转发（零拷贝、低延迟）。

use crate::error::{AppError, AppResult};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use tauri::{AppHandle, Emitter};

pub struct PtySession {
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>, // 持有 master，drop 时 PTY 关闭
}

pub struct TerminalState(pub Mutex<HashMap<String, PtySession>>);

impl Default for TerminalState {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

/// 启动一个终端会话（Windows 下默认 PowerShell，回退 cmd）
#[tauri::command]
pub fn terminal_spawn(app: AppHandle, state: tauri::State<'_, TerminalState>, cwd: Option<String>) -> AppResult<String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| AppError::TerminalSpawn(e.to_string()))?;

    #[cfg(target_os = "windows")]
    let shell = if which_shell("powershell.exe") { "powershell.exe" } else { "cmd.exe" };
    #[cfg(not(target_os = "windows"))]
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    #[cfg(target_os = "windows")]
    let mut cmd = CommandBuilder::new(shell);
    #[cfg(not(target_os = "windows"))]
    let mut cmd = CommandBuilder::new(shell.as_str());

    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| AppError::TerminalSpawn(e.to_string()))?;

    let session_id = uuid_v4();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| AppError::TerminalSpawn(e.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| AppError::TerminalSpawn(e.to_string()))?;

    // 读取线程：把 PTY 输出按块推送到前端事件
    let sid = session_id.clone();
    let app_clone = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    // 事件名带会话 id，前端按需订阅
                    let _ = app_clone.emit(&format!("terminal-output-{}", sid), text);
                }
                Err(_) => break,
            }
        }
        let _ = app_clone.emit(&format!("terminal-exit-{}", sid), "");
    });

    state.0.lock().insert(
        session_id.clone(),
        PtySession {
            writer,
            child,
            master: pair.master,
        },
    );
    Ok(session_id)
}

/// 向终端写入输入（用户键入或 Agent 下发的命令）
#[tauri::command]
pub fn terminal_write(state: tauri::State<'_, TerminalState>, session_id: String, data: String) -> AppResult<()> {
    let mut sessions = state.0.lock();
    let session = sessions.get_mut(&session_id).ok_or(AppError::TerminalNotFound)?;
    session
        .writer
        .write_all(data.as_bytes())
        .and_then(|_| session.writer.flush())?;
    Ok(())
}

/// 终止终端会话
#[tauri::command]
pub fn terminal_kill(state: tauri::State<'_, TerminalState>, session_id: String) -> AppResult<()> {
    let mut sessions = state.0.lock();
    let mut session = sessions.remove(&session_id).ok_or(AppError::TerminalNotFound)?;
    let _ = session.child.kill();
    Ok(())
}

#[cfg(target_os = "windows")]
fn which_shell(name: &str) -> bool {
    std::process::Command::new("where")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", nanos, std::process::id())
}
