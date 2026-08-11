//! 虚拟机内置微信 —— VNC 屏幕流内嵌 + 输入控制。
//!
//! 架构：
//! - 宿主机用 VirtualBox 跑一个 Windows 11 虚拟机（AI-WeChat），
//!   客机内装微信 + TightVNC Server（无头启动，NAT 端口转发 15900→5900）；
//! - 本模块实现 RFB 3.8 客户端：连接 → VNC 认证（DES）→ 接收帧更新（raw 编码）→
//!   维护帧缓冲 → 定期以 PNG base64 经 Tauri 事件推给前端 canvas；
//! - 输入：鼠标 PointerEvent + 键盘 KeyEvent（X11 keysym）+ 中文粘贴
//!   （ClientCutText 写入客机剪贴板 + Ctrl+V），AI 可通过工具直接操作虚拟机里的微信。
//!
//! 真微信内置：微信本体跑在虚拟机里，与本机微信完全隔离互不影响。

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use des::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use serde_json::json;
use tauri::Emitter;

/// VNC 会话核心（std 阻塞 IO，跑在专用线程里）。
struct VncCore {
    stream: std::net::TcpStream,
    width: u16,
    height: u16,
    /// RGBA 帧缓冲
    fb: Vec<u8>,
    connected: bool,
}

static SESSION: OnceLock<Mutex<Option<VncCore>>> = OnceLock::new();

fn session() -> &'static Mutex<Option<VncCore>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

/// VNC 密码 → DES 密钥（每个字节位反转，补齐 8 字节）。
fn vnc_key(password: &str) -> [u8; 8] {
    let mut key = [0u8; 8];
    let bytes = password.as_bytes();
    for i in 0..8.min(bytes.len()) {
        let mut b = bytes[i];
        let mut r = 0u8;
        for j in 0..8 {
            r |= ((b >> j) & 1) << (7 - j);
        }
        key[i] = r;
    }
    key
}

/// VNC 认证：DES-ECB 加密服务端 16 字节挑战。
fn vnc_auth_response(stream: &mut std::net::TcpStream, password: &str) -> Result<(), String> {
    let mut challenge = [0u8; 16];
    stream
        .read_exact(&mut challenge)
        .map_err(|e| format!("读取认证挑战失败: {e}"))?;
    let key = vnc_key(password);
    let cipher = des::Des::new_from_slice(&key).map_err(|e| format!("DES 初始化失败: {e}"))?;
    let mut resp = [0u8; 16];
    let mut block = GenericArray::clone_from_slice(&challenge[..8]);
    cipher.encrypt_block(&mut block);
    resp[..8].copy_from_slice(&block);
    block = GenericArray::clone_from_slice(&challenge[8..]);
    cipher.encrypt_block(&mut block);
    resp[8..].copy_from_slice(&block);
    stream
        .write_all(&resp)
        .map_err(|e| format!("发送认证响应失败: {e}"))?;
    Ok(())
}

