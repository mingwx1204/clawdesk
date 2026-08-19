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
// ── AI 生活状态模拟器（世界线：吃饭/洗澡/打游戏/睡觉…） ──
mod living_state;
// ── AI 情绪引擎（心：想念/孤独/深夜情绪放大…） ──
mod mood;
// ── 细节记忆库（被看见：记住主人随口提过的事） ──
mod detail_memory;
// ── 自进化系统（AI 自动学习生成技能） ──
mod self_evolve;
// ── 生命叙事（睡前巩固 + 梦境 · 一生的故事线） ──
mod life_narrative;
// ── 主观关系叙事（我们之间的故事 · 关系记忆） ──
mod relationship;
// ── 驱动力层（内在动机 · 人格涌现的源头，借鉴 OpenHer） ──
mod drives;
// ── 人格底座（OCEAN 大五人格 · 心理学锚点，借鉴 character-sim） ──
mod persona_traits;
// ── Ghost 机制（可「已读不回」· 她有自己的状态，借鉴 eros-engine） ──
mod ghost;
// ── 好感度模型（六维 affinity · 关系从叙事升级为数值，借鉴 eros-engine） ──
mod affinity;

use commands::AppState;
use std::sync::{Mutex, OnceLock};
use tauri::{Emitter, Manager};

/// 托盘图标句柄（退出时主动销毁，避免残留图标堆积在系统通知区域）。
static TRAY_HANDLE: OnceLock<Mutex<Option<tauri::tray::TrayIcon>>> = OnceLock::new();

/// 销毁托盘图标（触发 Shell_NotifyIcon(NIM_DELETE)）。任何退出路径都调用：
/// 菜单"退出"、RunEvent::Exit（窗口关闭/系统注销/app.exit 等）。
fn destroy_tray() {
    if let Some(lock) = TRAY_HANDLE.get() {
        if let Ok(mut guard) = lock.lock() {
            *guard = None; // drop → NIM_DELETE
            eprintln!("[TRAY] 托盘图标已清理");
        }
    }
}

