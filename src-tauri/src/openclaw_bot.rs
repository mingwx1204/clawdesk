//! 内置 Bot HTTP 服务器
//! 使用 axum 提供轻量级 webhook 接收和 API 端点。
//! 无需外部服务——ClawDesk 自包含 Bot 引擎。

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command as StdCommand, Child, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use crate::error::AppError;

/// Bot 平台配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotPlatformConfig {
    pub enabled: bool,
    pub webhook_port: u16,
    pub bot_name: String,
    pub platforms: Vec<BotPlatform>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotPlatform {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub enabled: bool,
    pub connected: bool,
    pub config: HashMap<String, String>,
    pub description: String,
}

/// Webhook 收到的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookMessage {
    pub platform: String,
    pub from_user: String,
    pub content: String,
    pub msg_type: Option<String>,
    pub extra: Option<HashMap<String, String>>,
}

/// Bot 服务器状态
#[derive(Debug, Clone, Serialize)]
pub struct BotServerStatus {
    pub running: bool,
    pub port: u16,
    pub message_count: u64,
    pub platforms_connected: Vec<String>,
}

/// 全局 Bot 服务器状态
pub struct BotServerState {
    pub running: Arc<AtomicBool>,
    pub port: Arc<Mutex<u16>>,
    pub message_count: Arc<Mutex<u64>>,
    pub app_handle: Arc<Mutex<Option<AppHandle>>>,
    pub bridge_child: Arc<Mutex<Option<Child>>>,
}

impl Default for BotServerState {
    fn default() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            port: Arc::new(Mutex::new(19527)),
            message_count: Arc::new(Mutex::new(0)),
            app_handle: Arc::new(Mutex::new(None)),
            bridge_child: Arc::new(Mutex::new(None)),
        }
    }
}

/// axum 路由状态
#[derive(Clone)]
struct AppState {
    app_handle: AppHandle,
    message_count: Arc<Mutex<u64>>,
}

/// 获取当前时间戳（毫秒）
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// 启动内置 Bot HTTP 服务器
#[tauri::command]
pub async fn bot_server_start(
    state: tauri::State<'_, BotServerState>,
    app: AppHandle,
    config: BotPlatformConfig,
) -> Result<BotServerStatus, AppError> {
    if state.running.load(Ordering::SeqCst) {
        return Err(AppError::Other("Bot 服务器已在运行".into()));
    }

    let port = config.webhook_port;
    *state.port.lock() = port;
    *state.app_handle.lock() = Some(app.clone());

    // ── 服务器在独立 std 线程 + 独立 tokio runtime 中运行 ──
    // 路由语法必须用 axum 0.8 的 {capture}（v0.7 的 :param 会 panic）。
    // 线程隔离可避免与主 tauri runtime 的调度互相影响。
    let running_flag = state.running.clone();
    let msg_count = state.message_count.clone();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let app_state = AppState {
            app_handle: app2,
            message_count: msg_count,
        };
        let router: axum::Router<()> = Router::new()
            .route("/health", get(health_check))
            .route("/webhook/{platform}", post(handle_webhook))
            .route("/api/chat", post(handle_chat_api))
            .with_state(app_state);

        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[bot_server] tokio runtime 创建失败: {e}");
                return;
            }
        };
        rt.block_on(async move {
            let std_listener = match std::net::TcpListener::bind(format!("127.0.0.1:{}", port)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[bot_server] 端口 {port} 绑定失败: {e}");
                    return;
                }
            };
            if let Err(e) = std_listener.set_nonblocking(true) {
                eprintln!("[bot_server] 非阻塞设置失败: {e}");
                return;
            }
            let listener = match tokio::net::TcpListener::from_std(std_listener) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[bot_server] listener 转换失败: {e}");
                    return;
                }
            };
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    loop {
                        if !running_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    }
                })
                .await;
        });
    });

    state.running.store(true, Ordering::SeqCst);

    Ok(BotServerStatus {
        running: true,
        port,
        message_count: 0,
        platforms_connected: config.platforms.iter()
            .filter(|p| p.enabled)
            .map(|p| p.id.clone())
            .collect(),
    })
}

/// 停止 Bot 服务器（同时关闭微信桥接）
#[tauri::command]
pub async fn bot_server_stop(
    state: tauri::State<'_, BotServerState>,
) -> Result<(), AppError> {
    state.running.store(false, Ordering::SeqCst);
    // 杀死微信桥接子进程
    if let Some(mut child) = state.bridge_child.lock().take() {
        let _ = child.kill();
    }
    Ok(())
}

/// 获取 Bot 服务器状态
#[tauri::command]
pub async fn bot_server_status(
    state: tauri::State<'_, BotServerState>,
) -> Result<BotServerStatus, AppError> {
    Ok(BotServerStatus {
        running: state.running.load(Ordering::SeqCst),
        port: *state.port.lock(),
        message_count: *state.message_count.lock(),
        platforms_connected: vec![],
    })
}