/// 连接并完成 RFB 握手，返回 (VncCore, 桌面名)。
fn handshake(host: &str, port: u16, password: &str) -> Result<(VncCore, String), String> {
    let mut stream = std::net::TcpStream::connect((host, port))
        .map_err(|e| format!("连接 VNC {host}:{port} 失败: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("设置超时失败: {e}"))?;

    // 1. 版本协商
    let mut ver = [0u8; 12];
    stream
        .read_exact(&mut ver)
        .map_err(|e| format!("读取协议版本失败: {e}"))?;
    if !ver.starts_with(b"RFB 003.008") {
        // 兼容 3.3：服务器可能回复 003.003，仍按 3.8 流程继续
        eprintln!("[VM] VNC 版本: {}", String::from_utf8_lossy(&ver));
    }
    stream
        .write_all(b"RFB 003.008\n")
        .map_err(|e| format!("发送协议版本失败: {e}"))?;

    // 2. 安全类型
    let mut nsec = [0u8; 1];
    stream
        .read_exact(&mut nsec)
        .map_err(|e| format!("读取安全类型失败: {e}"))?;
    if nsec[0] == 0 {
        let mut reason_len = [0u8; 4];
        stream.read_exact(&mut reason_len).ok();
        let len = u32::from_be_bytes(reason_len) as usize;
        let mut reason = vec![0u8; len.min(512)];
        let _ = stream.read_exact(&mut reason);
        return Err(format!(
            "VNC 拒绝连接: {}",
            String::from_utf8_lossy(&reason)
        ));
    }
    let mut sec_types = vec![0u8; nsec[0] as usize];
    stream
        .read_exact(&mut sec_types)
        .map_err(|e| format!("读取安全类型列表失败: {e}"))?;

    let mut authed = false;
    if sec_types.contains(&1) {
        // None
        stream.write_all(&[1]).ok();
        authed = true;
    } else if sec_types.contains(&2) {
        // VNC 认证
        stream.write_all(&[2]).ok();
        vnc_auth_response(&mut stream, password)?;
        authed = true;
    }
    if !authed {
        return Err(format!(
            "VNC 不支持安全类型: {:?}（TightVNC 需勾选 VNC 认证）",
            sec_types
        ));
    }
    // SecurityResult
    let mut res = [0u8; 4];
    stream
        .read_exact(&mut res)
        .map_err(|e| format!("读取认证结果失败: {e}"))?;
    if u32::from_be_bytes(res) != 0 {
        return Err("VNC 认证失败（密码错误？）".into());
    }

    // 3. ClientInit
    stream.write_all(&[0]).ok();

    // 4. ServerInit
    let mut init = [0u8; 20];
    stream
        .read_exact(&mut init)
        .map_err(|e| format!("读取 ServerInit 失败: {e}"))?;
    let width = u16::from_be_bytes([init[0], init[1]]);
    let height = u16::from_be_bytes([init[2], init[3]]);
    let mut name_len = [0u8; 4];
    stream
        .read_exact(&mut name_len)
        .map_err(|e| format!("读取桌面名失败: {e}"))?;
    let nlen = u32::from_be_bytes(name_len) as usize;
    let mut name = vec![0u8; nlen.min(256)];
    let _ = stream.read_exact(&mut name);
    let desktop = String::from_utf8_lossy(&name).to_string();

    let mut core = VncCore {
        stream,
        width,
        height,
        fb: vec![0u8; width as usize * height as usize * 4],
        connected: true,
    };
    // 5. SetPixelFormat（32bpp truecolor BGRA）
    let mut pf = [0u8; 20];
    pf[0] = 0;
    pf[3] = 32; // bits-per-pixel
    pf[4] = 24; // depth
    pf[7] = 0; // big-endian flag = false
    pf[8] = 0; // true-colour
    pf[9] = 255; // red-max
    pf[10] = 255; // green-max
    pf[11] = 255; // blue-max
    pf[12] = 16; // red-shift
    pf[13] = 8; // green-shift
    pf[14] = 0; // blue-shift
    core.stream
        .write_all(&pf)
        .map_err(|e| format!("SetPixelFormat 失败: {e}"))?;
    // 6. SetEncodings：raw = 0
    let mut enc = [0u8; 8];
    enc[0] = 2;
    enc[4] = 0;
    enc[5] = 0;
    enc[6] = 0;
    enc[7] = 0;
    core.stream
        .write_all(&enc)
        .map_err(|e| format!("SetEncodings 失败: {e}"))?;
    // 7. 请求全屏更新
    send_fbu(&mut core, false)?;
    Ok((core, desktop))
}

/// 发送 FramebufferUpdateRequest（incremental=true 增量 / false 全屏）。
fn send_fbu(core: &mut VncCore, incremental: bool) -> Result<(), String> {
    let mut msg = [0u8; 10];
    msg[0] = 3;
    msg[1] = if incremental { 1 } else { 0 };
    core.stream
        .write_all(&msg)
        .map_err(|e| format!("FRU 失败: {e}"))
}

/// 虚拟机名（与 VirtualBox 中一致）。
const VM_NAME: &str = "AI-WeChat";

/// 用 VirtualBox 原生截图抓取虚拟机画面（可靠：VNC 抓屏在 Win11+VMSVGA 下会黑屏）。
/// 返回 (PNG Data URL, 宽, 高)。
fn vbox_screenshot_png() -> Result<(String, u32, u32), String> {
    let exe = "C:\\Program Files\\Oracle\\VirtualBox\\VBoxManage.exe";
    let tmp = std::env::temp_dir().join("clawdesk_vm_shot.png");
    let out = std::process::Command::new(exe)
        .args(["controlvm", VM_NAME, "screenshotpng"])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("VBoxManage 调用失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "VBox 截图失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let bytes = std::fs::read(&tmp).map_err(|e| format!("读取截图失败: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("解析截图失败: {e}"))?;
    let (w, h) = (img.width(), img.height());
    let _ = std::fs::remove_file(&tmp);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((format!("data:image/png;base64,{b64}"), w, h))
}

/// 后台读循环：接收服务端消息维持连接（TightVNC 抓屏在 Win11+VMSVGA 下黑屏，
/// 画面由 vbox_screenshot_png 线程提供，VNC 仅负责输入与连接保活）。
fn reader_loop(mut core: VncCore, app: tauri::AppHandle) {
    let mut last_full = Instant::now();
    loop {
        let mut buf = [0u8; 1];
        match core.stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        match buf[0] {
            0 => {
                // FramebufferUpdate：跳过矩形数据
                let mut rest = [0u8; 3];
                if core.stream.read_exact(&mut rest).is_err() {
                    break;
                }
                let nrects = u16::from_be_bytes([rest[1], rest[2]]);
                let mut ok = true;
                for _ in 0..nrects {
                    let mut rh = [0u8; 12];
                    if core.stream.read_exact(&mut rh).is_err() {
                        ok = false;
                        break;
                    }
                    let w = u16::from_be_bytes([rh[4], rh[5]]);
                    let h = u16::from_be_bytes([rh[6], rh[7]]);
                    let enc = u32::from_be_bytes([rh[8], rh[9], rh[10], rh[11]]);
                    match enc {
                        0 => {
                            let mut data = vec![0u8; w as usize * h as usize * 4];
                            if core.stream.read_exact(&mut data).is_err() {
                                ok = false;
                                break;
                            }
                        }
                        1 => {
                            let mut cr = [0u8; 4];
                            if core.stream.read_exact(&mut cr).is_err() {
                                ok = false;
                                break;
                            }
                        }
                        other => {
                            eprintln!("[VM] 未知编码 {other}，终止读取");
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    break;
                }
                let _ = send_fbu(&mut core, true);
            }
            2 => {
                // Bell
            }
            3 => {
                // ServerCutText
                let mut rest = [0u8; 7];
                if core.stream.read_exact(&mut rest).is_err() {
                    break;
                }
                let len = u32::from_be_bytes([rest[3], rest[4], rest[5], rest[6]]) as usize;
                let mut text = vec![0u8; len.min(65536)];
                let _ = core.stream.read_exact(&mut text);
            }
            4 => {
                let mut rest = [0u8; 6];
                let _ = core.stream.read_exact(&mut rest);
            }
            other => {
                eprintln!("[VM] 未知服务端消息类型 {other}");
                break;
            }
        }

        // 每 3s 强制全屏刷新（保活 + 触发增量）
        if Instant::now().duration_since(last_full) >= Duration::from_secs(3) {
            last_full = Instant::now();
            let _ = send_fbu(&mut core, false);
        }
    }
    // 会话结束
    core.connected = false;
    if let Ok(mut g) = session().lock() {
        *g = None;
    }
    let _ = app.emit("vm://status", json!({ "connected": false, "reason": "连接已断开" }));
    eprintln!("[VM] VNC 读循环退出");
}

/// 画面推送线程：独立于 VNC 连接，只要虚拟机在运行就持续用 VBox 截图推流。
/// （VNC 只负责鼠标键盘输入；画面永远可用，连接断开不中断帧流）
static STREAM_ON: AtomicBool = AtomicBool::new(false);
fn frame_loop(app: tauri::AppHandle) {
    while STREAM_ON.load(Ordering::Relaxed) {
        if let Ok((data_url, w, h)) = vbox_screenshot_png() {
            let _ = app.emit(
                "vm://frame",
                json!({ "dataUrl": data_url, "width": w, "height": h }),
            );
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
}

/// 启动画面流（幂等：全局只启动一个后台线程，仅当有前端监听时持续推帧）。
#[tauri::command]
pub fn vm_start_frame_stream(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_ok() {
        std::thread::spawn(move || frame_loop(app));
    }
    STREAM_ON.store(true, Ordering::Relaxed);
    Ok(json!({ "streaming": true }))
}

/// 停止画面流（面板关闭时调用，避免后台持续截图消耗 CPU）。
#[tauri::command]
pub fn vm_stop_frame_stream() -> Result<serde_json::Value, String> {
    STREAM_ON.store(false, Ordering::Relaxed);
    Ok(json!({ "streaming": false }))
}

fn lock_core() -> Result<std::sync::MutexGuard<'static, Option<VncCore>>, String> {
    session().lock().map_err(|_| "VNC 会话锁异常".to_string())
}

/// 当前是否已连接。
pub fn is_connected() -> bool {
    lock_core()
        .map(|g| g.as_ref().map(|c| c.connected).unwrap_or(false))
        .unwrap_or(false)
}

// ─── Tauri 命令 ──────────────────────────────────────────────

/// 连接虚拟机 VNC（默认 127.0.0.1:15900，密码 wxbot123）。
#[tauri::command]
pub fn vm_connect(
    app: tauri::AppHandle,
    host: Option<String>,
    port: Option<u16>,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = host.unwrap_or_else(|| "127.0.0.1".into());
    let port = port.unwrap_or(15900);
    let password = password.unwrap_or_else(|| "wxbot123".into());

    {
        let mut g = lock_core()?;
        if let Some(c) = g.as_ref() {
            if c.connected {
                return Err("VNC 已连接，请先断开".into());
            }
        }
    }

    let (core, desktop) = std::thread::scope(|_| {
        // handshake 在阻塞线程内执行（读超时保护）
        let handle = std::thread::spawn(move || handshake(&host, port, &password));
        match handle.join() {
            Ok(r) => r,
            Err(_) => Err("握手线程异常".into()),
        }
    })?;

    *lock_core()? = Some(core);
    let h = app.clone();
    std::thread::spawn(move || {
        let core = lock_core()
            .ok()
            .and_then(|mut g| g.take())
            .expect("会话已断开");
        reader_loop(core, h);
    });
    Ok(json!({ "connected": true, "desktop": desktop }))
}

/// 断开 VNC。
#[tauri::command]
pub fn vm_disconnect() -> Result<serde_json::Value, String> {
    let mut g = lock_core()?;
    *g = None;
    Ok(json!({ "disconnected": true }))
}

/// 鼠标事件（坐标相对屏幕；buttons: 1=左键按下, 2=中键, 4=右键, 0=松开）。
#[tauri::command]
pub fn vm_pointer(x: u16, y: u16, buttons: u8) -> Result<serde_json::Value, String> {
    let mut g = lock_core()?;
    let core = g.as_mut().ok_or("VNC 未连接")?;
    if !core.connected {
        return Err("VNC 未连接".into());
    }
    let mut msg = [0u8; 7];
    msg[0] = 5;
    msg[1] = buttons;
    msg[2] = (x >> 8) as u8;
    msg[3] = (x & 0xFF) as u8;
    msg[4] = (y >> 8) as u8;
    msg[5] = (y & 0xFF) as u8;
    core.stream
        .write_all(&msg)
        .map_err(|e| format!("发送鼠标事件失败: {e}"))?;
    Ok(json!({ "ok": true, "x": x, "y": y, "buttons": buttons }))
}

/// 键盘事件（keysym：ASCII 字符 = 本身；功能键用 0xFFxx）。
#[tauri::command]
pub fn vm_key(keysym: u32, down: bool) -> Result<serde_json::Value, String> {
    let mut g = lock_core()?;
    let core = g.as_mut().ok_or("VNC 未连接")?;
    if !core.connected {
        return Err("VNC 未连接".into());
    }
    let mut msg = [0u8; 8];
    msg[0] = 4;
    msg[1] = if down { 1 } else { 0 };
    msg[4] = (keysym >> 24) as u8;
    msg[5] = (keysym >> 16) as u8;
    msg[6] = (keysym >> 8) as u8;
    msg[7] = keysym as u8;
    core.stream
        .write_all(&msg)
        .map_err(|e| format!("发送键盘事件失败: {e}"))?;
    Ok(json!({ "ok": true, "keysym": keysym, "down": down }))
}

/// 粘贴文本（ClientCutText → 客机剪贴板；中文可用）。
#[tauri::command]
pub fn vm_paste(text: String) -> Result<serde_json::Value, String> {
    let mut g = lock_core()?;
    let core = g.as_mut().ok_or("VNC 未连接")?;
    if !core.connected {
        return Err("VNC 未连接".into());
    }
    let bytes = text.as_bytes();
    let mut msg = Vec::with_capacity(8 + bytes.len());
    msg.push(6);
    msg.push(0);
    msg.extend_from_slice(&[0, 0]);
    msg.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    msg.extend_from_slice(bytes);
    core.stream
        .write_all(&msg)
        .map_err(|e| format!("写入剪贴板失败: {e}"))?;
    Ok(json!({ "ok": true, "chars": text.chars().count() }))
}

/// 取当前帧截图（VBox 原生截图，PNG Data URL）。
#[tauri::command]
pub fn vm_screenshot() -> Result<serde_json::Value, String> {
    let (data_url, w, h) = vbox_screenshot_png()?;
    Ok(json!({ "dataUrl": data_url, "width": w, "height": h }))
}

/// 连接状态。
#[tauri::command]
pub fn vm_status() -> Result<serde_json::Value, String> {
    let g = lock_core()?;
    let core = g.as_ref();
    Ok(json!({
        "connected": core.map(|c| c.connected).unwrap_or(false),
        "width": core.map(|c| c.width).unwrap_or(0),
        "height": core.map(|c| c.height).unwrap_or(0),
    }))
}

/// 列出 VirtualBox 虚拟机。
#[tauri::command]
pub fn vm_list_vms() -> Result<serde_json::Value, String> {
    let out = run_vbox(&["list", "vms"])?;
    let mut vms = Vec::new();
    for line in out.lines() {
        if let Some((name, uuid)) = line.split_once(" {") {
            vms.push(json!({ "name": name.trim(), "uuid": uuid.trim_end_matches('}') }));
        }
    }
    // 运行状态
    let running = run_vbox(&["list", "runningvms"]).unwrap_or_default();
    for v in &mut vms {
        let name = v["name"].as_str().unwrap_or("");
        v["running"] = json!(running.lines().any(|l| l.starts_with(name)));
    }
    Ok(json!({ "vms": vms }))
}

/// 启动/停止虚拟机（默认 AI-WeChat）。
#[tauri::command]
pub fn vm_power(name: String, action: String) -> Result<serde_json::Value, String> {
    match action.as_str() {
        "start" => {
            let out = run_vbox(&["startvm", &name, "--type", "headless"])?;
            Ok(json!({ "ok": true, "out": out }))
        }
        "stop" => {
            let out = run_vbox(&["controlvm", &name, "acpipowerbutton"])?;
            Ok(json!({ "ok": true, "out": out }))
        }
        _ => Err("action 必须是 start 或 stop".into()),
    }
}

/// 确保虚拟机运行：未运行则后台启动（ClawDesk 启动时自动调用）。
#[tauri::command]
pub fn vm_ensure_running(name: Option<String>) -> Result<serde_json::Value, String> {
    let name = name.unwrap_or_else(|| VM_NAME.to_string());
    let running = run_vbox(&["list", "runningvms"])?;
    if running.lines().any(|l| l.starts_with(&name)) {
        return Ok(json!({ "running": true, "started": false }));
    }
    let out = run_vbox(&["startvm", &name, "--type", "headless"])?;
    Ok(json!({ "running": true, "started": true, "out": out }))
}

/// 设置虚拟机微信的"可聊天对象"白名单（逗号/顿号分隔；空 = 清空）。
/// AI 只能给白名单里的人发消息（vm_send 强制校验）。
#[tauri::command]
pub fn vm_whitelist_set(users: String) -> Result<serde_json::Value, String> {
    let list = crate::wechat_ui::set_whitelist("vm", &users)?;
    Ok(json!({ "ok": true, "users": list }))
}

/// 读取虚拟机微信当前白名单。
#[tauri::command]
pub fn vm_whitelist_get() -> Result<serde_json::Value, String> {
    Ok(json!({ "users": crate::wechat_ui::whitelist_of("vm") }))
}

/// 高级发送：Ctrl+F 搜联系人 → 粘贴名称 → 回车 → 粘贴内容 → 回车发送。
/// 强制白名单校验：to 必须在 vm_whitelist_set 设置的白名单内。
#[tauri::command]
pub fn vm_send(to: String, text: String) -> Result<serde_json::Value, String> {
    if text.trim().is_empty() {
        return Err("消息内容为空".into());
    }
    if !is_connected() {
        return Err("虚拟机 VNC 未连接（请先连接屏幕）".into());
    }
    // 白名单校验
    let list = crate::wechat_ui::whitelist_of("vm");
    if list.is_empty() {
        return Err("未设置可聊天对象（vm_whitelist_set）——AI 不允许发送消息".into());
    }
    let to_lower = to.to_lowercase();
    if !list.iter().any(|u| to_lower.contains(&u.to_lowercase())) {
        return Err(format!("{to} 不在可聊天白名单（{}）中，拒绝发送", list.join(" / ")));
    }
    // Ctrl+F 搜索
    press_combo("ctrl+f")?;
    crate::wechat_ui::wait_ms(500);
    paste_and_send(&to)?;
    crate::wechat_ui::wait_ms(800);
    press_combo("enter")?;
    crate::wechat_ui::wait_ms(1200);
    paste_and_send(&text)?;
    crate::wechat_ui::wait_ms(300);
    press_combo("enter")?;
    Ok(json!({
        "ok": true,
        "to": to,
        "chars": text.chars().count(),
        "note": "已通过虚拟机微信发送（Ctrl+F → 搜联系人 → 回车 → 输入 → 回车）",
    }))
}

fn run_vbox(args: &[&str]) -> Result<String, String> {
    let exe = "C:\\Program Files\\Oracle\\VirtualBox\\VBoxManage.exe";
    let out = std::process::Command::new(exe)
        .args(args)
        .output()
        .map_err(|e| format!("VBoxManage 调用失败（{exe}）: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "VBoxManage 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// 内置：发送快捷键（虚拟机关联）。
fn key_press(keysym: u32) -> Result<(), String> {
    let _ = vm_key(keysym, true)?;
    std::thread::sleep(Duration::from_millis(40));
    let _ = vm_key(keysym, false)?;
    Ok(())
}

/// 常用 keysym 解析（供工具层使用）。
pub fn parse_keysym(key: &str) -> Option<u32> {
    let lower = key.trim().to_lowercase();
    let ks: u32 = match lower.as_str() {
        "enter" | "return" => 0xFF0D,
        "esc" | "escape" => 0xFF1B,
        "tab" => 0xFF09,
        "backspace" => 0xFF08,
        "up" => 0xFF52,
        "down" => 0xFF54,
        "left" => 0xFF51,
        "right" => 0xFF53,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "delete" | "del" => 0xFFFF,
        "space" => 0x20,
        "ctrl" => 0xFFE3,
        "shift" => 0xFFE1,
        "alt" => 0xFFE9,
        "win" => 0xFFEB,
        _ => {
            if let Some(rest) = lower.strip_prefix("f") {
                if let Ok(n) = rest.parse::<u16>() {
                    if (1..=12).contains(&n) {
                        0xFFBEu32 + n as u32
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            } else if lower.len() == 1 {
                lower.as_bytes()[0] as u32
            } else {
                return None;
            }
        }
    };
    Some(ks)
}

/// 发送带修饰键的组合键（如 ctrl+f、shift+enter）。
pub fn press_combo(key: &str) -> Result<(), String> {
    let lower = key.trim().to_lowercase();
    let parts: Vec<&str> = lower.split('+').collect();
    let main = parts.last().copied().unwrap_or("");
    let mut mods: Vec<u32> = Vec::new();
    for p in &parts[..parts.len().saturating_sub(1)] {
        match *p {
            "ctrl" | "control" => mods.push(0xFFE3),
            "shift" => mods.push(0xFFE1),
            "alt" => mods.push(0xFFE9),
            "win" => mods.push(0xFFEB),
            _ => return Err(format!("不支持的修饰键: {p}")),
        }
    }
    let main_ks = parse_keysym(main).ok_or_else(|| format!("不支持的按键: {key}"))?;
    for m in &mods {
        let _ = vm_key(*m, true)?;
    }
    key_press(main_ks)?;
    for m in mods.iter().rev() {
        let _ = vm_key(*m, false)?;
    }
    Ok(())
}

/// 粘贴文本并 Ctrl+V（中文输入）。
pub fn paste_and_send(text: &str) -> Result<(), String> {
    let _ = vm_paste(text.to_string())?;
    std::thread::sleep(Duration::from_millis(100));
    press_combo("ctrl+v")
}
