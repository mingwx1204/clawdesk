use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use super::context::ToolContext;
use super::def::UnifiedToolDef;
use super::error::ToolError;
use super::registry::ToolRegistry;
use super::result::ToolResult;

/// `Send` 异步 future 别名 —— 供 handler 与中间件签名使用。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 全局调度器单例：由 AppState 初始化，供 agent_subtask 等执行器获取
/// 带中间件链的调度器（执行器注册签名固定，无法注入引用）。
static GLOBAL_DISPATCHER: OnceLock<Arc<ToolDispatcher>> = OnceLock::new();

/// 初始化全局调度器（应用启动时调用一次；重复调用忽略）。
pub fn init_global(d: Arc<ToolDispatcher>) {
    let _ = GLOBAL_DISPATCHER.set(d);
}

/// 获取全局调度器（未初始化返回 None）。
pub fn global() -> Option<Arc<ToolDispatcher>> {
    GLOBAL_DISPATCHER.get().cloned()
}

/// 工具调用请求 —— 经 IPC 从前端进入调度器。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// 调用唯一 ID（前端生成，用于回执）。
    pub id: String,
    /// 工具 ID：`source:name`。
    pub tool_id: String,
    /// 调用参数（JSON 对象）。
    pub arguments: serde_json::Value,
    /// 当前工具循环轮次（从 1 开始），超过 `max_rounds` 熔断。
    pub round: usize,
}

/// 安全中间件 trait —— 阶段 2 挂载具体实现（高危确认 / 审计 / 限流）。
///
/// 契约：`before` 返回 `Ok(())` 放行，返回 `Err` 即拦截本次调用。
/// 此 trait 为异步设计，以承载阶段 2 的用户确认弹窗等等待操作。
pub trait Middleware: Send + Sync {
    fn name(&self) -> &'static str;

    fn before<'a>(
        &'a self,
        def: &'a UnifiedToolDef,
        call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<(), ToolError>>;
}

/// 工具调度器：查表 → 中间件链 → 执行；内置轮次熔断（默认 5 轮）。
///
/// 调度流程（DEV_SPEC.md §9）：
/// 1. 轮次熔断：`call.round > max_rounds` 直接拒绝；
/// 2. 查表：定义与处理器任一缺失即 `NotFound`；
/// 3. 中间件链：顺序执行，任一拦截即终止；
/// 4. 执行：写入上下文轮次后调用 handler。
pub struct ToolDispatcher {
    registry: Arc<ToolRegistry>,
    middleware: RwLock<Vec<Arc<dyn Middleware>>>,
    max_rounds: RwLock<usize>,
}

