//! `HighRiskGuardMiddleware` —— 高危守卫 + 路径安全检查。
//!
//! 契约：
//! - **风险分级审计**：按 `risk::risk_of` 判定普通/高危，输出分级告警日志
//!   （前端已确认，不重复拦截）；
//! - **路径安全校验**：工具参数中的路径字段（path / file_path / image_path /
//!   output_dir 等，递归查找）若指向系统敏感目录 → 拦截拒绝；
//! - 覆盖生图（输出目录）、识图（读取路径）、文件读写等全部工具；
//! - 分级信息（RiskLevel）随审计日志输出，并随 ConfirmRequired 事件推前端。

use crate::core::tool::def::UnifiedToolDef;
use crate::core::tool::dispatcher::{BoxFuture, Middleware, ToolCall};
use crate::core::tool::error::ToolError;

use super::risk::{risk_of, RiskLevel};

pub struct HighRiskGuardMiddleware;

/// 会被检查的路径字段名（递归匹配）。
const PATH_KEYS: &[&str] = &[
    "path",
    "file_path",
    "image_path",
    "output_dir",
    "output_path",
    "dir",
    "directory",
];

impl Middleware for HighRiskGuardMiddleware {
    fn name(&self) -> &'static str {
        "high_risk_guard"
    }

    fn before<'a>(
        &'a self,
        def: &'a UnifiedToolDef,
        call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            // 风险分级审计（普通 / 高危）
            let level = risk_of(def, &call.arguments);
            if level.is_high() {
                eprintln!(
                    "[HIGH_RISK] level={} tool={} call_id={} round={} — 高危工具调用已通过前端确认",
                    level_to_str(level),
                    def.id,
                    call.id,
                    call.round
                );
            }

            // 路径安全校验（所有工具统一防护）
            if let Some(bad) = find_sensitive_path_arg(&call.arguments) {
                return Err(ToolError::middleware_rejected(
                    "high_risk_guard",
                    format!(
                        "路径安全检查拦截: 参数指向系统敏感目录 `{}`（风险等级: {}）",
                        bad,
                        level_to_str(level)
                    ),
                ));
            }

            Ok(())
        })
    }
}

/// 风险等级字符串（日志友好）。
fn level_to_str(level: RiskLevel) -> &'static str {
    match level {
        RiskLevel::Normal => "normal",
        RiskLevel::High => "high",
    }
}

/// 递归查找参数中指向敏感目录的路径字段值。
fn find_sensitive_path_arg(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if PATH_KEYS.contains(&key.as_str()) {
                    if let Some(s) = val.as_str() {
                        if is_sensitive_path(s) {
                            return Some(s.to_string());
                        }
                    }
                }
                if let Some(found) = find_sensitive_path_arg(val) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_sensitive_path_arg),
        _ => None,
    }
}

/// 系统敏感路径检查（与 analyze_image 执行器内双保险实现一致）。
///
/// ★ 防 junction/symlink 绕过（2026-08-12 修复）：先解析真实路径
///   （存在的祖先 canonicalize），与 risk.rs / sandbox 保持一致。
///   同时检查原始字符串（POSIX 风格标记在 Windows 规范化后可能失配）。
fn is_sensitive_path(p: &str) -> bool {
    let lower = p.to_lowercase();
    let resolved = super::sandbox::resolve_real_path(std::path::Path::new(p));
    let lower_resolved = resolved.to_string_lossy().to_lowercase();
    const MARKERS: &[&str] = &[
        "c:\\windows",
        "c:\\program files",
        "c:\\programdata",
        "/etc/",
        "/usr/",
        "/bin/",
        "/boot/",
        "/sys/",
        "/proc/",
        "/dev/",
        "\\.ssh",
        "/.ssh/",
    ];
    MARKERS.iter().any(|m| lower.contains(m) || lower_resolved.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::context::ToolContext;
    use crate::core::tool::dispatcher::ToolDispatcher;
    use crate::core::tool::registry::ToolRegistry;
    use crate::core::tool::result::ToolResult;
    use std::sync::Arc;

    fn guard() -> Arc<dyn Middleware> {
        Arc::new(HighRiskGuardMiddleware)
    }

    fn make_call(args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            tool_id: "builtin:x".into(),
            arguments: args,
            round: 1,
        }
    }

    fn sample_def() -> UnifiedToolDef {
        UnifiedToolDef::new("builtin", "x", "x", vec![]).unwrap()
    }

    /// 敏感路径参数被拦截。
    #[tokio::test]
    async fn blocks_sensitive_path_arg() {
        let g = guard();
        let err = g
            .before(&sample_def(), &make_call(serde_json::json!({ "image_path": "C:\\Windows\\System32\\a.png" })))
            .await
            .unwrap_err();
        assert!(err.message.contains("敏感目录"));
    }

    /// 嵌套路径也被拦截。
    #[tokio::test]
    async fn blocks_nested_sensitive_path() {
        let g = guard();
        let args = serde_json::json!({ "config": { "output_dir": "/etc/cron.d" } });
        let err = g.before(&sample_def(), &make_call(args)).await.unwrap_err();
        assert!(err.message.contains("敏感目录"));
    }

    /// 普通路径放行。
    #[tokio::test]
    async fn allows_normal_path() {
        let g = guard();
        let ok = g
            .before(&sample_def(), &make_call(serde_json::json!({ "image_path": "D:\\work\\photo.png" })))
            .await;
        assert!(ok.is_ok());
    }

    /// 无路径参数放行。
    #[tokio::test]
    async fn allows_no_path_args() {
        let g = guard();
        let ok = g
            .before(&sample_def(), &make_call(serde_json::json!({ "expression": "1+2" })))
            .await;
        assert!(ok.is_ok());
    }

    /// 集成：dispatcher 挂载中间件后，敏感路径调用被拒绝。
    #[tokio::test]
    async fn dispatcher_rejects_sensitive_path() {
        let registry = Arc::new(ToolRegistry::new());
        registry
            .register(
                sample_def(),
                Arc::new(|_a, _c| Box::pin(async { Ok(ToolResult::ok(serde_json::json!({}))) })),
            )
            .unwrap();
        let dispatcher = ToolDispatcher::new(registry.clone());
        dispatcher.add_middleware(guard());

        let err = dispatcher
            .dispatch(
                make_call(serde_json::json!({ "path": "/proc/self/mem" })),
                ToolContext::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.kind,
            crate::core::tool::error::ToolErrorKind::MiddlewareRejected
        );
    }
}