/// 启动时清理历史残留的 ClawDesk 托盘图标。
///
/// 背景：进程被强制结束（任务管理器 / Stop-Process -Force）时，explorer 偶尔
/// 不会自动移除该进程的托盘图标，导致托盘区堆积大量重复图标（实测出现过
/// 十几个"齿轮"图标）。本项目反复强杀 dev 进程时会触发。
///
/// 原理：枚举任务栏 Shell_TrayWnd → TrayNotifyWarn/TrayNotify 容器 → 其中的
/// ToolbarWindow32（托盘按钮列表），逐个读取按钮 tooltip（TB_GETBUTTONTEXTW），
/// 匹配 "ClawDesk" 的用 TB_DELETEBUTTON 删除。删除后索引重排，需原地重试。
///
/// 尽力而为：任何一步失败都静默跳过，不影响应用启动。
#[cfg(target_os = "windows")]
fn cleanup_stale_tray_icons() {
    unsafe {
        extern "system" {
            fn FindWindowW(lpClassName: *const u16, lpWindowName: *const u16) -> isize;
            fn FindWindowExW(
                hWndParent: isize,
                hWndChildAfter: isize,
                lpszClass: *const u16,
                lpszWindow: *const u16,
            ) -> isize;
            fn SendMessageW(hWnd: isize, msg: u32, wParam: usize, lParam: isize) -> isize;
        }
        fn w(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(std::iter::once(0)).collect()
        }

        // Toolbar 控件消息（托盘按钮列表是标准 Toolbar）
        const TB_BUTTONCOUNT: u32 = 0x0418;
        const TB_GETBUTTON: u32 = 0x0417;
        const TB_GETBUTTONTEXTW: u32 = 0x045D;
        const TB_DELETEBUTTON: u32 = 0x0416;
        #[repr(C)]
        #[allow(dead_code)]
        #[allow(non_snake_case)] // Win32 结构体字段命名（iBitmap/idCommand/fsState/fsStyle/bReserved/dwData/iString）
        struct TBBUTTON {
            iBitmap: i32,
            idCommand: i32,
            fsState: u8,
            fsStyle: u8,
            bReserved: [u8; 2],
            dwData: usize,
            iString: isize,
        }

        let shell_tray = FindWindowW(w("Shell_TrayWnd").as_ptr(), std::ptr::null());
        if shell_tray == 0 {
            return;
        }
        let mut removed = 0usize;
        // Windows 10/11 托盘容器类名（11 上通常为 TrayNotifyWarn）
        for container_cls in ["TrayNotifyWarn", "TrayNotify"] {
            let mut child = 0isize;
            loop {
                let c = w(container_cls);
                child = FindWindowExW(shell_tray, child, c.as_ptr(), std::ptr::null());
                if child == 0 {
                    break;
                }
                // 容器内可能有多个 Toolbar（按钮区/时钟区），逐个检查
                let mut tb = 0isize;
                loop {
                    let tcls = w("ToolbarWindow32");
                    tb = FindWindowExW(child, tb, tcls.as_ptr(), std::ptr::null());
                    if tb == 0 {
                        break;
                    }
                    let mut count = SendMessageW(tb, TB_BUTTONCOUNT, 0, 0);
                    if count <= 0 {
                        continue;
                    }
                    let mut i = 0isize;
                    while i < count {
                        let mut btn = TBBUTTON {
                            iBitmap: 0,
                            idCommand: 0,
                            fsState: 0,
                            fsStyle: 0,
                            bReserved: [0; 2],
                            dwData: 0,
                            iString: 0,
                        };
                        if SendMessageW(tb, TB_GETBUTTON, i as usize, &mut btn as *mut TBBUTTON as isize)
                            == 0
                        {
                            i += 1;
                            continue;
                        }
                        let mut buf = [0u16; 256];
                        let len = SendMessageW(tb, TB_GETBUTTONTEXTW, i as usize, buf.as_mut_ptr() as isize);
                        let matched = if len > 0 {
                            String::from_utf16_lossy(&buf[..(len as usize).min(256)])
                                .contains("ClawDesk")
                        } else {
                            false
                        };
                        if matched {
                            let _ = SendMessageW(tb, TB_DELETEBUTTON, i as usize, 0);
                            removed += 1;
                            // 删除后按钮索引重排 → 原地重试同索引
                            count = SendMessageW(tb, TB_BUTTONCOUNT, 0, 0);
                            if count <= 0 {
                                break;
                            }
                            continue;
                        }
                        i += 1;
                    }
                }
            }
        }
        if removed > 0 {
            eprintln!("[TRAY] 已清理 {removed} 个历史残留托盘图标");
        }
    }
}

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
    let builder = tauri::Builder::default()
        // ★ 单实例锁：同一时间只允许一个 ClawDesk 进程。
        //   防止多开（双进程同时收微信 → 重复回复）；重复启动时
        //   自动激活已有实例的窗口并退出新进程。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 第二个实例启动 → 找到主窗口并聚焦
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .manage(AppState::new())
        .manage(wechat::WechatBotState::default())
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
            commands::session_cmd::agent_sessions,
            commands::session_cmd::agent_session_messages,
            commands::session_cmd::agent_session_rename,
            commands::session_cmd::agent_session_metas,
            commands::session_cmd::agent_session_delete,
            commands::sandbox::sandbox_roots,
            commands::sandbox::sandbox_add_root,
            commands::sandbox::sandbox_remove_root,
            commands::set_sensitive_guard,
            commands::get_sensitive_guard,
            commands::router_cmd::router_status,
            commands::router_cmd::router_configure_main,
            commands::router_cmd::router_set_main_model,
            commands::router_cmd::router_configure_vision,
            commands::router_cmd::router_configure_image,
            commands::snapshot::snapshot_list,
            commands::snapshot::snapshot_restore,
            commands::snapshot::snapshot_delete,
            commands::snapshot::snapshot_diff,
            commands::settings_get,
            commands::settings_set,
            commands::settings_get_keys,
            // ── Edge TTS 朗读（神经网络拟人音色） ──
            commands::tts::tts_list_voices,
            commands::tts::tts_speak,
            commands::session_cmd::agent_fork,
            commands::session_cmd::agent_checkpoint,
            commands::session_cmd::agent_branches,
            commands::session_cmd::agent_session_usage,
            commands::balance::check_balance,
            commands::balance::deepseek_balance,
            commands::balance::list_models,
            commands::session_cmd::session_export,
            commands::session_cmd::session_search,
            commands::log_cmd::logs_tail,
            commands::log_cmd::logs_size,
            commands::log_cmd::app_last_error,
            commands::log_cmd::self_check_run,
            commands::win_cmd::win_open_in_explorer,
            commands::win_cmd::win_clipboard_set,
            commands::win_cmd::win_clipboard_get,
            commands::win_cmd::win_notify,
            commands::win_cmd::win_autostart,
            commands::skill_cmd::skills_list,
            commands::skill_cmd::skills_reload,
            commands::skill_cmd::skills_set_enabled,
            commands::export_cmd::export_all,
            // ── 方案B追加：harness 引擎命令 ──
            commands::harness_cmd::harness_set_model_config,
            commands::harness_cmd::harness_start_task,
            commands::harness_cmd::harness_stop_task,
            commands::harness_cmd::harness_respond_permission,
            commands::harness_cmd::harness_status,
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
            wechat::wechat_send_voice,
            wechat::wechat_bot_status,
            wechat::wechat_set_persona,
            wechat::wechat_set_proactive,
            wechat::wechat_set_bot_rules,
            wechat::wechat_history,
            wechat::wechat_typing,
            wechat::wechat_living_state,
            wechat::wechat_living_context,
            wechat::wechat_mood_state,
            wechat::wechat_soul_context,
    wechat::wechat_soul_snapshot,
            wechat::wechat_detail_add,
            wechat::wechat_profile_fact_add,
            wechat::wechat_detail_list,
            wechat::wechat_detail_forget,
            wechat::wechat_mood_record,
            wechat::mobile_qr_svg,
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

            // 敏感文件保护：启动时应用设置中的开关状态
            state
                .sensitive_guard
                .set_enabled(state.settings.get().sensitive_files_enabled);

            // ★ 关键路径初始化（放在技能扫描之前，避免被技能解析阻塞）：
            // AI 世界线 / 情绪 / 细节记忆
            crate::living_state::init();
            crate::mood::init();
            crate::detail_memory::init();
    crate::life_narrative::init();
    crate::relationship::init();
    crate::drives::init();
    crate::persona_traits::init();
    crate::ghost::init();
    crate::affinity::init();

            // ── 方案B追加：权限桥初始化（harness ↔ Vue 弹窗）──            // ── 方案B追加：权限桥初始化（harness ↔ Vue 弹窗）──
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
            // ★ 修复 v2：启动前先枚举任务栏，删除 tooltip 含 "ClawDesk" 的历史残留
            //   图标（进程被强杀时 explorer 偶尔不自动清理 → 托盘区堆积重复图标）。
            {
                #[cfg(target_os = "windows")]
                cleanup_stale_tray_icons();

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
                            destroy_tray();
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
        });

    // ★ 修复 v2：任何退出路径（菜单退出/窗口关闭/系统注销/app.exit/被正常终止）
    //   都销毁托盘图标，杜绝托盘区残留堆积。
    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|_app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            destroy_tray();
        }
    });
}
