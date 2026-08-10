//! ClawDesk 应用装配 —— 仅挂载模块与状态，不含业务逻辑。
//!
//! 说明：core / commands / executors / middleware / adapters / llm 在模块级声明。

mod adapters;
mod commands;
mod core;
mod executors;
mod llm;
mod middleware;
// ── 方案B追加：harness 移植模块 ──
mod harness;
// ── 微信 iLink Bot 接入（旧版移植） ──
mod wechat;
// ── 定时任务调度器 ──
mod scheduler;
// ── 猜人物游戏（真实 LLM 驱动） ──
mod guess;
// ── 自进化系统（AI 自动学习生成技能） ──
mod self_evolve;
// ── opencode 网关持续检测 + 自动回切 ──
mod opencode_watch;

use commands::AppState;
use std::sync::{Mutex, OnceLock};
use tauri::{Emitter, Manager};

/// 托盘图标句柄（退出时主动销毁，避免残留图标堆积在系统通知区域）。
static TRAY_HANDLE: OnceLock<Mutex<Option<tauri::tray::TrayIcon>>> = OnceLock::new();

/// Windows 下用 Win32 API 强制移除系统标题栏（WS_CAPTION / WS_SYSMENU）。
/// tauri 的 `set_decorations(false)` 在部分版本/环境下不真正清除窗口样式，
/// 直接修改窗口样式 + 刷新框架是最可靠的方式（已实测有效）。
#[cfg(target_os = "windows")]
fn force_undecorated_win32(win: &tauri::WebviewWindow) {
    extern "system" {
        fn GetWindowLongW(hWnd: isize, nIndex: i32) -> i32;
        fn SetWindowLongW(hWnd: isize, nIndex: i32, dwNewLong: i32) -> i32;
        fn SetWindowPos(
            hWnd: isize,
            hWndInsertAfter: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            uFlags: u32,
        ) -> i32;
    }
    if let Ok(hwnd) = win.hwnd() {
        // windows crate 的 HWND 是元组结构体，内部是指针
        let h = hwnd.0 as isize;
        const GWL_STYLE: i32 = -16;
        const WS_CAPTION: i32 = 0x00C0_0000;
        const WS_SYSMENU: i32 = 0x0008_0000;
        const SWP_FRAMECHANGED: u32 = 0x0020;
        const SWP_NOMOVE: u32 = 0x0002;
        const SWP_NOSIZE: u32 = 0x0001;
        const SWP_NOZORDER: u32 = 0x0004;
        unsafe {
            let style = GetWindowLongW(h, GWL_STYLE);
            SetWindowLongW(h, GWL_STYLE, style & !(WS_CAPTION | WS_SYSMENU));
            SetWindowPos(
                h,
                0,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
            eprintln!("[WINDOW] Win32 强制无边框完成 (style=0x{:X})", style);
        }
    } else {
        eprintln!("[WINDOW] 获取 hwnd 失败，无法强制无边框");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::new())
        .manage(wechat::WechatBotState::default())
        .manage(scheduler::SchedulerState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_tools,
            commands::invoke_tool,
            commands::mcp_add_server,
            commands::mcp_list_servers,
            commands::mcp_remove_server,
            commands::agent_chat,
            commands::agent_cancel,
            commands::agent_set_mode,
            commands::agent_get_mode,
            commands::agent_set_max_rounds,
            commands::agent_get_max_rounds,
            commands::agent_confirm_call,
            commands::agent_sessions,
            commands::agent_session_messages,
            commands::agent_session_rename,
            commands::agent_session_metas,
            commands::agent_session_delete,
            commands::sandbox_roots,
            commands::sandbox_add_root,
            commands::sandbox_remove_root,
            commands::set_sensitive_guard,
            commands::get_sensitive_guard,
            commands::router_status,
            commands::router_configure_main,
            commands::router_set_main_model,
            commands::router_configure_vision,
            commands::router_configure_image,
            commands::snapshot_list,
            commands::snapshot_restore,
            commands::snapshot_delete,
            commands::snapshot_diff,
            commands::settings_get,
            commands::settings_set,
            commands::settings_get_keys,
            commands::agent_fork,
            commands::agent_checkpoint,
            commands::agent_branches,
            commands::agent_session_usage,
            commands::check_balance,
            commands::deepseek_balance,
            commands::session_export,
            commands::session_search,
            commands::logs_tail,
            commands::logs_size,
            commands::app_last_error,
            commands::self_check_run,
            commands::win_open_in_explorer,
            commands::win_clipboard_set,
            commands::win_clipboard_get,
            commands::win_notify,
            commands::win_autostart,
            commands::skills_list,
            commands::skills_reload,
            commands::skills_set_enabled,
            commands::export_all,
            // ── 方案B追加：harness 引擎命令 ──
            commands::harness_set_model_config,
            commands::harness_start_task,
            commands::harness_stop_task,
            commands::harness_respond_permission,
            commands::harness_status,
            // ── 微信 iLink Bot 命令（旧版移植） ──
            wechat::wechat_get_qr,
            wechat::wechat_qr_status,
            wechat::wechat_verify_code,
            wechat::wechat_refresh_qr,
            wechat::wechat_logout,
            wechat::wechat_bot_start,
            wechat::wechat_bot_stop,
            wechat::wechat_bot_reply,
            wechat::wechat_send_message,
            wechat::wechat_send_image,
            wechat::wechat_bot_status,
            wechat::wechat_set_persona,
            wechat::wechat_set_proactive,
            wechat::wechat_history,
            wechat::mobile_qr_svg,
            // ── 定时任务 ──
            scheduler::scheduler_list,
            scheduler::scheduler_add,
            scheduler::scheduler_remove,
            scheduler::scheduler_set_enabled,
            scheduler::scheduler_trigger_now,
            scheduler::scheduler_status,
            // ── 猜人物游戏 ──
            guess::guess_start,
            guess::guess_reply,
            guess::guess_stop,
            // ── 自进化系统 ──
            self_evolve::self_evolve_enable,
            self_evolve::self_evolve_run,
            self_evolve::self_evolve_status,
            self_evolve::self_evolve_ranking,
        ])
        .setup(|app| {
            // ★ 自定义标题栏：强制无边框。
            //   tauri 的 set_decorations(false) 在 Windows 上可能未真正清除标题栏样式
            //   （已实测：样式仍含 WS_CAPTION），再用 Win32 直接清除，确保系统标题栏消失。
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_decorations(false);
                force_undecorated_win32(&win);
            }

            let state = app.state::<AppState>();

            // opencode 网关持续检测 + 自动回切（用户配置开关后后台运行）
            {
                let settings = state.settings.clone();
                opencode_watch::spawn(app.handle().clone(), settings);
            }

            // 敏感文件保护：启动时应用设置中的开关状态
            state
                .sensitive_guard
                .set_enabled(state.settings.get().sensitive_files_enabled);

            // ── 方案B追加：权限桥初始化（harness ↔ Vue 弹窗）──
            {
                use harness::hooks::bridge::{TauriPermissionBridge, PERMISSION_BRIDGE};
                let mut bridge = TauriPermissionBridge::new();
                let handle = app.handle().clone();
                bridge.set_emitter(Box::new(move |req: harness::hooks::bridge::PermissionRequest| {
                    let _ = handle.emit_to("main-window", "permission-request", req);
                }));
                let _ = PERMISSION_BRIDGE.set(std::sync::Arc::new(bridge));
            }

            // 全局异常捕获兜底（项目 13）：panic hook 捕获未处理异常，不闪退
            crate::llm::error_guard::install();

            // 技能加载（项目 16，§十五.1）：扫描用户技能目录自动注册，
            // 用户放入 `<数据目录>/skills/*.json` 或 `SKILL.md` 的技能文件自动进工具描述
            // ★ 目录与 skills_reload / skills_set_enabled 统一用 app_data_dir()/skills
            //   （%APPDATA%/com.clawdesk.app/skills，skillhub CLI 安装位置）；
            //   旧代码用 clawdesk_dir()/skills（D:\ClawDeskData\skills，为空）→ 重启后技能全丢。
            {
                let skills_dir = match app.path().app_data_dir() {
                    Ok(d) => d.join("skills"),
                    Err(e) => {
                        eprintln!("[SKILLHUB] 获取技能目录失败: {e}，回退到数据目录");
                        crate::llm::settings::clawdesk_dir().join("skills")
                    }
                };
                let _ = std::fs::create_dir_all(&skills_dir);
                match crate::adapters::skillhub::register_from_dir(&state.registry, &skills_dir) {
                    Ok(n) => eprintln!("[SKILLHUB] 用户技能目录扫描完成，注册 {} 个技能", n),
                    Err(e) => eprintln!("[SKILLHUB] 用户技能目录扫描失败: {}", e),
                }
                // 应用设置中禁用的技能（方案 3：技能管理）
                {
                    let disabled = state.settings.get().disabled_skills;
                    for id in &disabled {
                        let _ = state.registry.unregister(id);
                    }
                }
            }

            // 会话持久化：附加 SQLite（数据目录优先 D 盘）
            {
                let dir = crate::llm::settings::clawdesk_dir();
                let _ = std::fs::create_dir_all(&dir);
                state.init_sessions_persistence(&dir.join("sessions.db"));
            }

            // MCP 服务器从设置加载（重启自动恢复连接并注册远端工具）
            {
                let saved = state.settings.get().mcp_servers;
                let mut restored = 0usize;
                for cfg in saved {
                    match state.mcp.add_server(cfg) {
                        Ok(()) => restored += 1,
                        Err(e) => eprintln!("[MCP] 恢复 server 配置失败: {}", e),
                    }
                }
                if restored > 0 {
                    match state.mcp.register_tools(&state.registry) {
                        Ok(n) => eprintln!("[MCP] 已从设置恢复 {} 个 server，注册 {} 个工具", restored, n),
                        Err(e) => eprintln!("[MCP] 恢复注册失败: {}", e),
                    }
                }
            }

            // 沙箱根目录从设置恢复（重启后白名单不丢失）
            {
                let roots = state.settings.get().sandbox_roots;
                for r in roots {
                    let _ = state.sandbox.add_root(&r);
                }
                eprintln!(
                    "[SANDBOX] 已从设置恢复 {} 个授权根",
                    state.sandbox.roots().len()
                );
            }

            // 微信 iLink Bot：初始化数据目录 + 自动续连（全部已登录槽位后台恢复长轮询）
            {
                let wc = app.state::<wechat::WechatBotState>();
                wechat::init_data_dir(app.handle(), &wc);
                // 克隆全部槽位实例（与 app.state 共享同一批 Arc，操作同一份状态）
                let bots = wc.bots();
                let app_h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let state_ref = wechat::WechatBotState(parking_lot::Mutex::new(bots));
                    wechat::auto_resume(app_h, &state_ref).await;
                });
            }

            // 定时任务：初始化数据目录 + 启动调度循环（每 5 秒检查）
            {
                let sc = app.state::<scheduler::SchedulerState>();
                scheduler::init_data_dir(app.handle(), &sc);
                let app_h = app.handle().clone();
                let inner = sc.0.clone();
                tauri::async_runtime::spawn(async move {
                    scheduler::scheduler_loop(app_h, inner).await;
                });
            }

            // 窗口控制工具（窗口句柄 setup 阶段才可用）
            match crate::executors::builtin::window::register_window_tools(
                &state.registry,
                app.handle().clone(),
            ) {
                Ok(()) => eprintln!("[WINDOW] 窗口控制工具注册成功"),
                Err(e) => eprintln!("[WINDOW] 窗口控制工具注册失败: {}", e),
            }

            // ── 系统托盘：关闭窗口时隐藏到托盘，后台常驻（微信/定时任务不中断）──
            // ★ 修复：每个进程使用独立托盘 ID（含 PID），dev 热重载不会堆积重复图标
            //   旧进程被杀时 Windows 检测到 hWnd 无效后自动清理僵尸图标。
            // ★ 保险：启动时用 Win32 广播托盘刷新，强制清理历史残留的僵尸图标。
            {
                #[cfg(target_os = "windows")]
                unsafe {
                    // 向所有顶层窗口广播托盘刷新消息，触发系统清理失效的图标
                    extern "system" {
                        fn SendMessageTimeoutW(
                            hWnd: isize,
                            msg: u32,
                            wParam: usize,
                            lParam: usize,
                            flags: u32,
                            timeout: u32,
                            result: *mut usize,
                        ) -> usize;
                    }
                    const WM_NULL: u32 = 0x0000;
                    const HWND_BROADCAST: isize = 0xFFFF;
                    const SMTO_ABORTIFHUNG: u32 = 0x0002;
                    let mut result: usize = 0;
                    // 消息 0x01CE = TaskbarCreated，0x0600+ 自定义；这里直接发 WM_NULL 触发 explorer 重绘托盘
                    let _ = SendMessageTimeoutW(
                        HWND_BROADCAST,
                        WM_NULL,
                        0,
                        0,
                        SMTO_ABORTIFHUNG,
                        1000,
                        &mut result,
                    );
                }

                use tauri::menu::{Menu, MenuItem};
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

                let tray_id = format!("clawdesk-tray-{}", std::process::id());
                let show_i = MenuItem::with_id(app, "show", "显示 ClawDesk", true, None::<&str>)?;
                let quit_i = MenuItem::with_id(app, "quit", "退出 ClawDesk", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
                let tray = TrayIconBuilder::with_id(&tray_id)
                    .icon(app.default_window_icon().cloned().ok_or("缺少应用图标")?)
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .tooltip("ClawDesk - 桌面 AI 助手（后台运行中）")
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.unminimize();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => {
                            // ★ 退出前丢弃托盘图标（触发 NIM_DELETE），避免残留
                            if let Some(lock) = TRAY_HANDLE.get() {
                                if let Ok(mut guard) = lock.lock() {
                                    *guard = None; // drop → Shell_NotifyIcon(NIM_DELETE)
                                    eprintln!("[TRAY] 托盘图标已清理");
                                }
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            app.exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            let app = tray.app_handle();
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.unminimize();
                                let _ = w.set_focus();
                            }
                        }
                    })
                    .build(app)?;
                TRAY_HANDLE.get_or_init(|| Mutex::new(None))
                    .lock().map(|mut g| *g = Some(tray)).ok();
                eprintln!("[TRAY] 系统托盘已启用 tray_id={tray_id}（关闭窗口 = 后台常驻）");
            }
            Ok(())
        })
        // 关闭窗口 → 隐藏到托盘（后台常驻，微信自动回复 / 定时任务不中断）
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
