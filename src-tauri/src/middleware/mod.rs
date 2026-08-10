//! 安全中间件层 —— 阶段 2 起挂载（DEV_SPEC.md §9）。
//!
//! 分层契约：
//! - 中间件实现 `core::tool::dispatcher::Middleware` trait；
//! - 每个中间件一个文件，通过 `register_all` 挂载到 dispatcher；
//! - 本层不修改 core 层任何代码。

pub mod audit;
pub mod high_risk_guard;
pub mod risk;
pub mod sandbox;
pub mod sensitive_guard;

use std::sync::Arc;

use crate::core::tool::dispatcher::ToolDispatcher;

use self::sandbox::{SandboxManager, SandboxMiddleware};
use self::sensitive_guard::SensitiveFileGuardMiddleware;

/// 将全部安全中间件按顺序挂载到调度器，返回敏感文件守卫（供运行时开关）。
///
/// 链顺序（先注册先执行）：
/// 1. AuditMiddleware      — 日志审计（所有调用）
/// 2. HighRiskGuard         — 高危分级审计 + 敏感路径黑名单拦截
/// 3. SandboxMiddleware     — 沙箱白名单拦截（未授权目录拒绝访问）
/// 4. SensitiveFileGuard    — 敏感文件保护（.env / 密钥 / 凭据等）
pub fn register_all(
    dispatcher: &Arc<ToolDispatcher>,
    sandbox: Arc<SandboxManager>,
) -> Arc<SensitiveFileGuardMiddleware> {
    // 先审计，再黑名单，然后白名单，最后敏感文件：保证审计覆盖全部调用，
    // 高危路径先于沙箱判定（黑名单错误信息更精确）；敏感文件按文件名兜底。
    dispatcher.add_middleware(Arc::new(audit::AuditMiddleware));
    dispatcher.add_middleware(Arc::new(high_risk_guard::HighRiskGuardMiddleware));
    dispatcher.add_middleware(Arc::new(SandboxMiddleware::new(sandbox)));
    let guard = Arc::new(SensitiveFileGuardMiddleware::new());
    dispatcher.add_middleware(guard.clone());
    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::context::ToolContext;
    use crate::core::tool::def::UnifiedToolDef;
    use crate::core::tool::dispatcher::{ToolCall, ToolDispatcher};
    use crate::core::tool::registry::{ToolHandler, ToolRegistry};
    use crate::core::tool::result::ToolResult;
    use std::sync::Arc;

    fn echo_handler() -> ToolHandler {
        Arc::new(|args, _ctx| Box::pin(async move { Ok(ToolResult::ok(args)) }))
    }

    fn setup() -> (Arc<ToolRegistry>, Arc<ToolDispatcher>) {
        let registry = Arc::new(ToolRegistry::new());
        let dispatcher = Arc::new(ToolDispatcher::new(registry.clone()));
        // 注册普通 echo
        registry
            .register(
                UnifiedToolDef::new("builtin", "echo_safe", "安全 echo", vec![]).unwrap(),
                echo_handler(),
            )
            .unwrap();
        // 注册高危 echo
        registry
            .register(
                UnifiedToolDef::new("builtin", "echo_risky", "高危 echo", vec![])
                    .unwrap()
                    .high_risk(),
                echo_handler(),
            )
            .unwrap();
        (registry, dispatcher)
    }

    #[tokio::test]
    async fn middleware_chain_does_not_block_normal_calls() {
        let (_reg, dispatcher) = setup();
        register_all(&dispatcher, Arc::new(SandboxManager::new()));

        let result = dispatcher
            .dispatch(
                ToolCall {
                    id: "t1".into(),
                    tool_id: "builtin:echo_safe".into(),
                    arguments: serde_json::json!({"x": 1}),
                    round: 1,
                },
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn high_risk_guard_audits_but_does_not_block() {
        let (_reg, dispatcher) = setup();
        register_all(&dispatcher, Arc::new(SandboxManager::new()));

        // 高危调用应该通过（前端已确认，中间件不拦截）
        let result = dispatcher
            .dispatch(
                ToolCall {
                    id: "t2".into(),
                    tool_id: "builtin:echo_risky".into(),
                    arguments: serde_json::json!({"x": 1}),
                    round: 1,
                },
                ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(result.is_success());
    }
}
