//! 全局异常捕获兜底机制（项目 13，文档 §十三.2）。
//!
//! 设计说明：
//! - **panic hook**：捕获 Rust 侧所有未捕获 panic（异步任务 / 文件读写崩溃 /
//!   API 解析失败），**不会直接闪退软件**；
//! - 异常详情写入 `%APPDATA%/clawdesk/logs/last_error.json`（结构化：
//!   时间 / 消息 / 位置 / 完整日志路径），前端启动后经 `app_last_error`
//!   轮询查询，弹窗展示简化中文报错；
//! - 异常同时写入调试日志（debug.log，供开发者排查）；
//! - **自动终止任务**：前端收到异常后自动调用 `agent_cancel` 终止当前
//!   ReAct 任务，防止后台持续消耗 Token（前端配合实现，见 App.vue）。

use std::panic::{self, PanicHookInfo};
use std::path::PathBuf;
use std::sync::Once;

use serde_json::json;

/// 最近一次异常记录文件路径。
pub fn last_error_path() -> PathBuf {
    crate::llm::logging::log_dir().join("last_error.json")
}

/// 安装全局 panic hook（应用启动时调用一次；重复调用忽略）。
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        panic::set_hook(Box::new(|info: &PanicHookInfo| {
            record_panic(info);
        }));
    });
}

/// 捕获并记录 panic（写入 last_error.json + 调试日志）。
fn record_panic(info: &PanicHookInfo) {
    let message = panic_message(info);
    let location = info
        .location()
        .map(|l| format!("{}:{}", l.file(), l.line()))
        .unwrap_or_else(|| "未知位置".to_string());

    let record = json!({
        "timestamp": chrono::Local::now().to_rfc3339(),
        "message": message,
        "location": location,
        "logPath": crate::llm::logging::log_dir().join("debug.log").to_string_lossy(),
    });

    // 写 last_error.json（结构化，供前端查询弹窗）
    let path = last_error_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&record).unwrap_or_default(),
    );

    // 写调试日志（开发者排查）
    crate::llm::logging::debug("panic", &format!("{} @ {}", message, location));
}

/// 提取 panic 消息（字符串 / 格式化 payload 均兼容）。
fn panic_message(info: &PanicHookInfo) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "未知异常".to_string()
    }
}

/// 读取最近一次异常记录（无异常返回 None，供前端轮询）。
pub fn last_error() -> Option<serde_json::Value> {
    let path = last_error_path();
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_log_dir<T>(f: impl FnOnce() -> T) -> T {
        let _g = crate::llm::logging::test_env_lock();
        let dir = std::env::temp_dir().join(format!("clawdesk-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("APPDATA").ok();
        std::env::set_var("APPDATA", &dir);
        let result = f();
        match old {
            Some(v) => std::env::set_var("APPDATA", v),
            None => std::env::remove_var("APPDATA"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn no_error_returns_none() {
        with_temp_log_dir(|| {
            let _ = std::fs::remove_file(last_error_path());
            assert!(last_error().is_none());
        });
    }

    #[test]
    fn panic_recorded_and_readable() {
        with_temp_log_dir(|| {
            install(); // 安装 hook（幂等）
            // 触发一次 panic（在子线程捕获，避免测试本身 panic）
            let handle = std::thread::spawn(|| {
                let _ = panic::catch_unwind(|| {
                    panic!("测试异常: 文件读取失败");
                });
            });
            handle.join().unwrap();

            let err = last_error().expect("应记录异常");
            assert!(err["message"].as_str().unwrap().contains("文件读取失败"));
            assert!(err["location"].as_str().unwrap().contains(".rs"));
            // 调试日志同步写入
            let debug_log = crate::llm::logging::log_dir().join("debug.log");
            let content = std::fs::read_to_string(debug_log).unwrap_or_default();
            assert!(content.contains("文件读取失败"));
        });
    }
}
