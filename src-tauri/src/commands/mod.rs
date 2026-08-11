//! IPC 命令层 —— 仅做薄壳转发，不包含业务逻辑（DEV_SPEC.md §4.4）。
//!
//! 前端经 `invoke("list_tools")` / `invoke("invoke_tool")` / `invoke("agent_chat")`
//! 与后端通信。本层只依赖 core 层与 llm/adapters 层，不感知具体工具实现。

// ── Edge TTS 朗读合成命令（神经网络拟人音色） ──
pub mod tts;

use std::path::Path;
use std::sync::{Arc, RwLock};

use tauri::State;
use tauri::Emitter;
use tauri::Manager;

use crate::core::tool::context::ToolContext;
use crate::core::tool::def::UnifiedToolDef;
use crate::core::tool::dispatcher::{ToolCall, ToolDispatcher};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;
use crate::executors;
use crate::adapters::mcp::{client::McpServerConfig, McpAdapter};
use crate::llm::progress::{AgentProgress, CancelRegistry};
use crate::llm::router::{ModelRouter, RouterStatus};
use crate::llm::runner::{run_agent_loop, ChatProvider, ToolLoopOutcome};
use crate::llm::session::SessionManager;
use crate::middleware::sensitive_guard::SensitiveFileGuardMiddleware;
use crate::llm::settings::SettingsStore;
use crate::llm::AgentMode;
use crate::middleware::sandbox::SandboxManager;

/// 应用级共享状态：注册表 + 调度器 + MCP 适配器 + Agent 会话 + 取消注册表。
pub struct AppState {
    pub registry: Arc<ToolRegistry>,
    pub dispatcher: Arc<ToolDispatcher>,
    pub mcp: Arc<McpAdapter>,
    /// Agent 多轮会话记忆（SQLite 持久化经 attach_db 附加）。
    pub sessions: Arc<SessionManager>,
    /// 运行中 Agent 任务的取消令牌 + 逐步确认通道。
    pub cancel_tokens: Arc<CancelRegistry>,
    /// Agent 权限模式（出厂默认 Off，YOLO 需用户手动开启）。
    pub agent_mode: Arc<RwLock<AgentMode>>,
    /// ReAct 最大迭代轮数（硬上限，默认 15）。
    pub max_rounds: Arc<RwLock<usize>>,
    /// 沙箱管理器：工具文件操作白名单（项目 3）。
    pub sandbox: Arc<SandboxManager>,
    /// 敏感文件保护守卫（.env / 密钥 / 凭据，默认开启，可运行时开关）。
    pub sensitive_guard: Arc<SensitiveFileGuardMiddleware>,
    /// 多模型路由（项目 4）：主模型 / 视觉模型 / 绘图 API 职责分离 + 降级。
    pub router: Arc<ModelRouter>,
    /// 应用设置（五大标签页配置，JSON 持久化，项目 7）。
    pub settings: Arc<SettingsStore>,
}

impl AppState {
    /// 创建应用状态，并注册全部内置工具与适配器内置技能。
    ///
    /// Agent 出厂默认关闭：`agent_mode = AgentMode::Off`（硬性约束）。
    pub fn new() -> Self {
        let registry = Arc::new(ToolRegistry::new());
        let dispatcher = Arc::new(ToolDispatcher::new(registry.clone()));
        let sessions = Arc::new(SessionManager::new());
        let sandbox = Arc::new(SandboxManager::new());
        let router = Arc::new(ModelRouter::new());
        let settings = Arc::new(SettingsStore::new());
        // 初始化全局路由单例：analyze_image / generate_image 执行器经
        // `llm::router::global()` 读取，未配置时工具自动降级。
        crate::llm::router::init_global(router.clone());
        executors::register_builtin_tools(&registry).expect("内置工具注册失败");
        crate::adapters::register_builtin_adapters(&registry).expect("适配器内置工具注册失败");
        let sensitive_guard = crate::middleware::register_all(&dispatcher, sandbox.clone());
        // 全局调度器单例：agent_subtask 等执行器据此获取带中间件链的调度器
        crate::core::tool::dispatcher::init_global(dispatcher.clone());
        Self::register_memory_search(&registry, &sessions).expect("memory_search 注册失败");
        Self {
            registry,
            dispatcher,
            mcp: Arc::new(McpAdapter::new()),
            sessions,
            cancel_tokens: Arc::new(CancelRegistry::new()),
            agent_mode: Arc::new(RwLock::new(AgentMode::Off)),
            max_rounds: Arc::new(RwLock::new(15)),
            sandbox,
            sensitive_guard,
            router,
            settings,
        }
    }

    /// 注册跨会话记忆检索工具（C1）。
    fn register_memory_search(
        registry: &Arc<ToolRegistry>,
        sessions: &Arc<SessionManager>,
    ) -> Result<(), ToolError> {
        let sessions_clone = sessions.clone();
        let def = UnifiedToolDef::new(
            "builtin",
            "memory_search",
            "跨会话关键词检索历史对话记忆（SQLite 本地记忆）",
            vec![crate::core::tool::def::ToolParamDef {
                name: "keyword".into(),
                param_type: "string".into(),
                description: "检索关键词".into(),
                required: true,
                enum_values: None,
                default: None,
            }],
        )?;
        let handler: ToolHandler = Arc::new(move |args, _ctx| {
            let sessions = sessions_clone.clone();
            Box::pin(async move {
                let kw = args
                    .get("keyword")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if kw.is_empty() {
                    return Ok(ToolResult::err("keyword 不能为空"));
                }
                let hits = sessions.search(kw);
                Ok(ToolResult::ok(serde_json::json!({
                    "keyword": kw,
                    "count": hits.len(),
                    "hits": hits,
                })))
            })
        });
        registry.register(def, handler)
    }

