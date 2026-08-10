use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use super::context::ToolContext;
use super::def::UnifiedToolDef;
use super::dispatcher::BoxFuture;
use super::error::ToolError;
use super::result::ToolResult;

/// 工具处理器签名 —— 异步执行器。
///
/// 设计说明：handler 接收 `(参数, 上下文)` 并返回 `Result<ToolResult, ToolError>`。
/// 参数均为 owned（`serde_json::Value` / `ToolContext`），闭包内以
/// `Box::pin(async move { ... })` 捕获，future 为 `'static` —— 这一设计
/// 避免了 HRTB 生命周期绑定限制（E0582），同时足以承载阶段 3（OCR/生图）、
/// 阶段 4（MCP 远端调用）等异步长任务；执行器如需共享状态，通过 `Arc` 捕获。
pub type ToolHandler = Arc<
    dyn Fn(
            serde_json::Value,
            ToolContext,
        ) -> BoxFuture<'static, Result<ToolResult, ToolError>>
        + Send
        + Sync,
>;

/// 动态工具注册表：定义表 + 处理器表。
///
/// 契约（DEV_SPEC.md §7）：
/// - **无任何硬编码规则**：工具与来源全部运行时注册；
/// - 注册时强制校验 `id == source:name`，不合法直接拒绝；
/// - `sources()` / `list_by_source()` 动态枚举，供前端分组渲染。
#[derive(Default)]
pub struct ToolRegistry {
    defs: RwLock<HashMap<String, UnifiedToolDef>>,
    handlers: RwLock<HashMap<String, ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册工具：定义 + 处理器。
    ///
    /// 强制校验（DEV_SPEC.md §6）：
    /// - `id` 必须等于 `format!("{}:{}", source, name)`；
    /// - 重复注册拒绝（先查定义表，再插入，避免死锁）。
    pub fn register(&self, def: UnifiedToolDef, handler: ToolHandler) -> Result<(), ToolError> {
        def.validate_id()?;

        let mut defs = self.defs.write().unwrap();
        if defs.contains_key(&def.id) {
            return Err(ToolError::already_registered(&def.id));
        }

        // 两表写入顺序固定：先 handlers 后 defs，
        // 使并发读路径（list / handler）始终看到一致状态。
        self.handlers.write().unwrap().insert(def.id.clone(), handler);
        defs.insert(def.id.clone(), def);
        Ok(())
    }

    /// 注销工具（定义 + 处理器），返回被移除的定义。
    pub fn unregister(&self, id: &str) -> Option<UnifiedToolDef> {
        self.handlers.write().unwrap().remove(id);
        self.defs.write().unwrap().remove(id)
    }

    /// 按 ID 查询工具定义。
    pub fn get(&self, id: &str) -> Option<UnifiedToolDef> {
        self.defs.read().unwrap().get(id).cloned()
    }

    /// 按 ID 查询工具处理器。
    pub fn handler(&self, id: &str) -> Option<ToolHandler> {
        self.handlers.read().unwrap().get(id).cloned()
    }

    /// 列出全部工具定义（按 id 排序，保证前端渲染稳定）。
    pub fn list(&self) -> Vec<UnifiedToolDef> {
        let defs = self.defs.read().unwrap();
        let mut items: Vec<_> = defs.values().cloned().collect();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        items
    }

    /// 按来源列出工具定义。
    pub fn list_by_source(&self, source: &str) -> Vec<UnifiedToolDef> {
        self.list()
            .into_iter()
            .filter(|d| d.source == source)
            .collect()
    }

    /// 动态枚举所有已注册来源 —— 无硬编码枚举（DEV_SPEC.md §6）。
    pub fn sources(&self) -> Vec<String> {
        let defs = self.defs.read().unwrap();
        let mut set: HashSet<&str> = defs.values().map(|d| d.source.as_str()).collect();
        let mut list: Vec<String> = set.drain().map(String::from).collect();
        list.sort();
        list
    }

    pub fn len(&self) -> usize {
        self.defs.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::error::ToolErrorKind;

    /// 构造一个恒成功的哑处理器。
    fn dummy_handler() -> ToolHandler {
        Arc::new(|_args, _ctx| Box::pin(async { Ok(ToolResult::ok(serde_json::json!(null))) }))
    }

    fn sample_def(source: &str, name: &str) -> UnifiedToolDef {
        UnifiedToolDef::new(source, name, "测试工具", vec![]).unwrap()
    }

    #[test]
    fn register_and_get() {
        let reg = ToolRegistry::new();
        let def = sample_def("builtin", "echo");
        reg.register(def.clone(), dummy_handler()).unwrap();

        assert_eq!(reg.get("builtin:echo").unwrap().id, "builtin:echo");
        assert!(reg.handler("builtin:echo").is_some());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn reject_malformed_id() {
        let reg = ToolRegistry::new();
        let mut def = sample_def("builtin", "echo");
        def.id = "wrong-id".into(); // 手工篡改，模拟绕过构造器
        let err = reg.register(def, dummy_handler()).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::InvalidDef);
        assert!(reg.is_empty());
    }

    #[test]
    fn reject_duplicate_registration() {
        let reg = ToolRegistry::new();
        let def = sample_def("builtin", "echo");
        reg.register(def.clone(), dummy_handler()).unwrap();
        let err = reg.register(def, dummy_handler()).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::AlreadyRegistered);
    }

    #[test]
    fn reject_name_containing_colon() {
        let err = UnifiedToolDef::new("builtin", "a:b", "非法名称", vec![]).unwrap_err();
        assert_eq!(err.kind, ToolErrorKind::InvalidDef);
    }

    #[test]
    fn sources_are_dynamic() {
        let reg = ToolRegistry::new();
        reg.register(sample_def("builtin", "a"), dummy_handler()).unwrap();
        reg.register(sample_def("mcp", "b"), dummy_handler()).unwrap();
        reg.register(sample_def("skillhub", "c"), dummy_handler()).unwrap();

        assert_eq!(
            reg.sources(),
            vec!["builtin".to_string(), "mcp".to_string(), "skillhub".to_string()]
        );
        assert_eq!(reg.list_by_source("mcp").len(), 1);
    }

    #[test]
    fn unregister_removes_both_tables() {
        let reg = ToolRegistry::new();
        let def = sample_def("builtin", "echo");
        reg.register(def.clone(), dummy_handler()).unwrap();

        let removed = reg.unregister("builtin:echo").unwrap();
        assert_eq!(removed.id, "builtin:echo");
        assert!(reg.get("builtin:echo").is_none());
        assert!(reg.handler("builtin:echo").is_none());
        assert!(reg.is_empty());
    }

    #[test]
    fn list_is_sorted_by_id() {
        let reg = ToolRegistry::new();
        reg.register(sample_def("builtin", "z"), dummy_handler()).unwrap();
        reg.register(sample_def("builtin", "a"), dummy_handler()).unwrap();
        let ids: Vec<String> = reg.list().into_iter().map(|d| d.id).collect();
        assert_eq!(ids, vec!["builtin:a".to_string(), "builtin:z".to_string()]);
    }
}

// ── 方案B追加：手动 Clone（引擎 DispatcherExecutor 需 Arc 重建注册表）──
impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            defs: std::sync::RwLock::new(self.defs.read().unwrap().clone()),
            handlers: std::sync::RwLock::new(self.handlers.read().unwrap().clone()),
        }
    }
}
