//! `AuditMiddleware` —— 日志审计中间件。
//!
//! 所有工具调用均经过此中间件，记录 tool_id / call_id / round / 参数摘要。
//! 阶段 2 使用 `println!`（stdout），后续阶段可替换为文件 / tracing。

use crate::core::tool::def::UnifiedToolDef;
use crate::core::tool::dispatcher::{BoxFuture, Middleware, ToolCall};
use crate::core::tool::error::ToolError;

pub struct AuditMiddleware;

impl Middleware for AuditMiddleware {
    fn name(&self) -> &'static str {
        "audit"
    }

    fn before<'a>(
        &'a self,
        def: &'a UnifiedToolDef,
        call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            // 参数摘要：截断过长参数避免刷屏
            let args_str = call.arguments.to_string();
            // 按 char 边界截断（UTF-8 中文不能按字节切片，否则 panic）
            let args_preview = truncate_chars(&args_str, 200);

            // 三级日志体系（项目 12）：审计日志永久留存所有工具调用记录，
            // 不占用 LLM 上下文 token（独立文件，容量轮转）。
            crate::llm::logging::audit(
                "tool_call",
                &format!(
                    "tool={} call_id={} round={} risk={} args={}",
                    def.id, call.id, call.round, def.is_high_risk, args_preview
                ),
            );
            Ok(())
        })
    }
}

/// 按 char 边界截断字符串。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let head: String = chars[..max_chars].iter().collect();
        format!("{}…(+{}chars)", head, chars.len() - max_chars)
    }
}
