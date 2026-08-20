//! 本地视觉模型（Qwen2.5-VL-7B @ llama-server）生命周期管理。
//!
//! 职责：
//! - 检测本地模型文件 / llama-server 可执行文件是否存在；
//! - 后台启动 llama-server 常驻服务（不阻塞主线程）；
//! - 提供统一的端点 URL / 端口常量，供 analyze_image 本地视觉 fallback 复用；
//! - 退出时回收子进程。
//!
//! 设计：零配置自动生效 —— 只要模型文件在预期目录，ClawDesk 启动时自动拉起服务；
//! 文件缺失或已有一个服务在跑（端口占用）时安全跳过。

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// 本地视觉服务监听端口（analyze_image 的 local_vision_fallback 复用）。
pub const LOCAL_VISION_PORT: u16 = 8088;
/// 本地视觉 health 端点。
pub const LOCAL_VISION_HEALTH: &str = "http://127.0.0.1:8088/health";
/// 本地视觉 chat 端点。
pub const LOCAL_VISION_URL: &str = "http://127.0.0.1:8088/v1/chat/completions";
/// 本地视觉模型名（llama-server 的 model 字段，也用于结果标记）。
pub const LOCAL_VISION_MODEL: &str = "qwen2.5-vl-7b";

/// 已启动的 llama-server 子进程句柄（退出时回收）。
static LLAMA_SERVER: Mutex<Option<Child>> = Mutex::new(None);

/// 是否已启动过（避免重复 spawn）。
static STARTED: AtomicBool = AtomicBool::new(false);

/// 返回模型目录（若模型文件存在）。
fn model_dir() -> Option<PathBuf> {
    // 模型存放目录（手动下载 + Motrix 归位后固定于此）
    let dir = PathBuf::from("D:/workspace/models/qwen2.5-vl-7b");
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

/// 返回 llama-server 可执行文件路径（若存在）。
fn llama_server_exe() -> Option<PathBuf> {
    // llama.cpp 预编译 CUDA 版解压目录
    let exe = PathBuf::from("D:/workspace/llama-cpp-bin/llama-server.exe");
    if exe.exists() {
        Some(exe)
    } else {
        None
    }
}

/// 后台启动 llama-server（非阻塞，spawn 后立即返回）。
///
/// 幂等：已启动过 / 文件缺失 / 端口占用（已有服务）时安全跳过，
/// 不抛错、不影响主流程。
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return; // 已尝试过启动
    }

    let (Some(dir), Some(exe)) = (model_dir(), llama_server_exe()) else {
        eprintln!("[LOCAL_VISION] 模型或 llama-server 缺失，跳过本地视觉服务");
        return;
    };

    let model = dir.join("Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf");
    let mmproj = dir.join("mmproj-model-f16.gguf");
    if !model.exists() || !mmproj.exists() {
        eprintln!("[LOCAL_VISION] 模型文件不完整，跳过本地视觉服务");
        return;
    }

    // 先探测端口是否已被占用（可能是上次残留的服务 / 用户手动起的）
    if health_ok() {
        eprintln!("[LOCAL_VISION] 检测到已有本地视觉服务在 {}, 复用", LOCAL_VISION_PORT);
        return;
    }

    eprintln!("[LOCAL_VISION] 启动本地视觉模型服务（Qwen2.5-VL-7B @ llama-server）...");
    match Command::new(&exe)
        .arg("-m").arg(&model)
        .arg("--mmproj").arg(&mmproj)
        .arg("-ngl").arg("22")
        .arg("-c").arg("2048")
        .arg("--host").arg("127.0.0.1")
        .arg("--port").arg(LOCAL_VISION_PORT.to_string())
        // 关键：设置 cwd 到 llama-server 所在目录，否则找不到同目录 DLL
        .current_dir(exe.parent().unwrap_or(std::path::Path::new(".")))
        .spawn()
    {
        Ok(child) => {
            let mut guard = LLAMA_SERVER.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(child);
            eprintln!("[LOCAL_VISION] llama-server 已启动（后台加载模型中，约 10~20 秒就绪）");
        }
        Err(e) => {
            eprintln!("[LOCAL_VISION] llama-server 启动失败: {}", e);
        }
    }
}

/// 探测本地视觉服务是否已就绪（health 检查，短超时）。
pub fn health_ok() -> bool {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_millis(500))
        .build();
    agent
        .get(LOCAL_VISION_HEALTH)
        .call()
        .map(|_| true)
        .unwrap_or(false)
}

/// 终止 llama-server 子进程（退出时调用）。
pub fn stop() {
    let mut guard = LLAMA_SERVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("[LOCAL_VISION] llama-server 已回收");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 集成验证：auto-start 能拉起 llama-server，health 就绪后 stop 回收。
    /// 仅当模型文件存在时生效（无模型环境直接返回）。
    #[test]
    fn autostart_then_stop() {
        if std::env::var("CLAWDESK_TEST_LOCAL_VISION").as_deref() != Ok("1") {
            return; // 默认跳过
        }
        // 确保干净起点
        stop();
        STARTED.store(false, Ordering::SeqCst);

        eprintln!("调用 start()...");
        start();
        assert!(LLAMA_SERVER.lock().unwrap().is_some(), "start 后应有子进程句柄");

        // 轮询 health 直至就绪或超时（模型加载约 10~20 秒）
        let mut ready = false;
        for i in 0..60 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if health_ok() {
                ready = true;
                eprintln!("服务就绪，耗时约 {}s", i + 1);
                break;
            }
        }
        assert!(ready, "llama-server 未在 60s 内就绪");

        // 回收
        stop();
        assert!(LLAMA_SERVER.lock().unwrap().is_none());
    }
}
