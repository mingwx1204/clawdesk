//! 多模型路由 IPC 命令（主模型 / 视觉模型 / 绘图 API 配置，§八）。

use tauri::State;

use crate::commands::AppState;
use crate::llm::router::RouterStatus;

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
