//! 本地视觉模型（Qwen2.5-VL-7B @ llama-server）生命周期管理。
//!
//! 职责：
//! - 检测本地模型文件 / llama-server 可执行文件是否存在；
//! - 后台启动 llama-server 常驻服务（不阻塞主线程）；
//! - 提供统一的端点 URL / 端口常量，供 analyze_image 本地视觉 fallback 复用；
//! - 退出时回收子进程。
//!
//! 设计：零配置自动生效 —— 自动扫描常见位置发现模型文件与 llama-server，
//! 找到即拉起服务；文件缺失或已有一个服务在跑（端口占用）时安全跳过。
//!
//! 自动发现优先级（高→低）：
//!   1. 环境变量显式指定：CLAWDESK_VISION_MODEL_DIR / CLAWDESK_LLAMA_SERVER
//!   2. ClawDesk 应用数据目录下的 models/ 子目录
//!   3. 常见模型目录：D:\\workspace\\models、D:\\models
//!   4. 磁盘根目录浅层扫描（D:\\、C:\\ 各盘第一层 models 目录）

use std::path::{Path, PathBuf};
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

/// 返回视觉模型目录（含主模型 .gguf + mmproj .gguf）。
///
/// 自动发现：按优先级扫描，找到包含 mmproj 文件 + 主模型(.gguf) 的目录即返回。
fn model_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1) 环境变量显式指定
    if let Ok(env) = std::env::var("CLAWDESK_VISION_MODEL_DIR") {
        let p = PathBuf::from(env);
        if p.exists() {
            candidates.push(p);
        }
    }

    // 2) ClawDesk 应用数据目录下的 models/
    let clawdesk = crate::llm::settings::clawdesk_dir();
    candidates.push(clawdesk.join("models"));

    // 3) 常见模型目录
    for fixed in [
        "D:/workspace/models",
        "D:/models",
        "D:/Workspace/models",
        "C:/models",
    ] {
        candidates.push(PathBuf::from(fixed));
    }

    // 4) 对整个候选目录递归查找「含 mmproj 的目录」
    for root in &candidates {
        if let Some(dir) = find_vision_model_dir(root) {
            return Some(dir);
        }
    }

    // 5) 兜底：扫描各盘根下第一层含 models 的目录
    for drive in ["D:\\", "C:\\", "E:\\"] {
        let p = Path::new(drive);
        if !p.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && path.file_name().map(|n| {
                        let s = n.to_string_lossy().to_lowercase();
                        s.contains("model") || s.contains("模型")
                    }).unwrap_or(false)
                {
                    if let Some(dir) = find_vision_model_dir(&path) {
                        return Some(dir);
                    }
                }
            }
        }
    }

    None
}

/// 在 root 目录（含子目录）递归查找「视觉模型目录」：
/// 同时含 mmproj*.gguf 与主模型 .gguf(>500MB) 的目录。
fn find_vision_model_dir(root: &Path) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }
    // 递归收集所有 .gguf 文件（限制深度，避免全盘扫描过慢）
    let mut gguf_files: Vec<PathBuf> = Vec::new();
    collect_gguf(root, 3, &mut gguf_files);

    // 找到含 mmproj 的目录
    let mmproj_dirs: std::collections::HashSet<PathBuf> = gguf_files
        .iter()
        .filter(|f| f.file_name().map(|n| {
            n.to_string_lossy().to_lowercase().contains("mmproj")
        }).unwrap_or(false))
        .filter_map(|f| f.parent().map(|p| p.to_path_buf()))
        .collect();

    // 对每个含 mmproj 的目录，确认也有主模型（>500MB 的非 mmproj gguf）
    for dir in mmproj_dirs {
        let has_main_model = gguf_files.iter().any(|f| {
            f.parent() == Some(dir.as_path())
                && !f.file_name().map(|n| n.to_string_lossy().to_lowercase().contains("mmproj")).unwrap_or(false)
                && f.metadata().map(|m| m.len() > 500 * 1024 * 1024).unwrap_or(false)
        });
        if has_main_model {
            return Some(dir);
        }
    }
    None
}