    /// 应用 setup 阶段附加会话持久化。
    pub fn init_sessions_persistence(&self, db_path: &Path) {
        match self.sessions.attach_db(db_path) {
            Ok(()) => eprintln!("[SESSION] SQLite 会话持久化已启用: {}", db_path.display()),
            Err(e) => eprintln!("[SESSION] 会话持久化初始化失败（继续内存模式）: {}", e),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// 列出全部已注册工具定义（动态注册表，无硬编码）。
#[tauri::command]
pub fn list_tools(state: State<'_, AppState>) -> Vec<UnifiedToolDef> {
    state.registry.list()
}

/// 分发一次工具调用。
#[tauri::command]
pub async fn invoke_tool(
    state: State<'_, AppState>,
    call: ToolCall,
) -> Result<ToolResult, ToolError> {
    state.dispatcher.dispatch(call, ToolContext::default()).await
}

/// 登记一个 MCP server 并立即连接注册其工具（配置持久化，重启自动恢复）。
#[tauri::command]
pub async fn mcp_add_server(
    state: State<'_, AppState>,
    config: McpServerConfig,
) -> Result<usize, ToolError> {
    state.mcp.add_server(config.clone())?;
    // 持久化到设置（保存后即时生效，重启自动恢复）
    {
        let mut cur = state.settings.get();
        if !cur.mcp_servers.iter().any(|s| s.name == config.name) {
            cur.mcp_servers.push(config.clone());
            let _ = state
                .settings
                .apply(serde_json::json!({ "mcpServers": cur.mcp_servers }));
        }
    }
    state.mcp.register_tools(&state.registry)
}

/// 列出已登记的 MCP server 配置。
#[tauri::command]
pub fn mcp_list_servers(state: State<'_, AppState>) -> Vec<McpServerConfig> {
    state.mcp.list_servers()
}

/// 移除一个 MCP server（断开连接、注销其注册的工具、持久化）。
#[tauri::command]
pub fn mcp_remove_server(state: State<'_, AppState>, name: String) -> bool {
    let ok = state.mcp.remove_server(&name);
    if ok {
        // 持久化
        let mut cur = state.settings.get();
        cur.mcp_servers.retain(|s| s.name != name);
        let _ = state
            .settings
            .apply(serde_json::json!({ "mcpServers": cur.mcp_servers }));
        // 注销该 server 注册的 mcp:* 工具
        let ids: Vec<String> = state
            .registry
            .list()
            .iter()
            .filter(|d| d.id.starts_with("mcp:"))
            .map(|d| d.id.clone())
            .collect();
        for id in ids {
            let _ = state.registry.unregister(&id);
        }
    }
    ok
}

/// 敏感文件保护开关（运行时切换 + 持久化到设置）。
#[tauri::command]
pub fn set_sensitive_guard(state: State<'_, AppState>, enabled: bool) -> bool {
    state.sensitive_guard.set_enabled(enabled);
    let _ = state
        .settings
        .apply(serde_json::json!({ "sensitiveFilesEnabled": enabled }));
    enabled
}

/// 查询敏感文件保护当前状态。
#[tauri::command]
pub fn get_sensitive_guard(state: State<'_, AppState>) -> bool {
    state.sensitive_guard.is_enabled()
}

/// 智能 Agent 对话：会话记忆 + 权限模式 + 进度事件 + 可取消 + 超时保护。
///
/// 安全契约：`api_key` 仅存在于本次调用的内存参数中，**不落盘、不打印**；
/// 模式与轮数取全局配置（agent_mode / max_rounds）。
#[tauri::command]
pub async fn agent_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    api_key: String,
    session_id: String,
    run_id: String,
    prompt: String,
    resume: bool,
    images: Option<Vec<String>>,
    thinking: Option<bool>,
    // 人设（system prompt 追加），微信自动回复时由前端传入对应槽位的人设
    persona: Option<String>,
) -> Result<ToolLoopOutcome, ToolError> {
    // 从持久化设置读取最新配置（项目 7）：Agent 开关 / 模式 / 轮数实时生效
    let s = state.settings.get();
    // ★ 思考模式：true 时主对话强制用推理模型（真实思考链）+ 引擎流式路径
    //   模型名按端点适配：OpenCode Go 用 deepseek-v4-pro（支持 reasoning），DeepSeek 官方用 deepseek-reasoner
    let thinking = thinking.unwrap_or(false);
    let is_opencode_go = s.model_endpoint.contains("opencode.ai");
    let engine_model = if thinking {
        if is_opencode_go {
            "deepseek-v4-pro".to_string()
        } else {
            "deepseek-reasoner".to_string()
        }
    } else {
        s.model.clone()
    };
    if s.agent_enabled && s.agent_mode != "off" {
        let mode_from_settings = crate::llm::AgentMode::from_str(&s.agent_mode);
        if !matches!(mode_from_settings, crate::llm::AgentMode::Off) {
            *state.agent_mode.write().unwrap() = mode_from_settings;
        }
    }
    if s.max_rounds >= 1 {
        *state.max_rounds.write().unwrap() = s.max_rounds.clamp(1, 50);
    }
    // ★ 工具循环熔断上限从设置读取（settings.maxToolRounds，可调 1~30）：
    //   同步到全局调度器（agent_subtask 等也走同一 dispatcher 熔断）
    {
        let n = s.max_tool_rounds.clamp(1, 30);
        if let Some(d) = crate::core::tool::dispatcher::global() {
            if d.max_rounds() != n {
                d.set_max_rounds(n);
                eprintln!("[CHAT] 工具循环熔断上限已同步: {n}");
            }
        }
    }
    // 主模型经路由层提供（项目 4）：同步 key（仅内存态）并复用 router 主客户端，
    // 用户通过 router_set_main_model 切换的模型（V4-Pro ↔ V4-Flash）实时生效。
    state.router.ensure_main_key(api_key.clone());
    // 同步 harness 引擎配置（仅内存态，Key 不落盘）：
    // StepConfirm / Yolo 模式走 harness 引擎，必须保证引擎已配置，否则报"引擎未配置"。
    // model 用设置中的模型；端点从设置 modelEndpoint 提取（支持 DeepSeek 官方 / OpenCode Go 等 OpenAI 兼容端点）。
    // 思考模式（thinking=true）则强制推理模型 → 返回真实 reasoning_content → 前端流式展示思考链。
    // 端点识别：OpenCode Go（opencode.ai）用 deepseek-v4-pro，其余（DeepSeek 官方等）用 deepseek-reasoner。
    let engine_base = endpoint_base_url(&s.model_endpoint);
    crate::harness::engine::config::set_engine_config(crate::harness::engine::config::EngineConfig {
        api_key: api_key.clone(),
        base_url: engine_base,
        model: engine_model.clone(),
        // Medium 思考力度：保留思考链（用户可折叠查看），同时避免 High 的过度思考
        // （High 时部分请求只输出 thinking 而 content 为空）。
        effort: crate::harness::engine::param::ReasoningEffort::Medium,
    });
    eprintln!("[CHAT] 发送任务: model={} thinking={} mode={:?} endpoint={}", engine_model, thinking, *state.agent_mode.read().unwrap(), s.model_endpoint);
    // 同步设置中配置的视觉 / 绘图 API Key（仅内存态）到路由层（项目 7）
    {
        let keys = state.settings.keys();
        if !keys.vision.is_empty() {
            state.router.configure_vision(keys.vision.clone(), &s.vision_model, &s.vision_endpoint);
        }
        if !keys.image.is_empty() {
            state.router.configure_image(keys.image.clone(), &s.image_model, &s.image_endpoint);
        }
    }
    let provider: Arc<dyn ChatProvider> = state.router.clone();

    let progress: crate::llm::progress::ProgressSink = Box::new(move |ev: &AgentProgress| {
        let _ = app.emit("agent://progress", ev);
    });

    let mode = *state.agent_mode.read().unwrap();
    // ★ 2026-08-12 修复：不再因思考模式静默把 Off/PlanOnly 临时切到 Yolo。
    //   thinking 只影响模型调用参数（engine_model 已在上方按 thinking 选择推理模型），
    //   不改变用户的权限模式 —— 否则"未开启 Agent"的用户在思考模式下工具会被静默放行，
    //   绕过 StepConfirm 高危确认（安全红线）。
    let max_rounds = *state.max_rounds.read().unwrap();
    let cancel = state.cancel_tokens.create(run_id.clone());

    // ★ 图片：主模型为文本模型（无法直接看 image_url），把用户上传的图片保存到本地
    //   附件目录，路径拼入 prompt → 模型调用 analyze_image / ocr 工具读取图片内容。
    let mut prompt_text = prompt;
    if let Some(imgs) = images {
        if !imgs.is_empty() {
            match save_user_images(&imgs) {
                Ok(paths) if !paths.is_empty() => {
                    let note = format!(
                        "\n\n[用户图片]\n{}\n以上图片已保存到本地磁盘，请调用 analyze_image 工具读取图片内容。",
                        paths
                            .iter()
                            .map(|p| format!("- {p}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    prompt_text.push_str(&note);
                    eprintln!("[CHAT] 已保存 {} 张用户图片: {:?}", paths.len(), paths);
                }
                Ok(_) => {}
                Err(e) => eprintln!("[CHAT] 保存用户图片失败: {e}"),
            }
        }
    }

    let result = run_agent_loop(
        &provider,
        &state.registry,
        &state.sandbox,
        &state.dispatcher,
        &state.sessions,
        &state.cancel_tokens,
        &session_id,
        &prompt_text,
        max_rounds,
        mode,
        resume,
        300, // 整体超时 300s
        &progress,
        &cancel,
        persona.as_deref(), // 微信人设注入（None 则无）
    )
    .await;

    state.cancel_tokens.remove(&run_id);
    state.cancel_tokens.clear_confirms();
    match &result {
        Ok(o) => eprintln!("[CHAT] 完成: rounds={} final_len={}", o.used_rounds, o.final_text.len()),
        Err(e) => eprintln!("[CHAT] 失败: {e}"),
    }
    result.map_err(|e| ToolError::execution(format!("Agent 运行失败: {}", e)))
}

/// 从完整模型端点 URL 提取引擎 base_url（LlmClient 会追加 `/v1/chat/completions`）。
/// - `https://opencode.ai/zen/go/v1/chat/completions` → `https://opencode.ai/zen/go`
/// - `https://api.deepseek.com/chat/completions` → `https://api.deepseek.com`
/// - 其余未知格式原样返回（作为 base_url 直接使用）。
pub(crate) fn endpoint_base_url(endpoint: &str) -> String {
    let mut e = endpoint.trim().trim_end_matches('/').to_string();
    if e.is_empty() {
        return "https://api.deepseek.com".to_string();
    }
    for suffix in [
        "/chat/completions",
        "/v1/chat/completions",
        "/messages",
        "/responses",
        "/completions",
    ] {
        if let Some(stripped) = e.strip_suffix(suffix) {
            e = stripped.trim_end_matches('/').to_string();
            break;
        }
    }
    // LlmClient 固定追加 /v1/chat/completions：若剩余以 /v1 结尾则去掉，避免 /v1/v1
    if e.ends_with("/v1") {
        e = e.trim_end_matches("/v1").trim_end_matches('/').to_string();
    }
    if e.is_empty() {
        "https://api.deepseek.com".to_string()
    } else {
        e
    }
}

/// 从完整端点 URL 提取 origin（scheme://host）。
/// - `https://api.deepseek.com/chat/completions` → `https://api.deepseek.com`
/// - `https://api.siliconflow.cn/images/generations` → `https://api.siliconflow.cn`
fn extract_origin(endpoint: &str) -> String {
    let e = endpoint.trim();
    if let Some(after_scheme) = e.strip_prefix("https://") {
        if let Some(host_end) = after_scheme.find('/') {
            format!("https://{}", &after_scheme[..host_end])
        } else {
            format!("https://{}", after_scheme)
        }
    } else if let Some(after_scheme) = e.strip_prefix("http://") {
        if let Some(host_end) = after_scheme.find('/') {
            format!("http://{}", &after_scheme[..host_end])
        } else {
            format!("http://{}", after_scheme)
        }
    } else {
        e.to_string()
    }
}

/// 从端点 host 名识别提供商（余额查询用）。
fn detect_provider(endpoint: &str) -> &'static str {
    let e = endpoint.trim().to_lowercase();
    if e.contains("opencode.ai") {
        "opencode-go"
    } else if e.contains("api.deepseek.com") {
        "deepseek"
    } else if e.contains("api.siliconflow.cn") {
        "siliconflow"
    } else if e.contains("api.z.ai") || e.contains("open.bigmodel.cn") {
        "zai"
    } else if e.contains("api.openai.com") {
        "openai"
    } else {
        "unknown"
    }
}

/// 解析 dataURL（`data:image/png;base64,xxx`）→ (mime, base64)。
/// 若输入已是纯 base64 则原样返回，mime 兜底为 image/png。
fn parse_image_data_url(data: &str) -> (String, String) {
    let body = data.trim();
    if let Some(rest) = body.strip_prefix("data:") {
        if let Some((meta, b64)) = rest.split_once(',') {
            if !b64.is_empty() {
                let mime = meta.split(';').next().unwrap_or("image/png").to_string();
                return (mime, b64.to_string());
            }
        }
    }
    ("image/png".to_string(), body.to_string())
}

/// 把用户上传的图片（dataURL 数组）保存到本地附件目录，返回绝对路径列表。
/// 复用 `attachment::attach_dir()`（`<系统临时目录>/clawdesk-attachments/`），
/// 单张上限 20MB；供模型用 analyze_image / ocr 工具读取内容。
fn save_user_images(images: &[String]) -> Result<Vec<String>, String> {
    use base64::Engine as _;

    let dir = crate::executors::builtin::attachment::attach_dir()?;
    let mut paths = Vec::new();
    for (i, img) in images.iter().enumerate() {
        if img.trim().is_empty() {
            continue;
        }
        let (mime, b64) = parse_image_data_url(img);
        let ext = match mime.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            "image/gif" => "gif",
            _ => "png",
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("图片 base64 解码失败: {e}"))?;
        if bytes.len() > 20 * 1024 * 1024 {
            return Err(format!(
                "图片过大（{}MB，上限 20MB）",
                bytes.len() / 1024 / 1024
            ));
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let path = dir.join(format!("user_image_{}_{}.{}", ts, i, ext));
        std::fs::write(&path, &bytes).map_err(|e| format!("保存图片失败: {e}"))?;
        paths.push(path.to_string_lossy().to_string());
    }
    Ok(paths)
}

/// 取消正在运行的 Agent 任务。
#[tauri::command]
pub fn agent_cancel(state: State<'_, AppState>, run_id: String) -> bool {
    state.cancel_tokens.cancel(&run_id)
}

/// 设置 Agent 权限模式（YOLO 需用户手动开启 —— 硬性约束）。
#[tauri::command]
pub fn agent_set_mode(state: State<'_, AppState>, mode: String) -> AgentMode {
    let new_mode = AgentMode::from_str(&mode);
    *state.agent_mode.write().unwrap() = new_mode;
    eprintln!("[AGENT] 权限模式已切换: {:?}", new_mode);
    new_mode
}

/// 读取当前 Agent 权限模式。
#[tauri::command]
pub fn agent_get_mode(state: State<'_, AppState>) -> AgentMode {
    *state.agent_mode.read().unwrap()
}

/// 设置 ReAct 最大迭代轮数（1..=50 钳制）。
#[tauri::command]
pub fn agent_set_max_rounds(state: State<'_, AppState>, rounds: usize) -> usize {
    let clamped = rounds.clamp(1, 50);
    *state.max_rounds.write().unwrap() = clamped;
    clamped
}

/// 读取当前最大迭代轮数。
#[tauri::command]
pub fn agent_get_max_rounds(state: State<'_, AppState>) -> usize {
    *state.max_rounds.read().unwrap()
}

/// 应答逐步确认（StepConfirm 模式）；返回是否找到对应调用。
/// ★ 2026-08-12：先查旧确认通道（CancelRegistry.confirms），查不到再查
///   PERMISSION_BRIDGE —— 引擎路径的 oneshot 注册在桥里（ID 与 ConfirmRequired 事件统一），
///   前端用 callId 应答两条路径都能命中。
#[tauri::command]
pub fn agent_confirm_call(state: State<'_, AppState>, call_id: String, approve: bool) -> bool {
    if state.cancel_tokens.resolve_confirm(&call_id, approve) {
        return true;
    }
    if let Some(bridge) = crate::harness::hooks::bridge::PERMISSION_BRIDGE.get() {
        return crate::harness::ENGINE_RT.block_on(bridge.resolve(&call_id, approve, None));
    }
    false
}

/// 列出全部 Agent 会话 ID。
#[tauri::command]
pub fn agent_sessions(state: State<'_, AppState>) -> Vec<String> {
    state.sessions.list()
}

/// 重命名会话（new_name 为空则清除自定义名，前端回退显示 id）。
#[tauri::command]
pub fn agent_session_rename(
    state: State<'_, AppState>,
    session_id: String,
    new_name: String,
) -> bool {
    let mut session = state.sessions.get_or_create(&session_id);
    let name = new_name.trim().to_string();
    session.name = if name.is_empty() { None } else { Some(name) };
    state.sessions.update(session);
    true
}

/// 返回全部会话的 id + 自定义名称（前端显示友好名）。
#[tauri::command]
pub fn agent_session_metas(state: State<'_, AppState>) -> Vec<serde_json::Value> {
    state
        .sessions
        .list()
        .into_iter()
        .map(|id| {
            let session = state.sessions.get_or_create(&id);
            serde_json::json!({ "id": id, "name": session.name })
        })
        .collect()
}

/// 加载指定会话的历史消息（供前端切换会话时恢复显示）。
/// 只返回 role + content（前端据此重建 ChatMsg；工具消息/思考链不落库故不回显）。
#[tauri::command]
pub fn agent_session_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Vec<serde_json::Value> {
    let session = state.sessions.get_or_create(&session_id);
    session
        .messages
        .iter()
        .map(|m| {
            serde_json::json!({
                "role": serde_json::to_value(&m.role).unwrap_or(serde_json::json!("")),
                "content": m.content,
            })
        })
        .collect()
}

/// 删除一个 Agent 会话。
#[tauri::command]
pub fn agent_session_delete(state: State<'_, AppState>, session_id: String) -> bool {
    state.sessions.delete(&session_id).is_some()
}

/// 当前会话的上下文占用详情（真实 token 统计 + 内容估算细分）。
///
/// 数据源：
/// - `windowTokens` / `pct`：runner 累计的「最近一次请求输入 token」→ 上下文窗口占用；
/// - `totalInput/Output/Tokens`：会话累计真实用量；
/// - `sys`：系统指令 + 工具定义 schema（估算）；
/// - `usr`：消息 / 工具输出 / 文件（按会话真实消息字符估算）。
#[tauri::command]
pub fn agent_session_usage(state: State<'_, AppState>, session_id: String) -> serde_json::Value {
    let s = state.sessions.get_or_create(&session_id);
    // 主模型上下文窗口（deepseek-v4 系列按 1M 估算，可在设置中调整）
    let window_limit: u64 = 1_000_000;
    // 一位小数百分比：1M 窗口下 <0.5% 的占用不再被取整成 0（例如系统指令 2200 token = 0.2%）
    let pct_of = |t: u64| -> f64 {
        if window_limit == 0 {
            0.0
        } else {
            ((t as f64 / window_limit as f64) * 1000.0).round() / 10.0
        }
    };
    let window_tokens = s.last_input_tokens.min(window_limit);

    // 内容细分估算（字符数 / 4 ≈ token），基于会话真实消息
    let mut msg_tokens: u64 = 0;
    let mut tool_tokens: u64 = 0;
    let mut file_tokens: u64 = 0;
    for m in &s.messages {
        let t = (m.content.chars().count() as u64) / 4;
        match m.role {
            crate::llm::Role::User => {
                if m.content.contains("[用户附件文件]") || m.content.contains("[用户图片]") {
                    file_tokens += t;
                } else {
                    msg_tokens += t;
                }
            }
            crate::llm::Role::Assistant => msg_tokens += t,
            crate::llm::Role::Tool => tool_tokens += t,
            crate::llm::Role::System => msg_tokens += t,
        }
    }

    // 系统指令固定 prompt 估算 + 工具定义 schema 估算（真实注册表）
    let sys_prompt_tokens: u64 = 2_200;
    let mut tool_defs_tokens: u64 = 0;
    for d in state.registry.list() {
        if let Ok(json) = serde_json::to_string(&d) {
            tool_defs_tokens += (json.chars().count() as u64) / 4;
        }
    }

    // 窗口占用：真实「最近一次请求输入 token」优先；重启后为 0 时用内容估算兜底，
    // 保证历史会话也能看到有意义的占用百分比。
    let sys_total = sys_prompt_tokens + tool_defs_tokens;
    let est_total = msg_tokens + tool_tokens + file_tokens + sys_total;
    let window_tokens = s.last_input_tokens.max(est_total).min(window_limit);

    serde_json::json!({
        "windowTokens": window_tokens,
        "windowLimit": window_limit,
        "pct": pct_of(window_tokens),
        "totalInput": s.total_input_tokens,
        "totalOutput": s.total_output_tokens,
        "totalTokens": s.total_input_tokens + s.total_output_tokens,
        "messages": s.messages.len(),
        "compactions": s.compacted_count,
        "sys": [pct_of(sys_prompt_tokens), pct_of(tool_defs_tokens)],
        "usr": [pct_of(msg_tokens), pct_of(tool_tokens), pct_of(file_tokens)],
    })
}

/// 导出会话为 Markdown 文件（对标大厂"导出对话"），返回保存路径。
#[tauri::command]
pub fn session_export(state: State<'_, AppState>, session_id: String) -> Result<String, String> {
    let s = state.sessions.get_or_create(&session_id);
    let mut md = String::new();
    md.push_str(&format!("# ClawDesk 对话导出\n\n- 会话 ID：`{session_id}`\n- 导出时间：{}\n\n---\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
    for m in &s.messages {
        let role = match m.role {
            crate::llm::Role::User => "👤 用户",
            crate::llm::Role::Assistant => "🤖 ClawDesk",
            crate::llm::Role::System => "⚙️ 系统",
            crate::llm::Role::Tool => "🔧 工具",
        };
        md.push_str(&format!("## {role}\n\n{}\n\n", m.content));
    }
    if s.messages.is_empty() {
        md.push_str("（空会话）\n");
    }
    let dir = crate::executors::builtin::attachment::attach_dir()?;
    let safe_id: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let path = dir.join(format!("export_{}_{}.md", chrono::Local::now().format("%Y%m%d_%H%M%S"), safe_id));
    std::fs::write(&path, md).map_err(|e| format!("导出写入失败: {e}"))?;
    eprintln!("[EXPORT] 会话 {session_id} 已导出 → {}", path.display());
    Ok(path.to_string_lossy().to_string())
}

/// 跨会话关键词搜索历史消息（对标大厂"搜索对话"），返回 { sessionId, role, content, time }。
#[tauri::command]
pub fn session_search(state: State<'_, AppState>, keyword: String) -> Vec<serde_json::Value> {
    if keyword.trim().is_empty() {
        return Vec::new();
    }
    state.sessions.search(keyword.trim())
}

/// 查询账户余额：根据端点 URL 自动识别提供商（DeepSeek / SiliconFlow / …），调用对应余额 API。
#[tauri::command]
pub async fn check_balance(api_key: String, endpoint: String) -> Result<serde_json::Value, String> {
    use std::time::Duration;
    let key = api_key.trim().to_string();
    let origin = extract_origin(&endpoint);
    let provider = detect_provider(&endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        // ★ IPv6 黑洞修复（2026-08-10）
        .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;

    match provider {
        "deepseek" => {
            let resp = client
                .get(format!("{}/user/balance", origin))
                .header("Authorization", format!("Bearer {}", key))
                .send()
                .await
                .map_err(|e| format!("查询余额失败（网络）: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(format!(
                    "查询余额失败 HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            serde_json::from_str(&text).map_err(|e| format!("解析余额响应失败: {e}"))
        }
        "siliconflow" => {
            let resp = client
                .get(format!("{}/v1/user/info", origin))
                .header("Authorization", format!("Bearer {}", key))
                .send()
                .await
                .map_err(|e| format!("查询余额失败（网络）: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(format!(
                    "查询余额失败 HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("解析余额响应失败: {e}"))?;
            // SiliconFlow 响应格式: { status: bool, data: { balance: string } }
            let balance = v
                .get("data")
                .and_then(|d| d.get("balance"))
                .and_then(|b| b.as_str())
                .unwrap_or("0");
            Ok(serde_json::json!({
                "is_available": true,
                "balance_infos": [{
                    "currency": "¥",
                    "total_balance": balance,
                    "granted_balance": "0",
                    "topped_up_balance": balance
                }]
            }))
        }
        "opencode-go" => {
            // OpenCode Go 无公开余额 API。发送一个最小 chat 请求验证 key 有效性：
            //  200 → key 有效 + 额度可用（消耗 ~1 token，几乎免费）
            //  429 → 已达使用限额（响应含 retry-after 重置时间）
            //  401 → key 无效
            let resp = client
                .post(format!("{}/zen/go/v1/chat/completions", origin))
                .header("Authorization", format!("Bearer {}", key))
                .json(&serde_json::json!({
                    "model": "deepseek-v4-flash",
                    "messages": [{ "role": "user", "content": "ping" }],
                    "max_tokens": 1
                }))
                .send()
                .await
                .map_err(|e| format!("查询余额失败（网络）: {e}"))?;
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                let hint = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| text.chars().take(200).collect::<String>());
                let reset = retry_after
                    .as_deref()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|secs| {
                        if secs >= 3600 {
                            format!("约 {} 小时后重置", secs / 3600)
                        } else {
                            format!("约 {} 分钟后重置", secs / 60 + 1)
                        }
                    })
                    .unwrap_or_else(|| "稍后重置".to_string())
                    .to_string();
                return Err(format!(
                    "已达 OpenCode Go 使用限额（{}）\n提示：{}（{reset}）",
                    status, hint
                ));
            }
            if !status.is_success() {
                return Err(format!(
                    "查询余额失败 HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            Ok(serde_json::json!({
                "is_available": true,
                "note": "OpenCode Go 订阅（每月 $10，含约 $60 使用额度）无公开余额 API，已通过最小请求验证 Key 有效且额度可用",
                "balance_infos": [{
                    "currency": "Go",
                    "total_balance": "Key 有效 · 额度可用",
                    "granted_balance": "0",
                    "topped_up_balance": "0"
                }]
            }))
        }
        "zai" | "openai" => Err(format!(
            "{} 官方不支持简单余额查询，请前往官网查看",
            if provider == "zai" { "智谱(Z.ai)" } else { "OpenAI" }
        )),
        other => Err(format!("暂不支持该提供商的余额查询（{}），请前往官网查看", other)),
    }
}

/// 查询 DeepSeek 账户余额（兼容旧接口，内部调用 check_balance）。
#[tauri::command]
pub async fn deepseek_balance(api_key: String) -> Result<serde_json::Value, String> {
    check_balance(api_key, "https://api.deepseek.com/chat/completions".to_string()).await
}

/// Fork 分支会话（§十二.2）：完整拷贝源会话记忆，返回新分支会话。
#[tauri::command]
pub fn agent_fork(state: State<'_, AppState>, source_id: String, new_id: String) -> Option<crate::llm::session::AgentSession> {
    let branch = state.sessions.fork(&source_id, new_id.clone())?;
    eprintln!("[FORK] {} -> {}", source_id, new_id);
    Some(branch)
}

/// 查询会话断点状态（§十二.1）：是否有可续跑的断点。
#[tauri::command]
pub fn agent_checkpoint(state: State<'_, AppState>, session_id: String) -> Option<crate::llm::session::AgentCheckpoint> {
    state.sessions.load_checkpoint(&session_id)
}

/// 列出指定父会话的全部分支会话 ID（侧边栏标注从属关系）。
#[tauri::command]
pub fn agent_branches(state: State<'_, AppState>, parent_id: String) -> Vec<String> {
    state.sessions.branches_of(&parent_id)
}

/// 列出当前沙箱授权根目录。
#[tauri::command]
pub fn sandbox_roots(state: State<'_, AppState>) -> Vec<String> {
    state.sandbox.roots()
}

/// 添加一个沙箱授权根目录；返回是否新增成功（重复添加返回 false）。
/// 成功后持久化到 settings.sandboxRoots（重启自动恢复）。
#[tauri::command]
pub fn sandbox_add_root(state: State<'_, AppState>, path: String) -> bool {
    let ok = state.sandbox.add_root(&path);
    if ok {
        let mut cur = state.settings.get();
        if !cur.sandbox_roots.contains(&path) {
            cur.sandbox_roots.push(path.clone());
            let _ = state
                .settings
                .apply(serde_json::json!({ "sandboxRoots": cur.sandbox_roots }));
        }
        eprintln!("[SANDBOX] 已添加授权根: {}", path);
    }
    ok
}

/// 移除一个沙箱授权根目录；返回是否移除成功。成功后同步持久化。
#[tauri::command]
pub fn sandbox_remove_root(state: State<'_, AppState>, path: String) -> bool {
    let ok = state.sandbox.remove_root(&path);
    if ok {
        let mut cur = state.settings.get();
        cur.sandbox_roots.retain(|r| r != &path);
        let _ = state
            .settings
            .apply(serde_json::json!({ "sandboxRoots": cur.sandbox_roots }));
        eprintln!("[SANDBOX] 已移除授权根: {}", path);
    }
    ok
}

/// 查询多模型路由状态（当前主模型 / 视觉模型 / 绘图 API 配置与最近故障）。
#[tauri::command]
pub fn router_status(state: State<'_, AppState>) -> RouterStatus {
    state.router.status()
}

/// 配置主模型（key 仅内存态；用于设置面板手动输入 DeepSeek key 的场景）。
#[tauri::command]
pub fn router_configure_main(
    state: State<'_, AppState>,
    api_key: String,
    model: String,
    endpoint: Option<String>,
) -> RouterStatus {
    let model = if model.trim().is_empty() {
        "deepseek-chat"
    } else {
        model.trim()
    };
    state
        .router
        .set_main(api_key, model, endpoint.as_deref());
    state.router.status()
}

/// 切换主模型（V4-Pro ↔ V4-Flash，§八.3）。
#[tauri::command]
pub fn router_set_main_model(state: State<'_, AppState>, model: String) -> RouterStatus {
    state.router.set_main_model(&model);
    eprintln!("[ROUTER] 主模型切换: {}", model);
    state.router.status()
}

/// 配置视觉专用模型（GLM-5V 等 OpenAI vision 兼容端点；key 仅内存态）。
#[tauri::command]
pub fn router_configure_vision(
    state: State<'_, AppState>,
    api_key: String,
    model: String,
    endpoint: String,
) -> RouterStatus {
    state.router.configure_vision(api_key, &model, &endpoint);
    eprintln!("[ROUTER] 视觉模型已配置: {} @ {}", model, endpoint);
    state.router.status()
}

/// 配置绘图 API（Flux / SD 系列 OpenAI images/generations 兼容端点；key 仅内存态）。
#[tauri::command]
pub fn router_configure_image(
    state: State<'_, AppState>,
    api_key: String,
    model: String,
    endpoint: String,
) -> RouterStatus {
    state.router.configure_image(api_key, &model, &endpoint);
    eprintln!("[ROUTER] 绘图 API 已配置: {} @ {}", model, endpoint);
    state.router.status()
}

/// 列出全部文件修改快照（快照回滚面板数据源，项目 6）。
#[tauri::command]
pub fn snapshot_list() -> Vec<crate::executors::builtin::snapshot::SnapEntry> {
    crate::executors::builtin::snapshot::list_snapshots()
}

/// 回滚指定快照（覆盖原文件）；返回回滚结果。
#[tauri::command]
pub fn snapshot_restore(snapshot_id: String) -> Result<serde_json::Value, String> {
    crate::executors::builtin::snapshot::restore_snapshot(&snapshot_id)
        .map_err(|e| format!("回滚失败: {}", e))
}

/// 删除指定快照（文件 + 索引项）；返回是否成功。
#[tauri::command]
pub fn snapshot_delete(snapshot_id: String) -> bool {
    crate::executors::builtin::snapshot::delete_snapshot(&snapshot_id).unwrap_or(false)
}

/// 对比快照与当前文件的差异（回滚前审查）。
#[tauri::command]
pub fn snapshot_diff(snapshot_id: String) -> Result<serde_json::Value, String> {
    crate::executors::builtin::snapshot::diff_snapshot(&snapshot_id)
        .map_err(|e| format!("对比失败: {}", e))
}

/// 读取完整应用设置（五大标签页配置，项目 7）。
#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> serde_json::Value {
    let mut v = serde_json::to_value(state.settings.get())
        .unwrap_or(serde_json::json!({}));
    // ★ 注入 opencode Key（实际存于 DPAPI keys.enc，前端照常读取）
    if let serde_json::Value::Object(ref mut map) = v {
        map.insert(
            "opencodeWatchApiKey".into(),
            serde_json::Value::String(state.settings.keys().opencode_watch),
        );
    }
    v
}

/// 读取运行时 API Key（仅内存态，不持久化）。用于前端在组件重载后恢复 Key，
/// 从而解锁输入框（输入框在 Key 为空时禁用，安全红线：Key 不落盘）。
#[tauri::command]
pub fn settings_get_keys(state: State<'_, AppState>) -> crate::llm::settings::ApiKeys {
    state.settings.keys()
}

/// 应用设置部分更新（仅更新提供的字段），立即持久化并返回最新设置。
#[tauri::command]
pub fn settings_set(
    state: State<'_, AppState>,
    patch: serde_json::Value,
) -> Result<crate::llm::settings::AppSettings, String> {
    // 提取运行时 API Key（仅内存态，不持久化）：mainKey / visionKey / imageKey
    let mut auto_start_val: Option<bool> = None;
    if let serde_json::Value::Object(map) = &patch {
        let mut keys = state.settings.keys();
        if let Some(v) = map.get("mainKey").and_then(|v| v.as_str()) { if !v.is_empty() { keys.main = v.to_string(); } }
        if let Some(v) = map.get("visionKey").and_then(|v| v.as_str()) { if !v.is_empty() { keys.vision = v.to_string(); } }
        if let Some(v) = map.get("imageKey").and_then(|v| v.as_str()) { if !v.is_empty() { keys.image = v.to_string(); } }
        // ★ opencode 回切 Key：同样只进内存 + DPAPI 加密存储（不落 settings.json 明文）
        if let Some(v) = map.get("opencodeWatchApiKey").and_then(|v| v.as_str()) {
            if !v.is_empty() { keys.opencode_watch = v.to_string(); }
        }
        state.settings.set_keys(keys);
        auto_start_val = map.get("autoStart").and_then(|v| v.as_bool());
    }
    let mut patch = patch;
    // ★ 从持久化补丁中剥离明文 Key（apply → save 不会写入 settings.json）
    if let serde_json::Value::Object(map) = &mut patch {
        map.remove("opencodeWatchApiKey");
    }
    let updated = state.settings.apply(patch)?;
    // ★ 开机自启动：设置变更时同步写入/删除注册表 Run 键
    if let Some(enabled) = auto_start_val {
        let _ = crate::llm::win_integration::autostart_set(enabled);
    }
    eprintln!("[SETTINGS] 设置已更新并持久化");
    Ok(updated)
}

/// 读取最近 N 行日志（三级日志体系查询：debug=调试 / audit=审计，项目 12）。
/// kind 支持 "debug" / "audit"（旧值 agent/engine/settings 兼容映射到 debug）。
#[tauri::command]
pub fn logs_tail(kind: String, lines: Option<usize>) -> Vec<String> {
    let kind = if kind.eq_ignore_ascii_case("audit") {
        crate::llm::logging::LogKind::Audit
    } else {
        crate::llm::logging::LogKind::Debug
    };
    crate::llm::logging::tail(kind, lines.unwrap_or(100).min(500))
}

/// 日志文件大小（字节，供自检/面板展示，项目 12）。
#[tauri::command]
pub fn logs_size(kind: String) -> u64 {
    let kind = if kind.eq_ignore_ascii_case("audit") {
        crate::llm::logging::LogKind::Audit
    } else {
        crate::llm::logging::LogKind::Debug
    };
    crate::llm::logging::size(kind)
}

/// 查询最近一次未捕获异常（全局异常捕获兜底，项目 13）。
/// 前端启动后轮询：有异常弹中文报错 + 自动取消任务。
#[tauri::command]
pub fn app_last_error() -> Option<serde_json::Value> {
    crate::llm::error_guard::last_error()
}

/// 执行启动健康自检（项目 14，§十三.4）：SQLite / MCP / API / 目录。
/// 前端启动时调用，失败项弹窗展示中文修复方案。
#[tauri::command]
pub fn self_check_run() -> serde_json::Value {
    let items = crate::llm::self_check::run_all();
    if crate::llm::self_check::has_failure(&items) {
        crate::llm::logging::debug("self_check", "启动自检存在失败项");
    }
    // 失败项写入调试日志
    for item in &items {
        if item.status == "fail" {
            crate::llm::logging::debug("self_check", &format!("{}: {}", item.name, item.detail));
        }
    }
    crate::llm::self_check::summary(&items)
}

/// 在 Windows 资源管理器中打开文件所在文件夹（项目 15，§十四.3）。
#[tauri::command]
pub fn win_open_in_explorer(path: String) -> Result<(), String> {
    crate::llm::win_integration::open_in_explorer(&path)
}

/// 写入系统剪贴板（项目 15，§十四.2）。
#[tauri::command]
pub fn win_clipboard_set(text: String) -> Result<(), String> {
    crate::llm::win_integration::clipboard_set(&text)
}

/// 读取系统剪贴板文本（项目 15，§十四.2）。
#[tauri::command]
pub fn win_clipboard_get() -> Result<String, String> {
    crate::llm::win_integration::clipboard_get()
}

/// 弹出 Windows 原生系统通知（项目 15，§十四.4）。
#[tauri::command]
pub fn win_notify(title: String, body: String) -> Result<(), String> {
    crate::llm::win_integration::notify(&title, &body)
}

/// 设置 / 取消开机自启（项目 15，§十四.1）。
#[tauri::command]
pub fn win_autostart(enabled: bool) -> Result<(), String> {
    crate::llm::win_integration::autostart_set(enabled)
}

/// 列出全部技能（source: skillhub，项目 16）。
/// 聚合「注册表中在册的」+「技能目录中存在的（含已禁用未注册的）」，
/// 保证禁用后的技能仍出现在列表中（enabled=false），可在 UI 上重新启用。
#[tauri::command]
pub fn skills_list(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Vec<serde_json::Value> {
    let disabled = state.settings.get().disabled_skills;
    let mut map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    // 注册表中在册的（含 builtin / 自进化生成等不在目录中的）
    for d in state.registry.list() {
        if d.id.starts_with("skillhub:") {
            map.insert(d.id, d.description);
        }
    }
    // 技能目录中存在的（覆盖描述；即使已被禁用卸载也能列出）
    if let Ok(dir) = app.path().app_data_dir() {
        for (id, desc) in crate::adapters::skillhub::list_skill_meta(&dir.join("skills")) {
            map.insert(id, desc);
        }
    }
    map.into_iter()
        .map(|(id, description)| {
            serde_json::json!({
                "id": id,
                "description": description,
                "enabled": !disabled.contains(&id),
            })
        })
        .collect()
}

/// 从设置应用禁用技能（启动 / 重扫 / 启用后调用）。
fn apply_disabled_skills(state: &AppState) {
    let disabled = state.settings.get().disabled_skills;
    for id in &disabled {
        if state.registry.unregister(id).is_some() {
            eprintln!("[SKILLHUB] 已禁用技能: {}", id);
        }
    }
}

/// 重新扫描用户技能目录：先卸载全部 skillhub 技能再重扫，
/// 干净反映新增 / 删除 / 启用状态（扫描 `app_data_dir/skills`）。
#[tauri::command]
pub fn skills_reload(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?
        .join("skills");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建技能目录失败: {e}"))?;
    for def in state.registry.list_by_source("skillhub") {
        let _ = state.registry.unregister(&def.id);
    }
    let n = crate::adapters::skillhub::register_from_dir(&state.registry, &dir)
        .map_err(|e| format!("技能目录扫描失败: {e}"))?;
    apply_disabled_skills(&state);
    eprintln!("[SKILLHUB] 技能目录重扫完成，注册 {} 个技能", n);
    Ok(n)
}

/// 启用 / 禁用技能（立即生效 + 持久化）：
/// 禁用 → 从注册表卸载；启用 → 重扫技能目录重新注册。
#[tauri::command]
pub fn skills_set_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    skill_id: String,
    enabled: bool,
) -> Result<bool, String> {
    let mut disabled = state.settings.get().disabled_skills;
    if enabled {
        disabled.retain(|x| x != &skill_id);
    } else if !disabled.contains(&skill_id) {
        disabled.push(skill_id.clone());
    }
    let _ = state.settings.apply(serde_json::json!({ "disabledSkills": disabled }))?;

    if enabled {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("获取应用数据目录失败: {e}"))?
            .join("skills");
        let _ = crate::adapters::skillhub::register_from_dir(&state.registry, &dir)
            .map_err(|e| format!("技能目录扫描失败: {e}"))?;
        apply_disabled_skills(&state);
    } else {
        state.registry.unregister(&skill_id);
    }
    eprintln!(
        "[SKILLHUB] 技能 `{}` 已{}",
        skill_id,
        if enabled { "启用" } else { "禁用" }
    );
    Ok(enabled)
}

/// 一键导出完整项目成果（项目 17，§十五.3）：快照/日志/图像/会话/报告打包 zip。
#[tauri::command]
pub fn export_all() -> Result<String, String> {
    crate::llm::export::export_all()
}


// ═══════════════════════════════════════════════════════════════
// 方案B追加：harness 引擎命令
// ═══════════════════════════════════════════════════════════════

#[tauri::command]
pub fn harness_set_model_config(api_key: String, base_url: String, effort: String, model: Option<String>) -> Result<serde_json::Value, String> {
    let effort_parsed = crate::harness::engine::param::ReasoningEffort::from_str(&effort);
    let cfg = crate::harness::engine::config::EngineConfig { api_key, base_url, model: model.unwrap_or_else(|| "deepseek-chat".to_string()), effort: effort_parsed };
    crate::harness::engine::config::set_engine_config(cfg);
    Ok(serde_json::json!({ "ok": true, "protocol": "HTTP/1.1 (SSE)" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_origin_strips_path() {
        assert_eq!(
            extract_origin("https://api.deepseek.com/chat/completions"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            extract_origin("https://api.siliconflow.cn/images/generations"),
            "https://api.siliconflow.cn"
        );
        assert_eq!(
            extract_origin("https://open.bigmodel.cn/api/paas/v4/chat/completions"),
            "https://open.bigmodel.cn"
        );
    }

    #[test]
    fn extract_origin_handles_bare_host_and_http() {
        assert_eq!(extract_origin("https://api.deepseek.com"), "https://api.deepseek.com");
        assert_eq!(extract_origin("http://127.0.0.1:8080/v1/chat/completions"), "http://127.0.0.1:8080");
        assert_eq!(extract_origin("   "), ""); // trim 后为空字符串
    }

    #[test]
    fn detect_provider_recognizes_known_hosts() {
        assert_eq!(detect_provider("https://opencode.ai/zen/go/v1/chat/completions"), "opencode-go");
        assert_eq!(detect_provider("https://api.deepseek.com/chat/completions"), "deepseek");
        assert_eq!(detect_provider("https://api.siliconflow.cn/images/generations"), "siliconflow");
        assert_eq!(detect_provider("https://api.z.ai/chat/completions"), "zai");
        assert_eq!(detect_provider("https://open.bigmodel.cn/api/paas/v4/chat/completions"), "zai");
        assert_eq!(detect_provider("https://api.openai.com/v1/chat/completions"), "openai");
        assert_eq!(detect_provider("https://my-custom.example.com/chat/completions"), "unknown");
    }
}

#[tauri::command]
pub fn harness_status() -> serde_json::Value {
    match crate::harness::engine::config::engine_config() {
        Some(cfg) => serde_json::json!({ "configured": true, "baseUrl": cfg.base_url, "model": cfg.model, "effort": cfg.effort.as_str() }),
        None => serde_json::json!({ "configured": false }),
    }
}

#[tauri::command]
pub async fn harness_start_task(app: tauri::AppHandle, state: State<'_, AppState>, api_key: String, base_url: String, effort: String, model: Option<String>, session_id: String, prompt: String) -> Result<serde_json::Value, String> {
    use crate::harness::core::turn_loop::{self, EngineEvent, ToolExecutor, TurnConfig, TurnResult};
    use crate::harness::engine::client::LlmClient;
    use crate::harness::engine::context::ContextManager;
    use crate::harness::engine::param::{ModelParams, ReasoningEffort};

    let cfg = crate::harness::engine::config::EngineConfig { api_key: api_key.clone(), base_url: base_url.clone(), model: model.unwrap_or_else(|| "deepseek-chat".to_string()), effort: ReasoningEffort::from_str(&effort) };
    crate::harness::engine::config::set_engine_config(cfg.clone());

    let client = LlmClient::new(api_key, base_url).map_err(|e| e.to_string())?;
    let ctx_mgr = Arc::new(ContextManager::new(Some(client.clone())));

    let run_id = format!("engine-{}", uuid::Uuid::new_v4());
    let cancel_flag = state.cancel_tokens.create(run_id.clone());
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_watch = cancel_flag.clone();
    let cancel_token_clone = cancel_token.clone();
    let watcher = tokio::spawn(async move { loop { if cancel_watch.is_cancelled() { cancel_token_clone.cancel(); break; } tokio::time::sleep(std::time::Duration::from_millis(100)).await; } });

    let registry = state.registry.clone();
    let dispatcher = state.dispatcher.clone();
    let agent_mode = *state.agent_mode.read().unwrap();

    let executor: Arc<dyn ToolExecutor> = Arc::new(crate::llm::runner::DispatcherExecutor {
        registry: registry.clone(), dispatcher, round: std::sync::atomic::AtomicUsize::new(0),
        confirm: if matches!(agent_mode, crate::llm::AgentMode::StepConfirm) { Some(crate::harness::hooks::bridge::make_step_confirm_callback(registry.clone(), None)) } else { None },
    });

    let session = state.sessions.get_or_create(&session_id);
    let mut messages: Vec<serde_json::Value> = session.messages.iter().map(|m| {
        let role = match m.role { crate::llm::Role::System=>"system", crate::llm::Role::User=>"user", crate::llm::Role::Assistant=>"assistant", crate::llm::Role::Tool=>"tool" };
        serde_json::json!({ "role": role, "content": m.content })
    }).collect();
    messages.push(serde_json::json!({ "role": "user", "content": prompt }));

    let (tx_event, mut rx_event) = tokio::sync::mpsc::channel::<EngineEvent>(256);
    let app_relay = app.clone();
    let relay = tokio::spawn(async move { while let Some(ev) = rx_event.recv().await {
        let payload = match ev {
            EngineEvent::TextDelta(t) => serde_json::json!({ "type":"text_delta","content":t }),
            EngineEvent::ThinkingDelta(t) => serde_json::json!({ "type":"thinking_delta","content":t }),
            EngineEvent::ToolCallStart{name} => serde_json::json!({ "type":"tool_start","name":name }),
            EngineEvent::ToolCallEnd{name,result,ok} => serde_json::json!({ "type":"tool_end","name":name,"result":result,"ok":ok }),
            EngineEvent::ConfirmRequired{call_id,tool_id,risk_level,arguments} => serde_json::json!({ "type":"confirm","callId":call_id,"toolId":tool_id,"riskLevel":risk_level,"arguments":arguments }),
            EngineEvent::Status(s) => serde_json::json!({ "type":"status","text":s }),
            EngineEvent::Error(e) => serde_json::json!({ "type":"error","message":e }),
            EngineEvent::Usage{input_tokens,output_tokens} => serde_json::json!({ "type":"usage","inputTokens":input_tokens,"outputTokens":output_tokens }),
            EngineEvent::TurnFinished{ok,error} => serde_json::json!({ "type":"turn_finished","ok":ok,"error":error }),
        }; let _ = app_relay.emit_to("main-window","engine://stream",payload);
    }});

    let params = ModelParams { model: cfg.model.clone(), reasoning_effort: cfg.effort, ..Default::default() };
    // 技能按需加载（方案 1）：固定保留 builtin/mcp，skillhub 技能按消息检索 top-N
    let tools_json = crate::llm::serialize_tools(&crate::llm::tool_selector::select_tools(
        &registry.list(),
        &prompt,
        crate::llm::tool_selector::DEFAULT_TOP_N,
    ));
    let turn_cfg = TurnConfig { params, system_prompt: Some(crate::llm::build_system_prompt()), max_tool_calls_per_turn: (*state.max_rounds.read().unwrap()).clamp(1, 50), tools: tools_json };
    let messages_arc = Arc::new(tokio::sync::RwLock::new(messages));
    let client_loop = client.clone(); let executor_loop = executor.clone(); let ctx_loop = ctx_mgr.clone(); let msgs_loop = messages_arc.clone(); let cancel_loop = cancel_token.clone(); let tx_loop = tx_event.clone();
    let result: Result<TurnResult, String> = crate::harness::ENGINE_RT.spawn(async move { turn_loop::run_turn_loop(client_loop,executor_loop,ctx_loop,msgs_loop,turn_cfg,cancel_loop,tx_loop).await.map_err(|e| e.to_string()) }).await.map_err(|e| format!("引擎任务失败: {e}"))?;
    // ★ 2026-08-12 修复：不再立即 abort relay（尾部事件丢失）。
    //   引擎退出时其 tx_event 被 drop → 通道关闭 → relay 的 recv() 返回 None 自然退出，
    //   期间会把通道里剩余事件（含 TurnFinished）全部转发完。这里 await 等它排空。
    let _ = watcher.abort();
    let _ = relay.await;
    state.cancel_tokens.remove(&run_id); state.cancel_tokens.clear_confirms();

    match result {
        Ok(turn) => {
            let mut session = state.sessions.get_or_create(&session_id);
            let updated = messages_arc.read().await;
            session.messages = updated.iter().map(|v| {
                let role = match v["role"].as_str().unwrap_or("user") { "system"=>crate::llm::Role::System, "assistant"=>crate::llm::Role::Assistant, "tool"=>crate::llm::Role::Tool, _=>crate::llm::Role::User };
                crate::llm::LlmMessage { role, content: v["content"].as_str().unwrap_or("").to_string(), tool_calls: None, tool_call_id: None }
            }).collect();
            state.sessions.update(session);
            Ok(serde_json::json!({ "ok":true, "finalText":turn.final_text, "usedToolCalls":turn.used_tool_calls, "usage":{ "inputTokens":turn.usage_input_tokens, "outputTokens":turn.usage_output_tokens } }))
        }
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub fn harness_stop_task(state: State<'_, AppState>, run_id: String) -> bool { state.cancel_tokens.cancel(&run_id) }

#[tauri::command]
pub async fn harness_respond_permission(request_id: String, approved: bool, note: Option<String>) -> bool {
    crate::harness::hooks::bridge::PERMISSION_BRIDGE.get().map(|b| crate::harness::ENGINE_RT.block_on(b.resolve(&request_id, approved, note))).unwrap_or(false)
}