/// 健康检查端点
async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "ClawDesk OpenClaw Bot Engine",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// Webhook 接收端点 — 外部平台推送消息
async fn handle_webhook(
    State(state): State<AppState>,
    axum::extract::Path(platform): axum::extract::Path<String>,
    Json(body): Json<WebhookMessage>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 增加消息计数
    {
        let mut count = state.message_count.lock();
        *count += 1;
    }

    // 推送消息到前端 UI
    let payload = serde_json::json!({
        "platform": platform,
        "fromUser": body.from_user,
        "content": body.content,
        "msgType": body.msg_type.unwrap_or_else(|| "text".into()),
        "timestamp": now_millis(),
    });

    let _ = state.app_handle.emit("bot-message", &payload);

    // 返回确认 — AI 回复将通过独立通道推送
    Ok(Json(serde_json::json!({
        "received": true,
        "message": "消息已接收，AI 正在处理中...",
    })))
}

/// REST API 直接调用 Chat
#[derive(Deserialize)]
struct ChatApiRequest {
    message: String,
    #[serde(default)]
    model: Option<String>,
}

async fn handle_chat_api(
    State(state): State<AppState>,
    Json(body): Json<ChatApiRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 推送聊天请求到前端，由前端 AI 引擎处理
    let payload = serde_json::json!({
        "platform": "http-api",
        "fromUser": "api",
        "content": body.message,
        "msgType": "text",
        "timestamp": now_millis(),
        "model": body.model,
    });

    let _ = state.app_handle.emit("bot-message", &payload);

    Ok(Json(serde_json::json!({
        "received": true,
        "message": "请求已提交 AI 处理",
    })))
}

/* ─── 微信桥接子进程管理 ─── */

/// 手动启动微信桥接
#[tauri::command]
pub async fn start_wechat_bridge(
    state: tauri::State<'_, BotServerState>,
    app: AppHandle,
) -> Result<(), AppError> {
    launch_wechat_bridge_inner(state.bridge_child.clone(), app).await
}

/// 停止微信桥接
#[tauri::command]
pub async fn stop_wechat_bridge(
    state: tauri::State<'_, BotServerState>,
) -> Result<(), AppError> {
    if let Some(mut child) = state.bridge_child.lock().take() {
        let _ = child.kill();
    }
    Ok(())
}

/// 检查微信桥接状态
#[tauri::command]
pub async fn wechat_bridge_status(
    state: tauri::State<'_, BotServerState>,
) -> Result<serde_json::Value, AppError> {
    let running = state.bridge_child.lock().is_some();
    Ok(serde_json::json!({ "running": running }))
}

/// 核心：启动微信桥接 Node.js 子进程
async fn launch_wechat_bridge_inner(
    bridge_child: Arc<Mutex<Option<Child>>>,
    app: AppHandle,
) -> Result<(), AppError> {
    if bridge_child.lock().is_some() {
        return Ok(());
    }

    // 查找 wechat-bridge 目录
    let bridge_dir = find_bridge_dir()?;
    let bridge_js = bridge_dir.join("bridge.js");

    if !bridge_js.exists() {
        return Err(AppError::Other("wechat-bridge/bridge.js 未找到".into()));
    }

    // 首次运行自动 npm install
    let node_modules = bridge_dir.join("node_modules");
    if !node_modules.exists() {
        let _ = app.emit("bot-bridge-status", serde_json::json!({
            "type": "status", "status": "installing", "message": "正在安装微信桥接依赖..."
        }));
        if let Ok(mut child) = StdCommand::new("npm")
            .args(["install", "--no-audit", "--no-fund"])
            .current_dir(&bridge_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let _ = child.wait();
        }
    }

    let _ = app.emit("bot-bridge-status", serde_json::json!({
        "type": "status", "status": "starting", "message": "正在启动微信桥接..."
    }));

    let mut child = StdCommand::new("node")
        .arg(&bridge_js)
        .current_dir(&bridge_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Other(format!("无法启动微信桥接: {}", e)))?;

    let stdout = child.stdout.take();
    let app2 = app.clone();

    if let Some(out) = stdout {
        tokio::spawn(async move {
            let reader = BufReader::new(out);
            for line in reader.lines() {
                if let Ok(line) = line {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&line) {
                        let _ = app2.emit("bot-bridge-status", &data);
                    }
                }
            }
        });
    }

    *bridge_child.lock() = Some(child);

    Ok(())
}

fn find_bridge_dir() -> Result<std::path::PathBuf, AppError> {
    // 开发模式：从 Cargo.toml 往上找
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dev_path = manifest.parent().unwrap().join("wechat-bridge");
    if dev_path.exists() {
        return Ok(dev_path);
    }
    // 生产模式：exe 同级目录
    let exe = std::env::current_dir().unwrap_or_default();
    let prod_path = exe.join("wechat-bridge");
    if prod_path.exists() {
        return Ok(prod_path);
    }
    Err(AppError::Other("wechat-bridge 目录未找到".into()))
}