impl ToolDispatcher {
    /// 默认熔断阈值：5 轮（DEV_SPEC.md §9；可在设置 maxToolRounds 调整）。
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry,
            middleware: RwLock::new(Vec::new()),
            max_rounds: RwLock::new(5),
        }
    }

    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    pub fn max_rounds(&self) -> usize {
        *self.max_rounds.read().unwrap()
    }

    /// 配置熔断阈值（必须 > 0）。
    pub fn set_max_rounds(&self, n: usize) {
        assert!(n > 0, "max_rounds 必须大于 0");
        *self.max_rounds.write().unwrap() = n;
    }

    /// 挂载安全中间件（阶段 2 调用）。
    pub fn add_middleware(&self, m: Arc<dyn Middleware>) {
        self.middleware.write().unwrap().push(m);
    }

    pub fn clear_middleware(&self) {
        self.middleware.write().unwrap().clear();
    }

    /// 分发一次工具调用。
    pub async fn dispatch(
        &self,
        call: ToolCall,
        mut ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // 1) 轮次熔断
        let max_rounds = *self.max_rounds.read().unwrap();
        if call.round > max_rounds {
            return Err(ToolError::max_rounds(call.round, max_rounds));
        }

        // 2) 查表（定义 + 处理器）
        let def = self
            .registry
            .get(&call.tool_id)
            .ok_or_else(|| ToolError::not_found(&call.tool_id))?;
        let handler = self
            .registry
            .handler(&call.tool_id)
            .ok_or_else(|| ToolError::not_found(&call.tool_id))?;

        // 3) 中间件链（阶段 2 挂载安全中间件）
        // 注意：先克隆 Arc 列表再释放读锁，避免非 Send 的
        // `RwLockReadGuard` 跨 `.await` 存活导致 future 不满足 Send。
        let middleware: Vec<Arc<dyn Middleware>> = self.middleware.read().unwrap().clone();
        for m in middleware.iter() {
            m.before(&def, &call).await?;
        }

        // 4) 执行
        ctx.round = call.round;
        handler(call.arguments, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::error::ToolErrorKind;
    use crate::core::tool::registry::{ToolHandler, ToolRegistry};

    fn registry_with(def: UnifiedToolDef, handler: ToolHandler) -> Arc<ToolRegistry> {
        let reg = Arc::new(ToolRegistry::new());
        reg.register(def, handler).unwrap();
        reg
    }

    fn echo_handler() -> ToolHandler {
        Arc::new(|args, _ctx| Box::pin(async move { Ok(ToolResult::ok(args)) }))
    }

    fn echo_def() -> UnifiedToolDef {
        UnifiedToolDef::new("builtin", "echo", "回声", vec![]).unwrap()
    }

    fn call(tool_id: &str, round: usize) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            tool_id: tool_id.into(),
            arguments: serde_json::json!({"x": 1}),
            round,
        }
    }

    #[tokio::test]
    async fn dispatch_success() {
        let dispatcher = ToolDispatcher::new(registry_with(echo_def(), echo_handler()));
        let result = dispatcher
            .dispatch(call("builtin:echo", 1), ToolContext::default())
            .await
            .unwrap();
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn reject_not_found() {
        let dispatcher = ToolDispatcher::new(Arc::new(ToolRegistry::new()));
        let err = dispatcher
            .dispatch(call("builtin:nope", 1), ToolContext::default())
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::NotFound);
    }

    #[tokio::test]
    async fn reject_round_exceeded() {
        let dispatcher = ToolDispatcher::new(registry_with(echo_def(), echo_handler()));
        // 默认熔断 5 轮，round=6 必须被拒
        let err = dispatcher
            .dispatch(call("builtin:echo", 6), ToolContext::default())
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::MaxRoundsExceeded);
    }

    #[tokio::test]
    async fn round_5_is_allowed() {
        let dispatcher = ToolDispatcher::new(registry_with(echo_def(), echo_handler()));
        let result = dispatcher
            .dispatch(call("builtin:echo", 5), ToolContext::default())
            .await
            .unwrap();
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn middleware_can_block() {
        struct BlockAll;
        impl Middleware for BlockAll {
            fn name(&self) -> &'static str {
                "block-all"
            }
            fn before<'a>(
                &'a self,
                _def: &'a UnifiedToolDef,
                _call: &'a ToolCall,
            ) -> BoxFuture<'a, Result<(), ToolError>> {
                Box::pin(async { Err(ToolError::middleware_rejected("block-all", "测试拦截")) })
            }
        }

        let dispatcher = ToolDispatcher::new(registry_with(echo_def(), echo_handler()));
        dispatcher.add_middleware(Arc::new(BlockAll));
        let err = dispatcher
            .dispatch(call("builtin:echo", 1), ToolContext::default())
            .await
            .unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::MiddlewareRejected);
    }

    #[tokio::test]
    async fn context_round_is_written() {
        let handler: ToolHandler = Arc::new(|_args, ctx| {
            Box::pin(async move { Ok(ToolResult::ok(serde_json::json!({ "round": ctx.round }))) })
        });
        let dispatcher = ToolDispatcher::new(registry_with(echo_def(), handler));
        let result = dispatcher
            .dispatch(call("builtin:echo", 3), ToolContext::default())
            .await
            .unwrap();
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["round"], 3);
            }
            _ => panic!("期望 success"),
        }
    }
}

// ── 方案B追加：手动 Clone（引擎 DispatcherExecutor 需 Arc 重建调度器）──
impl Clone for ToolDispatcher {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            middleware: std::sync::RwLock::new(self.middleware.read().unwrap().clone()),
            max_rounds: RwLock::new(*self.max_rounds.read().unwrap()),
        }
    }
}
