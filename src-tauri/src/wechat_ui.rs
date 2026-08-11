//! 独立微信（UI 自动化）路线 —— 给 AI 一个本机多开的微信窗口。
//!
//! 与 `wechat.rs` 的 iLink Bot 路线互补，两条路线并存：
//! - 路线 1（Bot）：扫码登录你自己的微信，长轮询自动回复（wechat.rs）；
//! - 路线 2（本文件）：AI 直接操作一个独立微信窗口（多开账号），通过
//!   窗口截图 + 鼠标键盘模拟（SendInput）完成"看消息 → 点开会话 → 打字 → 发送"。
//!
//! 安全红线：`wechat_ui_send` 强制白名单校验 —— AI 只能发给
//! `wechat_ui_whitelist` 设置过的聊天对象，白名单为空时拒绝发送。
//! 白名单按窗口句柄持久化到 `<数据目录>/wechat_ui.json`。

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::json;

/// 每个窗口的白名单（hwnd → 允许聊天的对象名列表）。
static WHITELIST: OnceLock<parking_lot::Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

fn whitelist_map() -> &'static parking_lot::Mutex<HashMap<String, Vec<String>>> {
    WHITELIST.get_or_init(|| {
        let m = load_whitelist().unwrap_or_default();
        parking_lot::Mutex::new(m)
    })
}

fn whitelist_path() -> std::path::PathBuf {
    crate::llm::settings::clawdesk_dir().join("wechat_ui.json")
}

fn load_whitelist() -> Option<HashMap<String, Vec<String>>> {
    let text = std::fs::read_to_string(whitelist_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut out = HashMap::new();
    for (k, users) in v.get("whitelist")?.as_object()?.iter() {
        let list = users
            .as_array()?
            .iter()
            .filter_map(|u| u.as_str().map(String::from))
            .collect();
        out.insert(k.clone(), list);
    }
    Some(out)
}

fn save_whitelist(map: &HashMap<String, Vec<String>>) -> Result<(), String> {
    let path = whitelist_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let v = json!({ "whitelist": map });
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|e| format!("白名单持久化失败: {e}"))
}

/// 读取指定窗口的可聊天对象白名单。
pub fn whitelist_of(hwnd: &str) -> Vec<String> {
    whitelist_map().lock().get(hwnd).cloned().unwrap_or_default()
}

/// 设置指定窗口的白名单（逗号/顿号分隔；空字符串 = 清空）。
pub fn set_whitelist(hwnd: &str, users: &str) -> Result<Vec<String>, String> {
    let list: Vec<String> = users
        .split([',', '，', '、', '\n', '\r', ' '])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut map = whitelist_map().lock();
    if list.is_empty() {
        map.remove(hwnd);
    } else {
        map.insert(hwnd.to_string(), list.clone());
    }
    let snapshot = map.clone();
    drop(map);
    save_whitelist(&snapshot)?;
    Ok(list)
}

/// 校验目标是否在白名单内（大小写不敏感的子串匹配）。
fn check_whitelist(hwnd: &str, to: &str) -> Result<(), String> {
    let list = whitelist_of(hwnd);
    if list.is_empty() {
        return Err("该微信未设置可聊天对象（wechat_ui_whitelist）——AI 不允许发送消息".into());
    }
    let to_lower = to.to_lowercase();
    if !list.iter().any(|u| to_lower.contains(&u.to_lowercase())) {
        return Err(format!(
            "{to} 不在可聊天白名单（{}）中，拒绝发送",
            list.join(" / ")
        ));
    }
    Ok(())
}

// ─── Win32 封装（windows 0.61 API）──────────────────────────