/// 在指定目录内定位 (主模型, mmproj) 文件对。
/// 主模型 = 最大的非 mmproj .gguf；mmproj = 文件名含 mmproj 的 .gguf。
fn locate_model_files(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut main: Option<PathBuf> = None;
    let mut main_size: u64 = 0;
    let mut mmproj: Option<PathBuf> = None;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !path.extension().map(|e| e.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase().to_string()).unwrap_or_default();
        let size = path.metadata().map(|m| m.len()).unwrap_or(0);
        if name.contains("mmproj") {
            if mmproj.is_none() {
                mmproj = Some(path);
            }
        } else if size > main_size {
            main_size = size;
            main = Some(path);
        }
    }

    match (main, mmproj) {
        (Some(m), Some(p)) => Some((m, p)),
        _ => None,
    }
}

/// 递归收集 .gguf 文件（限制 depth）。
fn collect_gguf(dir: &Path, max_depth: usize, out: &mut Vec<PathBuf>) {
    if max_depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_gguf(&path, max_depth - 1, out);
        } else if path.extension().map(|e| e.eq_ignore_ascii_case("gguf")).unwrap_or(false) {
            out.push(path);
        }
    }
}

/// 返回 llama-server 可执行文件路径（自动发现）。
fn llama_server_exe() -> Option<PathBuf> {
    // 1) 环境变量显式指定
    if let Ok(env) = std::env::var("CLAWDESK_LLAMA_SERVER") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
    }

    // 2) 常见解压目录 + 递归浅扫
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("D:/workspace/llama-cpp-bin"),
        PathBuf::from("D:/llama.cpp"),
        PathBuf::from("D:/workspace/llama.cpp"),
        crate::llm::settings::clawdesk_dir().join("llama-cpp"),
    ];

    // 3) 盘根浅扫 llama-cpp 相关目录
    for drive in ["D:\\", "C:\\", "E:\\"] {
        let p = Path::new(drive);
        if !p.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.file_name().map(|n| {
                    let s = n.to_string_lossy().to_lowercase();
                    s.contains("llama")
                }).unwrap_or(false) {
                    candidates.push(path);
                }
            }
        }
    }

    // 逐个候选目录递归找 llama-server.exe（限深度 3）
    for root in &candidates {
        if let Some(exe) = find_exe(root, "llama-server.exe", 3) {
            return Some(exe);
        }
    }
    None
}

/// 递归查找指定文件名的可执行文件（限制 depth）。
fn find_exe(dir: &Path, name: &str, max_depth: usize) -> Option<PathBuf> {
    if !dir.exists() || max_depth == 0 {
        return None;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_exe(&path, name, max_depth - 1) {
                return Some(found);
            }
        } else if path.file_name().map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false) {
            return Some(path);
        }
    }
    None
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

    // 自动识别目录内的主模型（最大的非 mmproj gguf）与 mmproj 文件
    let (model, mmproj) = match locate_model_files(&dir) {
        Some(pair) => pair,
        None => {
            eprintln!("[LOCAL_VISION] 模型目录内未找到主模型+mmproj，跳过");
            return;
        }
    };
    eprintln!(
        "[LOCAL_VISION] 模型: {} | mmproj: {}",
        model.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        mmproj.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
    );

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

    /// 验证自动发现：能找到本机的模型目录与 llama-server（无文件环境则跳过）。
    #[test]
    fn autodiscover_model_and_server() {
        if std::env::var("CLAWDESK_TEST_LOCAL_VISION").as_deref() != Ok("1") {
            return;
        }
        let dir = model_dir();
        eprintln!("模型目录: {:?}", dir);
        assert!(dir.is_some(), "应能找到模型目录");

        let exe = llama_server_exe();
        eprintln!("llama-server: {:?}", exe);
        assert!(exe.is_some(), "应能找到 llama-server");

        // 验证目录内能定位到 (主模型, mmproj) 对
        let (model, mmproj) = locate_model_files(dir.as_ref().unwrap()).unwrap();
        eprintln!("主模型: {:?} | mmproj: {:?}", model, mmproj);
        assert!(model.file_name().unwrap().to_string_lossy().contains("VL") || model.metadata().unwrap().len() > 500*1024*1024);
        assert!(mmproj.file_name().unwrap().to_string_lossy().to_lowercase().contains("mmproj"));
    }

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
