//! 三级日志体系（项目 12，文档 §十三.1）。
//!
//! 设计说明：
//! - **调试日志**（`debug.log`）：Rust 运行报错、模型 API 请求详情、前后端通信异常，
//!   仅开发者查看；容量上限 5MB，超限轮转保留 `debug.log.1`；
//! - **审计日志**（`audit.log`）：永久留存文件修改 / 终端命令 / 高危操作 / 图像生成 /
//!   MCP 调用记录（由各执行器显式调用 `audit()` 落盘），不可一键批量删除；
//! - **用户对话日志**：已由 SQLite sessions 表承载（记忆体系打通，无需重复落盘）；
//! - 全部日志**不占用 LLM 上下文 token**（独立文件存储）；
//! - 容量管控：单文件超过 `MAX_BYTES` 自动轮转（.1 覆盖），防止无限写盘（§十七.4）。

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 单日志文件容量上限：5MB。
const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// 日志写互斥（避免并发交错）。
static LOG_MUTEX: Mutex<()> = Mutex::new(());

/// 日志类型（对应三级分层中的两档文件；对话日志走 SQLite）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// 调试日志（开发者查看）。
    Debug,
    /// 审计日志（永久留存，不可批量删除）。
    Audit,
}

impl LogKind {
    fn file_name(self) -> &'static str {
        match self {
            Self::Debug => "debug.log",
            Self::Audit => "audit.log",
        }
    }
}

/// 日志根目录：`<数据目录>/logs/`（数据目录优先 D 盘）。
pub fn log_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("logs")
}

/// 写一条结构化日志（追加 + 容量轮转）。
pub fn write(kind: LogKind, category: &str, message: &str) {
    let path = log_dir().join(kind.file_name());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let line = format!(
        "[{}] [{}] {}\n",
        chrono::Local::now().to_rfc3339(),
        category,
        message
    );

    let _guard = LOG_MUTEX.lock();
    rotate_if_needed(&path, MAX_BYTES);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 写调试日志（模型 API 请求/报错/通信异常）。
pub fn debug(category: &str, message: &str) {
    write(LogKind::Debug, category, message);
}

/// 写审计日志（文件修改/终端/高危/图像/MCP —— 永久留存）。
pub fn audit(category: &str, message: &str) {
    write(LogKind::Audit, category, message);
}

/// 读取最近 N 行日志（供设置面板日志查看）。
pub fn tail(kind: LogKind, lines: usize) -> Vec<String> {
    let path = log_dir().join(kind.file_name());
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    content.lines().rev().take(lines).map(|s| s.to_string()).collect()
}

/// 日志文件是否存在及当前大小（自检用）。
pub fn size(kind: LogKind) -> u64 {
    let path = log_dir().join(kind.file_name());
    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
}

/// 测试专用：串行锁 —— logging / error_guard / self_check / export 的测试
/// 都会 `set_var("APPDATA")` 指向各自临时目录，并行运行会互相污染；
/// 统一复用本锁保证这些测试串行执行（§十六.4 独立测试覆盖）。
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap()
}

/// 超过容量上限时轮转：`x.log` → `x.log.1`（覆盖旧轮转文件）。
fn rotate_if_needed(path: &PathBuf, max_bytes: u64) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= max_bytes {
            let rotated = path.with_extension(format!(
                "{}.1",
                path.extension().and_then(|e| e.to_str()).unwrap_or("log")
            ));
            let _ = std::fs::rename(path, &rotated);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试专用：把日志目录指向临时目录（覆盖层方案，避免 set_var 污染并行测试）。
    fn with_temp_log_dir<T>(f: impl FnOnce() -> T) -> T {
        let _g = super::test_env_lock();
        let dir = std::env::temp_dir().join(format!("clawdesk-logdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // ★ 用 CLAWDESK_DATA_DIR 覆盖（clawdesk_dir 优先读它；只覆盖 APPDATA
        //   在真实数据目录 D:\ClawDeskData 存在时会写入真实目录，并行测试互相污染）
        let old = std::env::var("CLAWDESK_DATA_DIR").ok();
        std::env::set_var("CLAWDESK_DATA_DIR", &dir);
        let result = f();
        match old {
            Some(v) => std::env::set_var("CLAWDESK_DATA_DIR", v),
            None => std::env::remove_var("CLAWDESK_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn debug_and_audit_write_separate_files() {
        with_temp_log_dir(|| {
            debug("api", "请求成功 model=deepseek");
            audit("file_write", "D:\\work\\a.txt");

            let d = std::fs::read_to_string(log_dir().join("debug.log")).unwrap();
            let a = std::fs::read_to_string(log_dir().join("audit.log")).unwrap();
            assert!(d.contains("deepseek"));
            assert!(a.contains("file_write"));
            // 互不串扰
            assert!(!d.contains("file_write"));
            assert!(!a.contains("deepseek"));
        });
    }

    #[test]
    fn tail_returns_recent_lines() {
        with_temp_log_dir(|| {
            for i in 0..5 {
                audit("t", &format!("line{}", i));
            }
            let lines = tail(LogKind::Audit, 3);
            assert_eq!(lines.len(), 3);
            assert!(lines[0].contains("line4"));
            assert!(lines[2].contains("line2"));
        });
    }

    #[test]
    fn rotate_after_capacity() {
        with_temp_log_dir(|| {
            // 写大量数据触发轮转（容量 5MB 太大，直接验证 rotate_if_needed 逻辑）
            let path = log_dir().join("debug.log");
            std::fs::create_dir_all(log_dir()).unwrap();
            std::fs::write(&path, vec![b'x'; 10]).unwrap();
            rotate_if_needed(&path, 5);
            // 5 < 10 → 应轮转
            let rotated = log_dir().join("debug.log.1");
            assert!(rotated.exists(), "应产生轮转文件");
            assert!(!path.exists(), "原文件应已移走");
        });
    }

    #[test]
    fn missing_log_tail_empty() {
        with_temp_log_dir(|| {
            assert!(tail(LogKind::Audit, 10).is_empty());
            assert_eq!(size(LogKind::Audit), 0);
        });
    }
}