#[cfg(target_os = "windows")]
mod win32 {
    use std::time::Duration;

    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC,
        DeleteObject, GetDIBits, GetDC, ReleaseDC, SelectObject, DIB_RGB_COLORS,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEINPUT, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN,
        MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE,
        VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_LSHIFT, VK_RETURN, VK_RIGHT,
        VK_SPACE, VK_TAB, VK_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextW, IsWindowVisible, SetCursorPos,
        SetForegroundWindow, ShowWindow, SW_RESTORE, WNDENUMPROC,
    };

    /// 微信窗口信息。
    pub struct WechatWindow {
        pub hwnd: isize,
        pub title: String,
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
        pub visible: bool,
    }

    fn is_true(b: BOOL) -> bool {
        b.0 != 0
    }

    /// 枚举全部顶级窗口，筛出微信窗口（标题含"微信"或 WeChat）。
    pub fn list_wechat_windows() -> Vec<WechatWindow> {
        unsafe {
            extern "system" fn proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
                unsafe {
                let list = &mut *(lparam.0 as *mut Vec<WechatWindow>);
                let mut buf = vec![0u16; 512];
                let len = GetWindowTextW(hwnd, &mut buf);
                if len <= 0 {
                    return BOOL(1);
                }
                let title = String::from_utf16_lossy(&buf[..len as usize]);
                if !title.contains("微信") && !title.contains("WeChat") {
                    return BOOL(1);
                }
                let mut r = RECT::default();
                let _ = GetWindowRect(hwnd, &mut r);
                let visible = is_true(IsWindowVisible(hwnd));
                list.push(WechatWindow {
                    hwnd: hwnd.0 as isize,
                    title,
                    left: r.left,
                    top: r.top,
                    right: r.right,
                    bottom: r.bottom,
                    visible,
                });
                BOOL(1)
                }
            }
            let mut list: Vec<WechatWindow> = Vec::new();
            let proc: WNDENUMPROC = Some(proc);
            let _ = EnumWindows(proc, LPARAM(&mut list as *mut Vec<WechatWindow> as isize));
            list
        }
    }

    fn hwnd_of(id: isize) -> HWND {
        HWND(id as *mut core::ffi::c_void)
    }

    /// 把窗口带到前台并还原（最小化时 SendInput 无法聚焦输入框）。
    fn activate(hwnd: isize) -> Result<(), String> {
        let h = hwnd_of(hwnd);
        unsafe {
            let _ = ShowWindow(h, SW_RESTORE);
            for _ in 0..3 {
                if is_true(SetForegroundWindow(h)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(80));
            }
        }
        std::thread::sleep(Duration::from_millis(120));
        Ok(())
    }

    /// 窗口相对坐标 → 屏幕绝对坐标。
    fn to_screen(hwnd: isize, x: i32, y: i32) -> Result<(i32, i32), String> {
        let mut r = RECT::default();
        unsafe {
            GetWindowRect(hwnd_of(hwnd), &mut r).map_err(|e| format!("获取窗口矩形失败: {e}"))?;
        }
        Ok((r.left + x, r.top + y))
    }

    fn mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS, data: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send_inputs(inputs: &[INPUT]) -> Result<(), String> {
        let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if n != inputs.len() as u32 {
            return Err(format!("SendInput 失败（发送 {} / {}）", n, inputs.len()));
        }
        Ok(())
    }

    /// 在窗口内 (x,y) 处点击（左键；double=true 双击）。
    pub fn click_at(hwnd: isize, x: i32, y: i32, double: bool) -> Result<(), String> {
        activate(hwnd)?;
        let (sx, sy) = to_screen(hwnd, x, y)?;
        unsafe {
            let _ = SetCursorPos(sx, sy);
        }
        std::thread::sleep(Duration::from_millis(80));
        for _ in 0..if double { 2 } else { 1 } {
            send_inputs(&[
                mouse_input(0, 0, MOUSEEVENTF_LEFTDOWN, 0),
                mouse_input(0, 0, MOUSEEVENTF_LEFTUP, 0),
            ])?;
            std::thread::sleep(Duration::from_millis(60));
        }
        Ok(())
    }

    /// 输入文本（Unicode，支持中文）。先聚焦窗口再 SendInput。
    pub fn type_text(hwnd: isize, text: &str) -> Result<(), String> {
        activate(hwnd)?;
        let mut inputs: Vec<INPUT> = Vec::new();
        for ch in text.encode_utf16() {
            inputs.push(key_input(VIRTUAL_KEY(0), ch, KEYEVENTF_UNICODE));
            inputs.push(key_input(VIRTUAL_KEY(0), ch, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
        send_inputs(&inputs)?;
        Ok(())
    }

    /// 特殊按键。key 支持 enter/esc/tab/backspace/up/down/left/right/home/end/delete/space/f1~f12，
    /// 组合键支持 "+"（如 ctrl+a、ctrl+enter、shift+enter）。
    pub fn press_key(hwnd: isize, key: &str) -> Result<(), String> {
        activate(hwnd)?;
        let lower = key.trim().to_lowercase();
        let parts: Vec<&str> = lower.split('+').collect();
        let main = parts.last().copied().unwrap_or("");
        let vk: VIRTUAL_KEY = match main {
            "enter" | "return" => VK_RETURN,
            "esc" | "escape" => VK_ESCAPE,
            "tab" => VK_TAB,
            "backspace" => VK_BACK,
            "up" => VK_UP,
            "down" => VK_DOWN,
            "left" => VK_LEFT,
            "right" => VK_RIGHT,
            "home" => VK_HOME,
            "end" => VK_END,
            "delete" | "del" => VK_DELETE,
            "space" => VK_SPACE,
            _ => {
                let fnum = main.strip_prefix('f').and_then(|s| s.parse::<u16>().ok());
                match fnum {
                    Some(n @ 1..=12) => VIRTUAL_KEY(0x70 + n - 1),
                    _ => {
                        // 单字母键（如 ctrl+f）：a~z → VK 0x41~0x5A，支持组合修饰符
                        let mut chars = main.chars();
                        match chars.next() {
                            Some(c) if c.is_ascii_lowercase() && chars.next().is_none() => {
                                VIRTUAL_KEY(0x41 + c as u16 - 'a' as u16)
                            }
                            _ => return Err(format!("不支持的按键: {key}")),
                        }
                    }
                }
            }
        };
        let mut mods: Vec<VIRTUAL_KEY> = Vec::new();
        for p in &parts[..parts.len().saturating_sub(1)] {
            match *p {
                "ctrl" | "control" => mods.push(VK_CONTROL),
                "shift" => mods.push(VK_LSHIFT),
                "alt" => mods.push(VIRTUAL_KEY(0x12)),
                "win" => mods.push(VIRTUAL_KEY(0x5B)),
                _ => return Err(format!("不支持的修饰键: {p}")),
            }
        }
        let mut inputs: Vec<INPUT> = Vec::new();
        for m in &mods {
            inputs.push(key_input(*m, 0, KEYBD_EVENT_FLAGS(0)));
        }
        inputs.push(key_input(vk, 0, KEYBD_EVENT_FLAGS(0)));
        inputs.push(key_input(vk, 0, KEYEVENTF_KEYUP));
        for m in mods.iter().rev() {
            inputs.push(key_input(*m, 0, KEYEVENTF_KEYUP));
        }
        send_inputs(&inputs)?;
        Ok(())
    }

    /// 在窗口内 (x,y) 处滚动鼠标滚轮（delta 正=向上，负=向下，步长建议 ±120）。
    pub fn scroll_at(hwnd: isize, x: i32, y: i32, delta: i32) -> Result<(), String> {
        activate(hwnd)?;
        let (sx, sy) = to_screen(hwnd, x, y)?;
        unsafe {
            let _ = SetCursorPos(sx, sy);
        }
        std::thread::sleep(Duration::from_millis(80));
        send_inputs(&[mouse_input(0, 0, MOUSEEVENTF_WHEEL, delta as u32)])?;
        Ok(())
    }

    /// 截取窗口画面（PrintWindow → PNG base64）。最小化窗口可能返回黑屏。
    pub fn capture_window(hwnd: isize) -> Result<(String, u32, u32), String> {
        let mut r = RECT::default();
        unsafe {
            GetWindowRect(hwnd_of(hwnd), &mut r).map_err(|e| format!("获取窗口矩形失败: {e}"))?;
        }
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return Err("窗口尺寸无效".into());
        }

        unsafe {
            let screen_dc = GetDC(None);
            if screen_dc.is_invalid() {
                return Err("获取屏幕 DC 失败".into());
            }
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            if mem_dc.is_invalid() {
                let _ = ReleaseDC(None, screen_dc);
                return Err("创建内存 DC 失败".into());
            }
            let bmp = CreateCompatibleBitmap(screen_dc, w, h);
            if bmp.is_invalid() {
                let _ = DeleteDC(mem_dc);
                let _ = ReleaseDC(None, screen_dc);
                return Err("创建位图失败".into());
            }
            let _old = SelectObject(mem_dc, bmp.into());
            // PW_RENDERFULLCONTENT=2：即使窗口被遮挡也完整渲染内容
            if !is_true(PrintWindow(hwnd_of(hwnd), mem_dc, PRINT_WINDOW_FLAGS(2))) {
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(mem_dc);
                let _ = ReleaseDC(None, screen_dc);
                return Err("PrintWindow 失败（该窗口不支持后台渲染，请勿最小化）".into());
            }

            // BGRA 位图（biHeight 取负 → 自上而下排列）
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [Default::default()],
            };
            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            let got = GetDIBits(
                mem_dc,
                bmp,
                0,
                h as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            );
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
            if got == 0 {
                return Err("读取像素失败".into());
            }

            // BGRA → RGBA
            let mut rgba = Vec::with_capacity(buf.len());
            for px in buf.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 0xFF]);
            }
            let img =
                image::RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("图片数据损坏")?;
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut png, image::ImageFormat::Png)
                .map_err(|e| format!("PNG 编码失败: {e}"))?;
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
            Ok((format!("data:image/png;base64,{b64}"), w as u32, h as u32))
        }
    }
}

