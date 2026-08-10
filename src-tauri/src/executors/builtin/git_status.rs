//! `builtin:git_status` —— Git 仓库状态查询工具。
//!
//! 设计说明：
//! - 执行 `git status --porcelain`、`git branch`、`git log` 等只读命令；
//! - 非高危工具（只读），不执行任何写操作；
//! - 命令超时 15s，防止大仓库或网络仓库挂死；
//! - 路径安全校验：拒绝系统敏感路径。

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

const GIT_TIMEOUT: Duration = Duration::from_secs(15);

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "git_status",
        "查询 Git 仓库状态：当前分支、工作区变更、暂存区状态、最近提交记录（只读）",
        vec![ToolParamDef {
            name: "path".into(),
            param_type: "string".into(),
            description: "Git 仓库根目录的绝对路径".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
            if path.is_empty() {
                return Ok(ToolResult::err("path 不能为空"));
            }
            if super::analyze_image::is_sensitive_path(path) {
                return Ok(ToolResult::err("禁止访问系统敏感路径"));
            }
            if !is_git_repo(path) {
                return Ok(ToolResult::err(format!("路径不是 Git 仓库: {}", path)));
            }
            match query_git_status(path) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("Git 查询失败: {}", e))),
            }
        })
    });

    registry.register(def, handler)
}

/// 检查路径是否是一个 Git 仓库（存在 .git 目录）。
fn is_git_repo(path: &str) -> bool {
    Path::new(path).join(".git").is_dir()
}

/// 综合查询 Git 仓库状态：分支 + 状态 + 最近提交。
fn query_git_status(repo_path: &str) -> Result<serde_json::Value, String> {
    let branch = git_command(repo_path, &["branch", "--show-current"])?;
    let status_lines = git_command(repo_path, &["status", "--porcelain"])?;
    let log_lines = git_command(repo_path, &["log", "--oneline", "-10", "--no-decorate"])?;
    let remote = git_command(repo_path, &["remote", "-v"]).unwrap_or_default();

    // 解析 status --porcelain
    let files: Vec<serde_json::Value> = status_lines
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let x = l.get(0..1).unwrap_or(" ");
            let y = l.get(1..2).unwrap_or(" ");
            let file = l.get(3..).unwrap_or("").trim().to_string();
            json!({
                "index": x,      // 暂存区状态
                "workTree": y,   // 工作区状态
                "file": file,
            })
        })
        .collect();

    // 解析 remote
    let remotes: Vec<serde_json::Value> = remote
        .lines()
        .map(|l| {
            let parts: Vec<&str> = l.splitn(2, '\t').collect();
            json!({
                "name": parts.first().unwrap_or(&""),
                "url": parts.get(1).unwrap_or(&"").trim_end_matches(" (fetch)").trim_end_matches(" (push)"),
            })
        })
        .collect();

    Ok(json!({
        "path": repo_path,
        "branch": branch.trim(),
        "dirty": !status_lines.trim().is_empty(),
        "changedFiles": files.len(),
        "files": files,
        "recentCommits": log_lines.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
        "remotes": remotes,
    }))
}

/// 执行 git 命令并返回 stdout（带超时）。
fn git_command(repo_path: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    super::terminal::hide_console(&mut cmd)
        .current_dir(repo_path)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 git: {}", e))?;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("等待 git 进程失败: {}", e))? {
            let mut out = String::new();
            if let Some(mut stdout) = child.stdout.take() {
                let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
            }
            if !status.success() {
                let mut err = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
                }
                if !err.is_empty() {
                    return Err(err.trim().to_string());
                }
            }
            return Ok(out);
        }
        if start.elapsed() > GIT_TIMEOUT {
            let _ = child.kill();
            return Err("Git 命令超时（15s），已终止".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_status_def_is_well_formed() {
        let def = UnifiedToolDef::new(
            "builtin",
            "git_status",
            "x",
            vec![ToolParamDef {
                name: "path".into(),
                param_type: "string".into(),
                description: "d".into(),
                required: true,
                enum_values: None,
                default: None,
            }],
        )
        .unwrap();
        assert_eq!(def.id, "builtin:git_status");
        def.validate_id().unwrap();
    }

    #[test]
    fn non_git_dir_rejected() {
        let tmp = std::env::temp_dir().join(format!("clawdesk-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(!is_git_repo(tmp.to_str().unwrap()));
        let err = query_git_status(tmp.to_str().unwrap()).unwrap_err();
        assert!(
            err.contains("git") || err.contains("无法启动"),
            "{}",
            err
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sensitive_path_blocked() {
        use crate::executors::builtin::analyze_image;
        assert!(analyze_image::is_sensitive_path("C:\\Windows\\temp"));
    }

    /// 在当前工作空间执行（workspace 自身是 git 仓库）。
    /// 此测试依赖 workspace 存在 .git 目录。
    #[test]
    fn workspace_git_status_ok() {
        // 找到 workspace 根目录（ClawDesk 父目录）
        let repo = "D:\\workspace";
        if !is_git_repo(repo) {
            // CI/非 git 环境跳过
            return;
        }
        let out = query_git_status(repo).unwrap();
        assert!(!out["branch"].as_str().unwrap().is_empty());
        assert!(out["recentCommits"].as_array().unwrap().len() <= 10);
    }
}
