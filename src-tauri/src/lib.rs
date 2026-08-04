//! ClawDesk 应用入口：插件注册、系统托盘、全局快捷键、命令导出。

mod error;
mod fs_cmds;
mod llm;
mod mobile;
mod openclaw_bot;
mod project;
mod screenshot;
mod terminal;
mod tts;
mod watcher;
mod wechat;

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// closeToTray 行为标志：true=隐藏到托盘，false=真正退出
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(true);

#[tauri::command]
fn set_close_to_tray(flag: bool) {
    CLOSE_TO_TRAY.store(flag, Ordering::Relaxed);
}

/// 组装系统托盘（最小化到托盘 / 双击显示 / 右键菜单）
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let new_chat = MenuItem::with_id(app, "new_chat", "新建对话", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &new_chat, &settings, &quit])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .tooltip("ClawDesk")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "new_chat" => {
                show_main_window(app);
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("tray-new-chat", ());
                }
            }
            "settings" => {
                show_main_window(app);
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("tray-open-settings", ());
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 双击托盘图标显示/隐藏主窗口
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        show_main_window(app);
                    }
                }
            }
            let _ = MouseButtonState::Down; // 保持枚举引用，避免未使用告警
        })
        .build(app)?;
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

use tauri::Emitter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 环境检测：WebView2 是 Tauri 桌面应用运行的必要条件
    #[cfg(target_os = "windows")]
    {
        let has_webview2 = std::path::Path::new(
            "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application"
        ).exists() || std::path::Path::new(
            "C:\\Program Files\\Microsoft\\EdgeWebView\\Application"
        ).exists();
        if !has_webview2 {
            // 弹出控制台错误信息 + 自动打开下载页
            println!("\n  ═══════════════════════════════════════════");
            println!("  ClawDesk 无法启动：缺少 WebView2 运行时");
            println!("  ───────────────────────────────────────────");
            println!("  正在打开 Microsoft 官方下载页面...");
            println!("  安装后重新运行 ClawDesk 即可。");
            println!("  ═══════════════════════════════════════════\n");
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "https://go.microsoft.com/fwlink/p/?LinkId=2124703"])
                .spawn();
            // 等待用户看到消息
            std::thread::sleep(std::time::Duration::from_secs(5));
            std::process::exit(1);
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_sql::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(terminal::TerminalState::default())
        .manage(watcher::WatcherState::default())
        .manage(llm::LlmState::default())
        .manage(mobile::MobileBridgeState::default())
        .manage(wechat::WechatBotState::default())
        .manage(openclaw_bot::BotServerState::default())
        .setup(|app| {
            setup_tray(app)?;

            // 初始化微信 Bot 数据目录（token 持久化）
            {
                let wstate = app.state::<wechat::WechatBotState>();
                wechat::init_data_dir(app.handle(), &wstate);
            }

            // 全局快捷键 Ctrl+Shift+O：唤起 / 隐藏主窗口
            let shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyO);
            let handle = app.handle().clone();
            app.global_shortcut().on_shortcut(shortcut, move |_app, _sc, event| {
                if event.state == ShortcutState::Pressed {
                    if let Some(w) = handle.get_webview_window("main") {
                        if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
                            let _ = w.hide();
                        } else {
                            show_main_window(&handle);
                        }
                    }
                }
            })?;
            Ok(())
        })
        // 关闭行为由 closeToTray 设置决定（前端通过 set_close_to_tray 命令控制）
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if CLOSE_TO_TRAY.load(Ordering::Relaxed) {
                    // 隐藏到托盘
                    let _ = window.hide();
                    api.prevent_close();
                }
                // else: 不阻止关闭，应用正常退出
            }
        })
        .invoke_handler(tauri::generate_handler![
            set_close_to_tray,
            tts::tts_speak,
            tts::tts_stop,
            tts::tts_set_voice,
            tts::tts_list_voices,
            fs_cmds::read_dir_tree,
            fs_cmds::count_dir_files,
            fs_cmds::read_file_text,
            fs_cmds::read_file_base64,
            fs_cmds::write_file_text,
            fs_cmds::rename_path,
            fs_cmds::delete_path,
            terminal::terminal_spawn,
            terminal::terminal_write,
            terminal::terminal_kill,
            watcher::watch_dir,
            watcher::unwatch_dir,
            screenshot::capture_screen,
            llm::llm_chat_start,
            llm::llm_chat_cancel,
            llm::llm_balance,
            project::project_stats,
            mobile::mobile_bridge_start,
            mobile::mobile_bridge_stop,
            mobile::mobile_bridge_push,
            mobile::mobile_bridge_status,
            mobile::mobile_qr_svg,
            wechat::wechat_bot_start,
            wechat::wechat_bot_stop,
            wechat::wechat_bot_reply,
            wechat::wechat_bot_status,
            wechat::wechat_get_qr,
            wechat::wechat_qr_status,
            wechat::wechat_verify_code,
            wechat::wechat_refresh_qr,
            wechat::wechat_logout,
            wechat::wechat_send_message,
            openclaw_bot::bot_server_start,
            openclaw_bot::bot_server_stop,
            openclaw_bot::bot_server_status,
            openclaw_bot::start_wechat_bridge,
            openclaw_bot::stop_wechat_bridge,
            openclaw_bot::wechat_bridge_status,
        ])
        .run(tauri::generate_context!())
        .expect("ClawDesk 启动失败");
}