// ─── 共用入口 ────────────────────────────────────────────────

fn list_windows_inner() -> Vec<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        win32::list_wechat_windows()
            .into_iter()
            .map(|w| {
                json!({
                    "hwnd": w.hwnd,
                    "title": w.title,
                    "left": w.left,
                    "top": w.top,
                    "right": w.right,
                    "bottom": w.bottom,
                    "width": w.right - w.left,
                    "height": w.bottom - w.top,
                    "visible": w.visible,
                })
            })
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// 解析窗口目标：优先 window_id（hwnd），其次按标题模糊匹配第一个微信窗口。
fn req_window_id(id: Option<String>, title: Option<String>) -> Result<isize, String> {
    if let Some(id) = id {
        return id.parse::<isize>().map_err(|_| format!("无效窗口句柄: {id}"));
    }
    let windows = list_windows_inner();
    let title = title.unwrap_or_default();
    let pick = windows
        .iter()
        .find(|w| {
            let t = w["title"].as_str().unwrap_or("");
            t.eq(&title) || t.contains(&title)
        })
        .or_else(|| windows.first());
    match pick {
        Some(w) => w["hwnd"]
            .as_i64()
            .map(|v| v as isize)
            .ok_or_else(|| "解析窗口句柄失败".into()),
        None => Err("未找到微信窗口，请先登录/打开微信（可多开独立账号）".into()),
    }
}

