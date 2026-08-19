//! IPC 命令层 —— 仅做薄壳转发，不包含业务逻辑（DEV_SPEC.md §4.4）。
//!
//! 前端经 `invoke("list_tools")` / `invoke("invoke_tool")` / `invoke("agent_chat")`
//! 与后端通信。本层只依赖 core 层与 llm/adapters 层，不感知具体工具实现。

// ── Edge TTS 朗读合成命令（神经网络拟人音色） ──
pub mod tts;
// ── 余额查询 & 模型列表探测 ──
pub mod balance;
// ── 会话管理（列表/消息/导出/搜索/分支/断点） ──
pub mod session_cmd;
// ── 文件快照 / 沙箱授权 / 多模型路由 ──
pub mod snapshot;
pub mod sandbox;
pub mod router_cmd;
// ── 日志查询 / 健康自检 / Windows 集成 / 一键导出 ──
pub mod log_cmd;
pub mod win_cmd;
pub mod export_cmd;
// ── 技能管理（skillhub） ──
pub mod skill_cmd;
// ── 方案B追加：harness 引擎命令 ──
pub mod harness_cmd;

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
use serde_json::json;
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
    let thinking = thinking.unwrap_or(true); // ★ 思考模式出厂默认开启
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

/// 《人是怎么样的》参考摘录（情境化）：按当前时段 + AI 心情从书里选「人性条目」的
/// 一句话定义 + 对话实例，供 AI 主动聊天/托管回复时理解"人"，更有活人感。
/// - 时段加权：深夜优先深夜语系（失眠/深夜的脆弱/关灯后的天花板…），白天均匀
/// - 心情加权：想念时优先想念/吃醋语系，低落时优先心之语系
/// - 随机补足：其余条目随机挑，保证每次参考常新
fn human_book_ref(max_entries: usize, max_chars: usize) -> String {
    // ★ 书路径可配置（settings.humanBookDir），搬家不丢引用
    let dir = {
        let s = crate::llm::settings::SettingsStore::new();
        let base = s.get().human_book_dir;
        std::path::PathBuf::from(base).join("条目")
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return String::new();
    };
    let mut files: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "md").unwrap_or(false))
        .collect();
    files.sort();
    if files.is_empty() {
        return String::new();
    }

    // ── 情境加权：给每条算一个"当前情境相关度"分数 ──
    use chrono::Timelike;
    let hour = chrono::Local::now().hour();
    let night = hour >= 23 || hour < 6;
    // 当前心情快照（想念/孤独/愉悦）
    let (longing, lonely, joy) = {
        let m = crate::mood::mood_snapshot();
        (m.longing, m.loneliness, m.joy)
    };

    // 情境关键词表（文件名/标题里出现即加分）
    let night_kws = ["深夜", "失眠", "关灯", "凌晨", "熬夜", "夜晚", "睡不着", "夜"];
    let longing_kws = ["想念", "吃醋", "暧昧", "暗恋", "异地", "心动", "网恋", "单恋", "思念"];
    let lonely_kws = ["孤独", "寂寞", "一个人的", "空荡", "冷落", "已读不回", "人群", "独处"];
    let low_kws = ["低落", "委屈", "难过", "疲惫", "心累", "崩溃", "焦虑", "失落", "失望", "不甘", "哭泣", "眼泪"];
    let joy_kws = ["开心", "喜悦", "甜蜜", "心动", "幸福", "心流", "热恋", "快乐"];

    let score = |name: &str| -> u32 {
        let mut s = 0u32;
        if night {
            for kw in night_kws {
                if name.contains(kw) { s += 3; break; }
            }
        }
        if longing >= 0.55 {
            for kw in longing_kws {
                if name.contains(kw) { s += 2; break; }
            }
        }
        if lonely >= 0.55 {
            for kw in lonely_kws {
                if name.contains(kw) { s += 2; break; }
            }
        }
        if joy <= 0.35 {
            for kw in low_kws {
                if name.contains(kw) { s += 2; break; }
            }
        } else if joy >= 0.75 {
            for kw in joy_kws {
                if name.contains(kw) { s += 2; break; }
            }
        }
        s
    };

    // 按相关度降序（情境条目优先）；同分用随机键打散保持常新。
    // ★ 不能用随机比较器（sort_by 要求全序，随机比较器 = 未定义行为），
    //   改为"分数降序 + 随机键升序"的稳定排序。
    let mut scored: Vec<(u32, u64, std::path::PathBuf)> = files
        .into_iter()
        .map(|p| {
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            (score(&name), rand_key(), p)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    // 前 max_entries 条（情境相关 + 随机余量混合，既贴情境又常新）
    let picked: Vec<std::path::PathBuf> = scored.into_iter().take(max_entries).map(|(_, _, p)| p).collect();

    let mut out = String::new();
    for p in picked {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        // 提取「一句话定义」与「对话实例」两节
        let mut one_liner = String::new();
        let mut dialog = String::new();
        let mut section = "";
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with("## ") {
                section = l.trim_start_matches("## ").trim();
                continue;
            }
            if section.starts_with("①") || section.contains("一句话定义") {
                if !l.is_empty() && !l.starts_with('#') {
                    one_liner.push_str(l);
                    one_liner.push('\n');
                }
            } else if section.starts_with("④") || section.contains("对话实例") {
                if !l.is_empty() && !l.starts_with('#') {
                    dialog.push_str(l);
                    dialog.push('\n');
                }
            }
            if one_liner.chars().count() > 200 || dialog.chars().count() > 300 {
                break;
            }
        }
        let entry = format!(
            "【{}】{}\n{}\n",
            name,
            one_liner.trim(),
            dialog.trim()
        );
        if out.chars().count() + entry.chars().count() > max_chars {
            break;
        }
        out.push_str(&entry);
        out.push('\n');
    }
    out
}

/// 随机键（打散同分条目，保持每次参考不同；作为稳定排序的次键）。
fn rand_key() -> u64 {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64;
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

/// 读取完整应用设置（五大标签页配置，项目 7）。
#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> serde_json::Value {
    serde_json::to_value(state.settings.get())
        .unwrap_or(serde_json::json!({}))
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
        state.settings.set_keys(keys);
        auto_start_val = map.get("autoStart").and_then(|v| v.as_bool());
    }
    let updated = state.settings.apply(patch)?;
    // ★ 开机自启动：设置变更时同步写入/删除注册表 Run 键
    if let Some(enabled) = auto_start_val {
        let _ = crate::llm::win_integration::autostart_set(enabled);
    }
    eprintln!("[SETTINGS] 设置已更新并持久化");
    Ok(updated)
}


