//! 工具执行日志落盘 —— 独立日志文件，不占用 LLM 上下文 token。
//!
//! 每次工具调用追加写入 `%APPDATA%/clawdesk/tool_logs.log`，
//! 记录时间 / 工具 ID / 状态 / 耗时 / 参数摘要 / 输出摘要。
//! 供审计与模型复盘（后续可被 memory_search 检索）。

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 日志写入互斥（避免并发写交错）。
#[allow(dead_code)]
static LOG_MUTEX: Mutex<()> = Mutex::new(());

/// 日志文件路径：`<数据目录>/tool_logs.log`（数据目录优先 D 盘）。
#[allow(dead_code)]
fn log_path() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("tool_logs.log")
}

/// 记录一次工具调用（追加写，容量由外层按需轮转）。
#[allow(dead_code)]
pub fn record(
    tool_id: &str,
    status: &str,
    args: &serde_json::Value,
    output: &serde_json::Value,
    elapsed_ms: u64,
) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let line = format!(
        "[{}] tool={} status={} elapsed={}ms args={} output={}\n",
        chrono::Local::now().to_rfc3339(),
        tool_id,
        status,
        elapsed_ms,
        truncate_json(args, 300),
        truncate_json(output, 500),
    );

    // 追加写 + 防止并发交错
    let _guard = LOG_MUTEX.lock();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// JSON 摘要截断（char 边界安全）。
#[allow(dead_code)]
fn truncate_json(v: &serde_json::Value, max_chars: usize) -> String {
    let s = v.to_string();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s
    } else {
        let head: String = chars[..max_chars].iter().collect();
        format!("{}…(+{}chars)", head, chars.len() - max_chars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_writes_log_file() {
        let _g = crate::llm::logging::test_env_lock();
        let dir = std::env::temp_dir().join(format!("clawdesk-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 线程级覆盖：只有当前测试线程写 tool_logs.log 会落到临时目录
        crate::llm::settings::set_test_thread_data_dir(Some(dir.clone()));

        record(
            "builtin:get_time",
            "success",
            &serde_json::json!({}),
            &serde_json::json!({ "time": "12:00" }),
            5,
        );

        // ★ 实际路径：<数据目录>/tool_logs.log（无 clawdesk 子目录）
        let log = dir.join("tool_logs.log");
        assert!(log.exists(), "日志文件应已创建");
        let content = std::fs::read_to_string(&log).unwrap();
        assert!(content.contains("builtin:get_time"));
        assert!(content.contains("success"));

        crate::llm::settings::set_test_thread_data_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncate_json_is_char_safe() {
        let v = serde_json::json!({ "text": "中文内容".repeat(200) });
        let s = truncate_json(&v, 100);
        assert!(s.chars().count() <= 100 + 20); // + 后缀
        assert!(!s.contains('�'));
    }
}