/// 等待毫秒（SendInput 之后给微信 UI 渲染留时间）。
pub fn wait_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

// ─── Tauri 命令（前端面板 + Agent 共用同一套底层）────────────

/// 列出当前所有微信窗口（多开时每个微信一个窗口，标题含"微信"）。
#[tauri::command]
pub fn wechat_ui_list_windows() -> Vec<serde_json::Value> {
    list_windows_inner()
}

/// 截取指定微信窗口画面，返回 PNG Data URL。
#[tauri::command]
pub fn wechat_ui_screenshot(
    window_id: Option<String>,
    window_title: Option<String>,
) -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window_id, window_title);
        return Err("仅支持 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        let hwnd = req_window_id(window_id, window_title)?;
        let (data_url, w, h) = win32::capture_window(hwnd)?;
        Ok(json!({ "hwnd": hwnd, "width": w, "height": h, "dataUrl": data_url }))
    }
}

/// 在微信窗口内 (x,y) 处点击（坐标相对窗口左上角；double=true 双击）。
#[tauri::command]
pub fn wechat_ui_click(
    window_id: Option<String>,
    x: i32,
    y: i32,
    double: Option<bool>,
) -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window_id, x, y, double);
        return Err("仅支持 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        let hwnd = req_window_id(window_id, None)?;
        win32::click_at(hwnd, x, y, double.unwrap_or(false))?;
        Ok(json!({ "ok": true, "hwnd": hwnd, "x": x, "y": y }))
    }
}

/// 向微信窗口输入文本（Unicode，支持中文）。
#[tauri::command]
pub fn wechat_ui_type(window_id: Option<String>, text: String) -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window_id, text);
        return Err("仅支持 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        let hwnd = req_window_id(window_id, None)?;
        win32::type_text(hwnd, &text)?;
        Ok(json!({ "ok": true, "hwnd": hwnd, "chars": text.chars().count() }))
    }
}

/// 发送特殊按键（enter/esc/tab/...，支持 ctrl+a 组合）。
#[tauri::command]
pub fn wechat_ui_key(window_id: Option<String>, key: String) -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window_id, key);
        return Err("仅支持 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        let hwnd = req_window_id(window_id, None)?;
        win32::press_key(hwnd, &key)?;
        Ok(json!({ "ok": true, "hwnd": hwnd, "key": key }))
    }
}

/// 在窗口内滚动鼠标滚轮（delta 正=向上/负=向下，步长 ±120）。
#[tauri::command]
pub fn wechat_ui_scroll(
    window_id: Option<String>,
    x: i32,
    y: i32,
    delta: i32,
) -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window_id, x, y, delta);
        return Err("仅支持 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        let hwnd = req_window_id(window_id, None)?;
        win32::scroll_at(hwnd, x, y, delta)?;
        Ok(json!({ "ok": true, "hwnd": hwnd, "delta": delta }))
    }
}

/// 设置指定微信窗口的"可聊天对象"白名单（逗号/顿号分隔；空 = 清空）。
#[tauri::command]
pub fn wechat_ui_whitelist(
    window_id: Option<String>,
    users: String,
) -> Result<serde_json::Value, String> {
    let hwnd = req_window_id(window_id, None)?;
    let list = set_whitelist(&hwnd.to_string(), &users)?;
    Ok(json!({ "ok": true, "hwnd": hwnd, "users": list }))
}

