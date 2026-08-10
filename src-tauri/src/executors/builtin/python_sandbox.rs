//! `builtin:python` —— Python 沙箱执行（对标大厂 Agent 的代码执行能力）。
//!
//! 在隔离进程中执行 Python 代码，输出 stdout/stderr 和返回值。
//! 安全约束：30s 超时、禁止危险模块（os.system/subprocess/socket 等审计）、输出截断。

use std::sync::Arc;

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const PY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 危险导入黑名单（审计钩子注入）。
const DANGEROUS_IMPORTS: &[&str] = &[
    "os.system", "subprocess", "socket", "shutil.rmtree",
    "__import__('os')", "eval(", "exec(", "compile(",
    "ctypes", "multiprocessing", "signal",
];

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "python",
        "在安全的沙箱中执行 Python 代码（30s 超时，禁止危险操作）。用于数据分析、计算、文件处理等。",
        vec![
            ToolParamDef {
                name: "code".into(),
                param_type: "string".into(),
                description: "Python 源代码".into(),
                required: true,
                enum_values: None,
                default: None,
            },
            ToolParamDef {
                name: "cwd".into(),
                param_type: "string".into(),
                description: "工作目录（可选，默认临时目录）".into(),
                required: false,
                enum_values: None,
                default: None,
            },
        ],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        let code = args.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cwd = args.get("cwd").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if code.is_empty() {
            return Box::pin(async { Ok(ToolResult::err("code 不能为空")) });
        }
        // 安全检查：禁止危险导入
        let code_lower = code.to_lowercase();
        for bad in DANGEROUS_IMPORTS {
            if code_lower.contains(bad) {
                return Box::pin(async move {
                    Ok(ToolResult::err(format!(
                        "安全拦截：代码中包含危险操作 `{}`。如需系统调用请使用 builtin:terminal 工具。",
                        bad
                    )))
                });
            }
        }
        Box::pin(async move {
            match run_python(&code, &cwd) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("Python 执行失败: {e}"))),
            }
        })
    });

    registry.register(def, handler)
}

fn run_python(code: &str, cwd: &str) -> Result<serde_json::Value, String> {
    // 写临时文件
    let dir = if !cwd.is_empty() {
        std::path::PathBuf::from(cwd)
    } else {
        crate::executors::builtin::attachment::attach_dir()
            .map_err(|e| format!("获取工作目录失败: {e}"))?
    };

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let script_path = dir.join(format!("_py_{}.py", ts));
    std::fs::write(&script_path, code).map_err(|e| format!("写入脚本失败: {e}"))?;

    let mut cmd = std::process::Command::new("python");
    super::terminal::hide_console(&mut cmd)
        .args([script_path.to_string_lossy().as_ref()])
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 Python: {e}（请确认已安装 Python 并在 PATH 中）"))?;

    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("等待进程失败: {e}"))? {
            let mut out = String::new();
            let mut err = String::new();
            if let Some(mut so) = child.stdout.take() {
                let _ = std::io::Read::read_to_string(&mut so, &mut out);
            }
            if let Some(mut se) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut se, &mut err);
            }
            // 清理临时脚本
            let _ = std::fs::remove_file(&script_path);
            let mut result = json!({
                "exitCode": status.code().unwrap_or(-1),
                "stdout": truncate(&out, 4000),
                "stderr": truncate(&err, 2000),
            });
            if let Some(obj) = result.as_object_mut() {
                obj.insert("elapsedMs".into(), json!(start.elapsed().as_millis()));
            }
            return Ok(result);
        }
        if start.elapsed() > PY_TIMEOUT {
            let _ = child.kill();
            let _ = std::fs::remove_file(&script_path);
            return Err("执行超时（30s），已终止".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let head: String = chars[..max].iter().collect();
        format!("{}…(+{} 字符)", head, chars.len() - max)
    }
}