/// 读取指定窗口当前白名单。
#[tauri::command]
pub fn wechat_ui_whitelist_get(window_id: Option<String>) -> Result<serde_json::Value, String> {
    let hwnd = req_window_id(window_id, None)?;
    Ok(json!({ "hwnd": hwnd, "users": whitelist_of(&hwnd.to_string()) }))
}

/// 高层发送：搜索联系人 → 打开会话 → 输入 → 回车发送。强制白名单校验。
/// 流程：Ctrl+F 搜索框 → 输入对象名 → 回车（进入会话）→ 等渲染 → 输入内容 → 回车发送。
#[tauri::command]
pub fn wechat_ui_send(
    window_id: Option<String>,
    to: String,
    text: String,
) -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window_id, to, text);
        return Err("仅支持 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        if text.trim().is_empty() {
            return Err("消息内容为空".into());
        }
        let hwnd = req_window_id(window_id, None)?;
        let hwnd_str = hwnd.to_string();
        check_whitelist(&hwnd_str, &to)?;
        win32::press_key(hwnd, "ctrl+f")?;
        wait_ms(400);
        win32::type_text(hwnd, &to)?;
        wait_ms(600);
        win32::press_key(hwnd, "enter")?;
        wait_ms(900);
        win32::type_text(hwnd, &text)?;
        wait_ms(200);
        win32::press_key(hwnd, "enter")?;
        Ok(json!({
            "ok": true,
            "hwnd": hwnd,
            "to": to,
            "chars": text.chars().count(),
            "note": "已按 UI 流程发送（Ctrl+F → 搜联系人 → 回车 → 输入 → 回车）",
        }))
    }
}

/// 发送 Enter 键（微信 PC 端 Enter = 发送；需要换行用 shift+enter）。
#[tauri::command]
pub fn wechat_ui_send_enter(window_id: Option<String>) -> Result<serde_json::Value, String> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window_id;
        return Err("仅支持 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        let hwnd = req_window_id(window_id, None)?;
        win32::press_key(hwnd, "enter")?;
        Ok(json!({ "ok": true, "hwnd": hwnd }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 白名单持久化 roundtrip（临时数据目录）。
    #[test]
    fn whitelist_save_load_roundtrip() {
        // ★ 共享串行锁：与其他改 CLAWDESK_DATA_DIR 的测试互斥
        let _g = crate::llm::logging::test_env_lock();
        let dir = std::env::temp_dir().join(format!("clawdesk-wxui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = std::env::var("CLAWDESK_DATA_DIR").ok();
        std::env::set_var("CLAWDESK_DATA_DIR", &dir);

        let list = set_whitelist("12345", "小明，小红 老王").unwrap();
        assert_eq!(list, vec!["小明", "小红", "老王"]);
        assert_eq!(whitelist_of("12345"), vec!["小明", "小红", "老王"]);
        assert_eq!(whitelist_of("99999"), Vec::<String>::new());

        let list2 = set_whitelist("12345", "").unwrap();
        assert!(list2.is_empty());
        assert_eq!(whitelist_of("12345"), Vec::<String>::new());

        if let Some(v) = old {
            std::env::set_var("CLAWDESK_DATA_DIR", v);
        } else {
            std::env::remove_var("CLAWDESK_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 白名单校验：未设置 / 不在名单内 → 拒绝。
    #[test]
    fn whitelist_check_blocks_unlisted() {
        // ★ 共享串行锁：与其他改 CLAWDESK_DATA_DIR 的测试互斥
        let _g = crate::llm::logging::test_env_lock();
        // ★ 隔离数据目录（与其他白名单测试互不干扰，也避免写入真实数据目录）
        let dir = std::env::temp_dir().join(format!("clawdesk-wxui2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = std::env::var("CLAWDESK_DATA_DIR").ok();
        std::env::set_var("CLAWDESK_DATA_DIR", &dir);

        let err = check_whitelist("777", "陌生人").unwrap_err();
        assert!(err.contains("未设置可聊天对象"), "{err}");

        let _ = set_whitelist("777", "小明");
        let err = check_whitelist("777", "陌生人").unwrap_err();
        assert!(err.contains("不在可聊天白名单"), "{err}");

        check_whitelist("777", "小明-工作号").unwrap();

        if let Some(v) = old {
            std::env::set_var("CLAWDESK_DATA_DIR", v);
        } else {
            std::env::remove_var("CLAWDESK_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
