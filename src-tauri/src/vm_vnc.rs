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
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

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

/// 只读模式：开启后 AI 只能截图查看，禁止点击/输入/发送（安全限制）。
static READONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 克隆音色：当前使用的参考音频文件名（voices 目录下）。
static VOICE: OnceLock<parking_lot::Mutex<String>> = OnceLock::new();

fn voice_name() -> String {
    VOICE
        .get_or_init(|| {
            let saved = std::fs::read_to_string(crate::llm::settings::clawdesk_dir().join("vm_voice.txt"))
                .unwrap_or_default();
            let name = saved.trim();
            if name.is_empty() {
                parking_lot::Mutex::new("示例音色.wav".to_string())
            } else {
                parking_lot::Mutex::new(name.to_string())
            }
        })
        .lock()
        .clone()
}

/// 克隆音色目录（可用环境变量 CLAWDESK_TTS_VOICES 覆盖，默认 IndexTTS2 的 voices）。
fn voices_dir() -> std::path::PathBuf {
    std::env::var("CLAWDESK_TTS_VOICES")
        .unwrap_or_else(|_| r"D:\workspace\indexTTS2\voices".to_string())
        .into()
}

fn voice_path() -> std::path::PathBuf {
    voices_dir().join(voice_name())
}

/// 列出可用克隆音色（voices 目录下的音频文件）。
#[tauri::command]
pub fn vm_voice_list() -> Result<serde_json::Value, String> {
    let dir = voices_dir();
    let mut files: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.to_lowercase().ends_with(".wav")
                || n.to_lowercase().ends_with(".mp3")
                || n.to_lowercase().ends_with(".flac")
                || n.to_lowercase().ends_with(".m4a")
            {
                files.push(n);
            }
        }
    }
    files.sort();
    let cur = voice_name();
    Ok(json!({ "voices": files, "current": cur, "dir": dir.to_string_lossy() }))
}

/// 设置克隆音色（必须是 voices 目录下的文件名）。
#[tauri::command]
pub fn vm_voice_set(name: String) -> Result<serde_json::Value, String> {
    let dir = voices_dir();
    // 文件名白名单：只允许纯文件名，防止把 voices 目录之外的文件当音色
    let name = name.trim().to_string();
    if name.is_empty()
        || name.starts_with('.')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ' ')
    {
        return Err("音色文件名不合法（只允许文件名本身，不能含路径）".into());
    }
    let target = dir.join(&name);
    if !target.is_file() {
        return Err(format!("音色文件不存在：{}（请把音频放到 {}\\voices\\）", name, dir.display()));
    }
    let path = crate::llm::settings::clawdesk_dir().join("vm_voice.txt");
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::write(&path, &name).map_err(|e| format!("保存音色设置失败: {e}"))?;
    *VOICE.get_or_init(|| parking_lot::Mutex::new("示例音色.wav".into())).lock() = name.clone();
    Ok(json!({ "ok": true, "current": name }))
}

/// AI 托管模式：后台监视虚拟机屏幕变化（微信新消息）→ 推事件给前端 → AI 自动回复。
struct GuardState {
    enabled: bool,
    last: Option<Vec<u8>>,
    prev_changed: bool,
    last_event_at: u64,
    /// ★ 红色像素比例（微信未读红点检测：新消息红点出现时红色像素比例上升）
    last_red: Option<f64>,
}

static GUARD: OnceLock<parking_lot::Mutex<GuardState>> = OnceLock::new();

fn guard_state() -> &'static parking_lot::Mutex<GuardState> {
    GUARD.get_or_init(|| {
        parking_lot::Mutex::new(GuardState {
            // ★ 默认开启：虚拟机是她的家，AI 托管默认全开（面板可关）
            enabled: true,
            last: None,
            prev_changed: false,
            last_event_at: 0,
            last_red: None,
        })
    })
}

/// 开启/关闭 AI 托管模式（AI 自动监视虚拟机微信新消息并回复）。
#[tauri::command]
pub fn vm_ai_guard_set(enabled: bool, app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _ = APP_HANDLE.set(app);
    guard_state().lock().enabled = enabled;
    ensure_guard_loop();
    Ok(json!({ "guard": enabled }))
}

/// 启动 guard 循环线程（幂等：全局只启动一次）。
/// 应用启动（setup）与 vm_ai_guard_set 共用，保证托管监视不依赖前端调用。
pub fn ensure_guard_loop() {
    static THREAD: OnceLock<()> = OnceLock::new();
    if THREAD.set(()).is_ok() {
        std::thread::spawn(|| guard_loop());
        eprintln!("[VM-GUARD] 🛡️ AI 托管监视循环已启动（每 5 秒检测虚拟机屏幕变化）");
    }
}

/// 应用启动时强制初始化：设置 AppHandle + 强制 enabled=true + 启动监视循环。
/// 与 vm_ai_guard_set 的区别：不读任何前端状态，保证"默认全开"。
pub fn vm_guard_init_force(app: tauri::AppHandle) -> Result<(), String> {
    let h = app.clone();
    let _ = APP_HANDLE.set(app);
    guard_state().lock().enabled = true;
    ensure_guard_loop();
    // ★ 2026-08-14：启动即异步自动连接 VNC（AI 托管无需用户手动开面板）。
    //   VM 在跑就直接连上；未运行则后续 vm_status/vm_send 会再尝试。
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3000));
        use std::io::Write;
        let path = "D:/AI-WeChat/diag_vnc.log";
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| {
            writeln!(f, "{} [VM-GUARD] 启动自动连接线程执行", chrono::Local::now().format("%H:%M:%S"))
        });
        if let Err(e) = ensure_vnc_connected() {
            eprintln!("[VM-GUARD] 启动自动连接未就绪（稍后工具调用会重试）: {e}");
        }
    });
    eprintln!("[VM-GUARD] 🚀 AI 托管已强制开启（应用启动默认全开）");
    Ok(())
}

/// 查询 AI 托管状态。
#[tauri::command]
pub fn vm_ai_guard_get() -> Result<serde_json::Value, String> {
    Ok(json!({ "guard": guard_state().lock().enabled }))
}

/// 监视循环：每 10 秒截图（排除任务栏），缩略图 diff，连续两次变化 → 推送事件。
fn guard_loop() {
    eprintln!("[VM-GUARD] guard_loop 启动");
    let mut tick: u64 = 0;
    // ★ 红点滞留计数：角标持续存在（AI 上回合没处理掉）的连续 tick 数
    let mut red_streak: u32 = 0;
    loop {
        std::thread::sleep(Duration::from_millis(5_000));
        tick += 1;
        // ★ 诊断：每个 tick 写文件（可执行代码，release 保留）
        {
            use std::io::Write;
            let path = r"D:\AI-WeChat\guard_runtime.log";
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "tick={} time={}", tick, chrono::Local::now().format("%H:%M:%S"));
            }
        }
        // 快速检查开关（截图是秒级重活，不能在持锁时做，否则 vm_ai_guard_get/set 会被卡住）
        
        let en = {
            let g = guard_state().lock();
            
            g.enabled
        };
        if !en {
            if tick % 2 == 0 {
                eprintln!("[VM-GUARD] ⏸️ tick={tick} enabled=false，等待开启");
            }
            continue;
        }
        if tick % 2 == 0 {
            eprintln!("[VM-GUARD] 🔄 tick={tick} enabled=true，正在截图检测");
        }
        let Ok((data_url, w, h)) = vbox_screenshot_png() else {
            eprintln!("[VM-GUARD] ❌ tick={tick} 截图失败（vbox_screenshot_png），继续等待");
            chain_log(&format!("❌ guard 截图失败 tick={tick}"));
            continue;
        };
    
        let b64 = data_url.split(',').nth(1).unwrap_or("");
        // ★ 诊断：保存 guard 截图到文件
        {
            if let Ok(b) = base64::engine::general_purpose::STANDARD.decode(b64.clone()) {
                let _ = std::fs::write(r"D:\AI-WeChat\guard_view.png", &b);
            }
        }
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            continue;
        };
        let Ok(img) = image::load_from_memory(&bytes) else { continue };
        // Nearest 比 Triangle 快数倍。
        // ★ 2026-08-16 修复：48x30 缩略图（1440px）对红点/数字角标这种小面积
        //   变化太钝（红点只占头像一角，缩略后变化像素可能不足 3% 被漏检）。
        //   提高到 160x100（16000px）+ 阈值降到 0.8%，小变化也能触发。
        let small = img.resize(160, 100, image::imageops::FilterType::Nearest).to_luma8();
        let sample = small.as_raw().to_vec();
        // ★★ 红点专用通道：微信未读红点/数字角标是红色 (r>150, g<100, b<100)。
        //   红点出现时全图红色像素数会显著增加（几十→几百），比灰度 diff 灵敏得多。
        //   全分辨率每 4 像素采样一次（49152 样本，每 5 秒一次，开销极小）。
        let rgb = img.to_rgb8();
        let raw = rgb.as_raw();
        let mut red_count: u64 = 0;
        let mut i = 0usize;
        while i + 2 < raw.len() {
            if raw[i] > 150 && raw[i + 1] < 100 && raw[i + 2] < 100 {
                red_count += 1;
            }
            i += 12; // 每 4 像素采样一次（步长 4*3=12）
        }
        let red_ratio = red_count as f64 / ((raw.len() / 12 + 1) as f64);
        let changed = {
            let g = guard_state().lock();
            match &g.last {
                Some(prev) => {
                    let n = sample.len().min(prev.len());
                    let diff = sample[..n]
                        .iter()
                        .zip(&prev[..n])
                        .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 18)
                        .count();
                    (diff as f64 / n as f64) > 0.008
                }
                None => false,
            }
        };
        // 红点通道：红色比例比上次增加超过 0.15%（约 +74 采样像素）→ 新红点出现
        let red_risen = {
            let g = guard_state().lock();
            match g.last_red {
                Some(prev) => red_ratio > prev + 0.0015,
                None => false,
            }
        };
        if changed || red_risen {
            let prev_red = guard_state().lock().last_red.unwrap_or(0.0);
            if red_risen {
                eprintln!("[VM-GUARD] 🔴 检测到红色未读红点增加（{:.4}% → {:.4}%）", prev_red, red_ratio);
            }
            chain_log(&format!(
                "🔔 guard 检测到变化 changed={} 红点 {:.4}%→{:.4}%（red_risen={}）",
                changed, prev_red, red_ratio, red_risen
            ));
            // ★ 修复：单次变化即触发（去掉"连续两次变化"的严格要求——单条新消息
            //   气泡出现后屏幕很快稳定，旧逻辑永远等不到第二次变化，导致新消息
            //   永远不触发回复）。30 秒节流防刷屏（微信新消息/屏幕操作频率足够）。
            let now = now_secs();
            let should_emit = {
                let g = guard_state().lock();
                now - g.last_event_at >= 30
            };
            if should_emit {
                let mut g = guard_state().lock();
                g.last_event_at = now;
                if let Some(app) = APP_HANDLE.get() {
                    eprintln!("[VM-GUARD] 🎯 检测到屏幕变化，emit vm://activity");
                    chain_log("🎯 guard emit vm://activity → 交给前端 AI 回合");
                    let _ = app.emit("vm://activity", json!({ "dataUrl": data_url, "w": w, "h": h }));
                }
            } else {
                chain_log("⏸️ guard 变化被 30s 节流拦下（上一事件刚发过）");
            }
            let mut g = guard_state().lock();
            g.prev_changed = false;
            g.last = Some(sample);
            g.last_red = Some(red_ratio);
        } else {
            // ★ 红点滞留重试（2026-08-17）：未读角标一旦没被上回合处理掉，
            //   红点基线会把它吸收成"常态"，普通通道永远不再触发——
            //   实测 8 条未读卡死无人回的根因。这里检测：红点持续 ≥2 分钟未消除
            //   且距上次事件 ≥5 分钟 → 强制重发 vm://activity 让 AI 再试，直到红点消失。
            if red_ratio > 0.0008 {
                red_streak += 1;
            } else {
                red_streak = 0;
            }
            if red_streak >= 24 && now_secs() - guard_state().lock().last_event_at >= 300 {
                if let Some(app) = APP_HANDLE.get() {
                    chain_log(&format!(
                        "🔁 红点滞留 {:.2}% 已 {} tick 未消除，强制重发 vm://activity 让 AI 重试",
                        red_ratio * 100.0, red_streak
                    ));
                    let _ = app.emit("vm://activity", json!({ "dataUrl": data_url, "w": w, "h": h }));
                }
                guard_state().lock().last_event_at = now_secs();
                red_streak = 0;
            }
            let mut g = guard_state().lock();
            g.prev_changed = false;
            g.last = Some(sample);
            g.last_red = Some(red_ratio);
        }
    }
}

/// AI 主动聊天（借鉴 AstrBot Cron 机制）：后台定时器随机间隔触发，
/// 到点推 vm://proactive 事件 → 前端调主对话 AI 生成话题 → vm_send 发给白名单对象。
struct ProactiveState {
    enabled: bool,
    interval_min: u64,
    interval_max: u64,
    next_at: u64,
}

static PROACTIVE: OnceLock<parking_lot::Mutex<ProactiveState>> = OnceLock::new();

fn proactive_state() -> &'static parking_lot::Mutex<ProactiveState> {
    PROACTIVE.get_or_init(|| {
        parking_lot::Mutex::new(ProactiveState {
            // ★ 默认开启：主动聊天默认全开（自带深夜安静/静默机制/存在感惩罚）
            enabled: true,
            interval_min: 60,
            interval_max: 180,
            next_at: 0,
        })
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 开启/关闭 AI 主动聊天（定时主动找白名单里的人聊天）。
/// interval_min/max：随机间隔（分钟），到点触发。
#[tauri::command]
pub fn vm_proactive_set(
    enabled: bool,
    interval_min: Option<u64>,
    interval_max: Option<u64>,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let _ = APP_HANDLE.set(app);
    let mut s = proactive_state().lock();
    s.enabled = enabled;
    if let Some(mn) = interval_min {
        s.interval_min = mn.clamp(5, 1440);
    }
    if let Some(mx) = interval_max {
        s.interval_max = mx.clamp(s.interval_min, 1440);
    }
    if s.interval_max < s.interval_min {
        s.interval_max = s.interval_min;
    }
    s.next_at = now_secs() + random_range(s.interval_min, s.interval_max) * 60;
    drop(s);
    ensure_proactive_loop();
    Ok(json!({
        "proactive": enabled,
        "intervalMin": interval_min.unwrap_or(60),
        "intervalMax": interval_max.unwrap_or(180),
    }))
}

/// 查询主动聊天配置。
#[tauri::command]
pub fn vm_proactive_get() -> Result<serde_json::Value, String> {
    let s = proactive_state().lock();
    Ok(json!({
        "proactive": s.enabled,
        "intervalMin": s.interval_min,
        "intervalMax": s.interval_max,
        "nextAt": s.next_at,
    }))
}

/// xorshift64*：轻量无锁伪随机（比时间种子分布均匀，多线程安全）。
static RNG_STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x9E37_79B9_7F4A_7C15);
fn random_range(lo: u64, hi: u64) -> u64 {
    let mut x = RNG_STATE.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    RNG_STATE.store(x, Ordering::Relaxed);
    lo + x % (hi - lo + 1)
}

/// 启动主动聊天循环线程（幂等）。应用启动时调用，保证主动聊天不依赖前端设置。
pub fn ensure_proactive_loop() {
    static THREAD: OnceLock<()> = OnceLock::new();
    if THREAD.set(()).is_ok() {
        std::thread::spawn(|| proactive_loop());
        eprintln!("[VM-PROACTIVE] 💬 AI 主动聊天循环已启动（每 30 秒检查是否到点）");
    }
}

/// 主动聊天循环：每 30 秒检查一次是否到点。
fn proactive_loop() {
    loop {
        std::thread::sleep(Duration::from_secs(30));
        let mut s = proactive_state().lock();
        if !s.enabled {
            continue;
        }
        let now = now_secs();
        if now < s.next_at {
            continue;
        }
        // 到点触发：推事件给前端（AI 生成话题并发送）
        // ★ 情绪驱动间隔：想念高 → 更想找人说话（缩短间隔）；
        //   深夜/情绪低落 → 安静待着（延长间隔）。让 AI 的"自由生活"有情绪节律。
        let mood_factor = {
            use chrono::Timelike;
            let hour = chrono::Local::now().hour();
            let m = crate::mood::mood_snapshot();
            let mut f = 1.0f64;
            if m.longing >= 0.55 {
                f *= 0.6; // 想念：想找人说话，间隔 ×0.6
            }
            if hour >= 23 || hour < 6 {
                f *= 1.8; // 深夜：安静，间隔 ×1.8
            }
            if m.joy <= 0.35 {
                f *= 1.5; // 低落：不太想动，间隔 ×1.5
            }
            f.clamp(0.5, 3.0)
        };
        let base = random_range(s.interval_min, s.interval_max) as f64;
        s.next_at = now + ((base * mood_factor) as u64).max(15) * 60;
        if let Some(app) = APP_HANDLE.get() {
            let _ = app.emit("vm://proactive", json!({ "at": now }));
        }
    }
}

fn check_writable() -> Result<(), String> {
    if READONLY.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("AI 微信处于只读模式（只允许查看屏幕，禁止操作）。请先在虚拟机面板关闭只读模式，或联系管理员授权".into());
    }
    Ok(())
}

/// 前端调试日志（写 D:\AI-WeChat\vm_frontend.log）。
/// ★ 全链路日志：后端关键节点统一写 D:\AI-WeChat\vm_frontend.log（与前端 vm_debug_log 同文件），
///   脱离控制台的 GUI 进程里 eprintln 会丢失，文件日志才能事后排查。
pub fn chain_log(msg: &str) {
    use std::io::Write;
    let path = r"D:\AI-WeChat\vm_frontend.log";
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} [be] {}", chrono::Local::now().format("%H:%M:%S"), msg);
    }
}

#[tauri::command]
pub fn vm_debug_log(msg: String) -> bool {
    chain_log(&format!("[fe] {msg}"));
    true
}

/// 设置/查询只读模式（true=AI 只能看不能操作）。
#[tauri::command]
pub fn vm_readonly_set(enabled: bool) -> Result<serde_json::Value, String> {
    READONLY.store(enabled, std::sync::atomic::Ordering::SeqCst);
    Ok(json!({ "readonly": enabled }))
}

/// 查询只读模式状态。
#[tauri::command]
pub fn vm_readonly_get() -> Result<serde_json::Value, String> {
    Ok(json!({ "readonly": READONLY.load(std::sync::atomic::Ordering::SeqCst) }))
}

/// VNC 密码 → DES 密钥（每个字节位反转，补齐 8 字节）。
fn vnc_key(password: &str) -> [u8; 8] {
    let mut key = [0u8; 8];
    let bytes = password.as_bytes();
    for i in 0..8.min(bytes.len()) {
        let b = bytes[i];
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
    // ★ 2026-08-14 修复：原实现字节偏移全错（bpp 放 pf[3]），TightVNC 收到
    //   非法 PixelFormat 会直接关闭连接 → reader_loop 自杀 → 连接永远连不上。
    //   RFB PixelFormat 布局（SetPixelFormat 消息 = 1字节 type + 3字节 padding
    //   + 16字节 pixel format，pixel format 从 offset 4 开始）：
    //     [4]=bits-per-pixel [5]=depth [6]=big-endian [7]=true-colour
    //     [8..9]=red-max [10..11]=green-max [12..13]=blue-max
    //     [14]=red-shift [15]=green-shift [16]=blue-shift [17..19]=padding
    let mut pf = [0u8; 20];
    pf[0] = 0; // message type
    pf[4] = 32; // bits-per-pixel
    pf[5] = 24; // depth
    pf[6] = 0; // big-endian flag = false
    pf[7] = 1; // true-colour
    pf[8] = 255; // red-max (high)
    pf[9] = 0; // red-max (low)
    pf[10] = 255; // green-max (high)
    pf[11] = 0; // green-max (low)
    pf[12] = 255; // blue-max (high)
    pf[13] = 0; // blue-max (low)
    pf[14] = 16; // red-shift
    pf[15] = 8; // green-shift
    pf[16] = 0; // blue-shift
    core.stream
        .write_all(&pf)
        .map_err(|e| format!("SetPixelFormat 失败: {e}"))?;
    // 6. SetEncodings：raw(0) + TightVNC 扩展剪贴板(0xC0A1E5CE)
    //   ★ 2026-08-16：加扩展剪贴板编码，让 TightVNC 服务器把 ClientCutText 按
    //   UTF-8 解码——这样剪贴板粘贴中文不再乱码（之前按 Latin-1 解导致"鏃╁晩"）。
    let mut enc = [0u8; 12];
    enc[0] = 2; // message type
    enc[1] = 0; // padding
    enc[2] = 0; // encoding count (high)
    enc[3] = 2; // encoding count (low) = 2
    enc[4] = 0; // encoding 0: raw
    enc[5] = 0;
    enc[6] = 0;
    enc[7] = 0;
    enc[8] = 0xC0; // encoding 1: pseudoEncodingExtendedClipboard (0xC0A1E5CE)
    enc[9] = 0xA1;
    enc[10] = 0xE5;
    enc[11] = 0xCE;
    core.stream
        .write_all(&enc)
        .map_err(|e| format!("SetEncodings 失败: {e}"))?;
    // 7. 请求全屏更新
    send_fbu(&mut core, false)?;
    Ok((core, desktop))
}

/// 发送 FramebufferUpdateRequest（incremental=true 增量 / false 全屏）。
/// ★ 2026-08-14 修复：原来 w/h 全 0（无效区域），TightVNC 不响应 → reader_loop
///   读超时 → 连接自杀。现在填全屏尺寸，服务器每次都会回 FramebufferUpdate 保活。
fn send_fbu(core: &mut VncCore, incremental: bool) -> Result<(), String> {
    let mut msg = [0u8; 10];
    msg[0] = 3;
    msg[1] = if incremental { 1 } else { 0 };
    msg[6] = (core.width >> 8) as u8;
    msg[7] = (core.width & 0xFF) as u8;
    msg[8] = (core.height >> 8) as u8;
    msg[9] = (core.height & 0xFF) as u8;
    core.stream
        .write_all(&msg)
        .map_err(|e| format!("FRU 失败: {e}"))
}

/// 虚拟机名（与 VirtualBox 中一致）。
const VM_NAME: &str = "AI-WeChat";

/// 数据目录（与 share_dir 同源，避免路径硬编码散落多处）。
fn vm_data_dir() -> std::path::PathBuf {
    std::env::var("CLAWDESK_DATA_DIR")
        .unwrap_or_else(|_| r"D:\AI-WeChat".to_string())
        .into()
}

/// VBox 忙标志：截图（vbox_screenshot_png）与排他命令（savestate/startvm/关机）互斥。
/// 背景：画面流/托管每 1~10 秒跑一次 VBoxManage screenshotpng，与 savestate/startvm
/// 并发时会竞争虚拟机会话锁 → 报 "machine is already locked by a session"。
/// 截图很快（<1s），让它让路；排他命令等截图释放后再执行。
static VBOX_BUSY: AtomicBool = AtomicBool::new(false);

/// RAII 忙标志：Drop 时自动释放。
struct VboxBusy;

impl VboxBusy {
    /// 非阻塞获取（截图用）：拿不到直接放弃本次截图。
    fn try_acquire() -> Option<VboxBusy> {
        if VBOX_BUSY.swap(true, Ordering::AcqRel) {
            None
        } else {
            Some(VboxBusy)
        }
    }

    /// 阻塞获取（排他命令用）：忙等截图释放，超时仍继续（宁可与截图并发，不卡死功能）。
    fn acquire(timeout: Duration) -> Option<VboxBusy> {
        let deadline = std::time::Instant::now() + timeout;
        while VBOX_BUSY.swap(true, Ordering::AcqRel) {
            if std::time::Instant::now() >= deadline {
                VBOX_BUSY.store(false, Ordering::Release);
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Some(VboxBusy)
    }
}

impl Drop for VboxBusy {
    fn drop(&mut self) {
        VBOX_BUSY.store(false, Ordering::Release);
    }
}

/// 用 VirtualBox 原生截图抓取虚拟机画面（可靠：VNC 抓屏在 Win11+VMSVGA 下会黑屏）。
/// 返回 (PNG Data URL, 宽, 高)。
fn vbox_screenshot_png() -> Result<(String, u32, u32), String> {
    // 与 savestate/startvm 互斥：忙时跳过本次截图（调用方 1~10s 后会重试）
    let _busy = VboxBusy::try_acquire().ok_or("VBox 忙（正在切换虚拟机模式），跳过本次截图")?;
    let exe = "C:\\Program Files\\Oracle\\VirtualBox\\VBoxManage.exe";
    // 临时文件按调用线程区分：guard 监视线程与画面流线程会同时截图，
    // 共用同一路径会产生半截文件竞态（读到另一线程正在写入的 PNG）。
    let tmp = vm_data_dir().join(format!("vm_shot_{:?}.png", std::thread::current().id()));
    let _ = std::fs::create_dir_all(vm_data_dir());
    let out = std::process::Command::new(exe)
        .creation_flags(0x08000000)
        .args(["controlvm", VM_NAME, "screenshotpng"])
        .arg(&tmp)
        .output()
        .map_err(|e| {
            format!("VBoxManage 调用失败: {e}")
        })?;
    if !out.status.success() {
        let msg = format!("VBox 截图失败: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Err(msg);
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
/// ★ 2026-08-14 修复：不再 take 会话里的 core（那会让所有命令拿不到连接），
///   改用 try_clone 的流；会话 core 保留供 vm_pointer/vm_send 等写操作使用。
fn reader_loop(mut stream: std::net::TcpStream, width: u16, height: u16, app: tauri::AppHandle) {
    // ★ 读超时 30s：TightVNC 在 VMSVGA 下黑屏、从不推送帧数据，超时是常态。
    //   ★ 关键修复：读超时 ≠ 断连！超时只代表"没数据可读"，连接仍然健康，
    //     继续发 FBU 保活即可；只有真正的连接错误（对端关闭/重置）才退出。
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut last_full = Instant::now();
    loop {
        let mut buf = [0u8; 1];
        match stream.read(&mut buf) {
            Ok(0) => break, // 对端关闭
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut
                || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                // ★ 读超时：无数据可读（TightVNC 黑屏不推帧的正常状态）。
                //   不是断连——直接走到底部 FBU 保活后继续循环。
                let _ = send_fbu_stream(&mut stream, false, width, height);
                continue;
            }
            Err(_) => break, // 真正连接错误
            Ok(_) => {}
        }
        match buf[0] {
            0 => {
                // FramebufferUpdate：跳过矩形数据
                let mut rest = [0u8; 3];
                if stream.read_exact(&mut rest).is_err() {
                    break;
                }
                let nrects = u16::from_be_bytes([rest[1], rest[2]]);
                let mut ok = true;
                for _ in 0..nrects {
                    let mut rh = [0u8; 12];
                    if stream.read_exact(&mut rh).is_err() {
                        ok = false;
                        break;
                    }
                    let w = u16::from_be_bytes([rh[4], rh[5]]);
                    let h = u16::from_be_bytes([rh[6], rh[7]]);
                    let enc = u32::from_be_bytes([rh[8], rh[9], rh[10], rh[11]]);
                    match enc {
                        0 => {
                            let mut data = vec![0u8; w as usize * h as usize * 4];
                            if stream.read_exact(&mut data).is_err() {
                                ok = false;
                                break;
                            }
                        }
                        1 => {
                            let mut cr = [0u8; 4];
                            if stream.read_exact(&mut cr).is_err() {
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
                let _ = send_fbu_stream(&mut stream, true, width, height);
            }
            2 => {
                // Bell
            }
            3 => {
                // ServerCutText
                let mut rest = [0u8; 7];
                if stream.read_exact(&mut rest).is_err() {
                    break;
                }
                let len = u32::from_be_bytes([rest[3], rest[4], rest[5], rest[6]]) as usize;
                let mut text = vec![0u8; len.min(65536)];
                let _ = stream.read_exact(&mut text);
            }
            4 => {
                let mut rest = [0u8; 6];
                let _ = stream.read_exact(&mut rest);
            }
            other => {
                eprintln!("[VM] 未知服务端消息类型 {other}");
                break;
            }
        }

        // 每 3s 强制全屏刷新（保活 + 触发增量）
        if Instant::now().duration_since(last_full) >= Duration::from_secs(3) {
            last_full = Instant::now();
            let _ = send_fbu_stream(&mut stream, false, width, height);
        }
    }
    // 会话结束：断开连接（core 仍在 session 中，标记为断开并清空）
    if let Ok(mut g) = session().lock() {
        if let Some(c) = g.as_mut() {
            c.connected = false;
        }
        *g = None;
    }
    let _ = app.emit("vm://status", json!({ "connected": false, "reason": "连接已断开" }));
    eprintln!("[VM] VNC 读循环退出");
}

/// 向克隆流发送 FramebufferUpdateRequest（reader_loop 保活用，不占会话锁）。
/// ★ 2026-08-14：w/h 填全屏，确保服务器响应（否则读循环超时自杀）。
fn send_fbu_stream(
    stream: &mut std::net::TcpStream,
    incremental: bool,
    width: u16,
    height: u16,
) -> Result<(), String> {
    let mut msg = [0u8; 10];
    msg[0] = 3;
    msg[1] = if incremental { 1 } else { 0 };
    msg[6] = (width >> 8) as u8;
    msg[7] = (width & 0xFF) as u8;
    msg[8] = (height >> 8) as u8;
    msg[9] = (height & 0xFF) as u8;
    stream
        .write_all(&msg)
        .map_err(|e| format!("FRU 失败: {e}"))
}

/// 画面推送线程：独立于 VNC 连接，只要虚拟机在运行就持续用 VBox 截图推流。
/// （VNC 只负责鼠标键盘输入；画面永远可用，连接断开不中断帧流）
/// 线程常驻（启动一次）；STREAM_ON 只作开关，停止时仅空转 sleep，不退出线程，
/// 否则面板关闭再打开后（OnceLock 不再 spawn）画面流无法恢复。
static STREAM_ON: AtomicBool = AtomicBool::new(false);
fn frame_loop(app: tauri::AppHandle) {
    loop {
        // STREAM_ON 为 false（面板关闭）时空转 sleep，不截图（也不退出线程，
        // 否则面板关闭再打开后 OnceLock 不再 spawn，画面流无法恢复）
        if STREAM_ON.load(Ordering::Relaxed) {
            if let Ok((data_url, w, h)) = vbox_screenshot_png() {
                let _ = app.emit(
                    "vm://frame",
                    json!({ "dataUrl": data_url, "width": w, "height": h }),
                );
            }
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
}

/// 全局 AppHandle（供 guard 线程发事件）。
static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

/// 启动画面流（幂等：全局只启动一个后台线程，仅当有前端监听时持续推帧）。
#[tauri::command]
pub fn vm_start_frame_stream(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let _ = APP_HANDLE.set(app.clone());
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

// ─── 自动连接（AI 托管无需用户手动开面板）────────────────────────
// ★ 2026-08-14 修复：vm_send / vm_click / vm_key / vm_fetch_file 等
//   全部改为"未连接时自动连接 VNC"——AI 托管模式下用户不打开虚拟机面板
//   也能发微信消息。截图（vbox_screenshot_png）走 VirtualBox 原生接口，
//   本身不依赖 VNC 连接，永远可用。

/// 确保 VNC 已连接；未连接时自动建立连接（默认 127.0.0.1:15900 / wxbot123）。
/// 幂等：已连接直接返回 Ok。AI 工具调用入口统一走这里，替代裸 is_connected() 检查。
pub fn ensure_vnc_connected() -> Result<(), String> {
    if is_connected() {
        return Ok(());
    }
    let host = "127.0.0.1";
    let port = 15900u16;
    let password = "wxbot123";
    // ★ 诊断：写文件日志（GUI 应用 eprintln 不可见）
    fn dlog(msg: &str) {
        use std::io::Write;
        let path = "D:/AI-WeChat/diag_vnc.log";
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} {}", chrono::Local::now().format("%H:%M:%S"), msg);
        }
    }
    dlog("ensure_vnc_connected 开始（尝试自动连接 VNC）");
    let hs = std::thread::scope(|_| {
        // handshake 在阻塞线程内执行（读超时保护）
        let handle = std::thread::spawn(move || handshake(host, port, password));
        match handle.join() {
            Ok(r) => r,
            Err(_) => {
                dlog("✗ 握手线程异常");
                Err("握手线程异常".into())
            }
        }
    });
    let core = match hs {
        Ok((c, _d)) => c,
        Err(e) => {
            dlog(&format!("✗ handshake 失败: {e}"));
            return Err(e);
        }
    };
    dlog(&format!("✓ handshake 成功 ({}x{})", core.width, core.height));
    let app = match APP_HANDLE.get() {
        Some(a) => a.clone(),
        None => {
            dlog("✗ AppHandle 未初始化");
            return Err("AppHandle 未初始化，无法建立 VNC 连接".into());
        }
    };
    // ★ 克隆流给 reader_loop（保活+读服务端消息），会话保留 core 供命令读写
    let reader_stream = match core.stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            dlog(&format!("✗ 克隆流失败: {e}"));
            return Err(format!("克隆 VNC 流失败: {e}"));
        }
    };
    let (w, h) = (core.width, core.height);
    *lock_core()? = Some(core);
    let h_app = app.clone();
    std::thread::spawn(move || reader_loop(reader_stream, w, h, h_app));
    dlog("✓ 自动连接完成，会话已保存");
    eprintln!("[VM] 🔌 AI 托管自动连接 VNC 成功（{host}:{port}）");
    Ok(())
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
        let g = lock_core()?;
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

    // ★ 克隆流给 reader_loop，会话保留 core 供命令读写
    let reader_stream = core
        .stream
        .try_clone()
        .map_err(|e| format!("克隆 VNC 流失败: {e}"))?;
    let (w, h) = (core.width, core.height);
    *lock_core()? = Some(core);
    let h_app = app.clone();
    std::thread::spawn(move || reader_loop(reader_stream, w, h, h_app));
    Ok(json!({ "connected": true, "desktop": desktop }))
}

/// 断开 VNC。
#[tauri::command]
pub fn vm_disconnect() -> Result<serde_json::Value, String> {
    let mut g = lock_core()?;
    *g = None;
    Ok(json!({ "disconnected": true }))
}

/// ★ 防长按保险（2026-08-17）：发送一次"全部按键松开"（mask=0）。
///   历史 bug：PointerEvent 多发 1 字节尾巴，被服务器误读成 SetPixelFormat 开头，
///   吞掉紧跟的"松开"事件 → 左键永远按住 → 下一次点击变成拖拽（用户看到的长按）。
///   现在所有点击入口先强制松开一次，双保险。
fn force_pointer_release() {
    if let Ok(mut g) = lock_core() {
        if let Some(core) = g.as_mut() {
            if core.connected {
                let cx = (core.width / 2) as u16;
                let cy = (core.height / 2) as u16;
                // RFB PointerEvent 精确 6 字节：[5, mask, xHi, xLo, yHi, yLo]
                let msg: [u8; 6] = [5, 0, (cx >> 8) as u8, (cx & 0xFF) as u8, (cy >> 8) as u8, (cy & 0xFF) as u8];
                let _ = core.stream.write_all(&msg);
            }
        }
    }
}

/// 鼠标事件（坐标相对屏幕；buttons: 1=左键按下, 2=中键, 4=右键, 0=松开）。
#[tauri::command]
pub fn vm_pointer(x: u16, y: u16, buttons: u8) -> Result<serde_json::Value, String> {
    chain_log(&format!("🖱️ vm_pointer({x},{y}) buttons={buttons}"));
    check_writable()?;
    ensure_vnc_connected()?;
    let mut g = lock_core()?;
    let core = g.as_mut().ok_or("VNC 未连接")?;
    if !core.connected {
        return Err("VNC 未连接".into());
    }
    // ★ 修复：RFB PointerEvent 协议是 6 字节，之前发 7 字节（多一个 0x00 尾巴），
    //   服务器把它当 SetPixelFormat 开头吞掉后续事件 → 间歇性丢"松开"→ 长按/拖拽。
    let msg: [u8; 6] = [5, buttons, (x >> 8) as u8, (x & 0xFF) as u8, (y >> 8) as u8, (y & 0xFF) as u8];
    core.stream
        .write_all(&msg)
        .map_err(|e| format!("发送鼠标事件失败: {e}"))?;
    Ok(json!({ "ok": true, "x": x, "y": y, "buttons": buttons }))
}

/// 键盘事件（keysym：ASCII 字符 = 本身；功能键用 0xFFxx）。
#[tauri::command]
pub fn vm_key(keysym: u32, down: bool) -> Result<serde_json::Value, String> {
    check_writable()?;
    ensure_vnc_connected()?;
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
    check_writable()?;
    ensure_vnc_connected()?;
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

/// ★ 语义点击（2026-08-16 新增）：AI 不需要自己估算坐标，
///   传语义位置即可点击微信界面的常用区域。坐标按当前 VNC 屏幕宽高比例换算
///   （基准布局 1024x768：微信 PC 全屏，左侧聊天列表，底部输入框）。
///   支持的 spot：
///   - "input"/"输入框"：聊天输入框（窗口底部中央，输入消息前点这里聚焦）
///   - "send"/"发送"：发送按钮（输入框右侧）
///   - "search"/"搜索框"：顶部搜索框
///   - "chat1"/"chat2"/"chat3"：聊天列表第 1/2/3 条会话
///   - "center"/"中央"：屏幕中央（唤醒/聚焦）
#[tauri::command]
pub fn vm_click_spot(spot: String) -> Result<serde_json::Value, String> {
    check_writable()?;
    ensure_vnc_connected()?;
    let mut g = lock_core()?;
    let core = g.as_mut().ok_or("VNC 未连接")?;
    if !core.connected {
        return Err("VNC 未连接".into());
    }
    let w = core.width.max(1) as f64;
    let h = core.height.max(1) as f64;
    let s = spot.trim();
    // 基准 1024x768 布局，按实际宽高比例换算
    let (fx, fy): (f64, f64) = match s {
        "input" | "输入框" | "inputbox" => (0.48, 0.935),
        "send" | "发送" | "发送按钮" => (0.76, 0.935),
        "search" | "搜索" | "搜索框" => (0.30, 0.068),
        "chat1" | "第一条会话" => (0.16, 0.215),
        "chat2" | "第二条会话" => (0.16, 0.30),
        "chat3" | "第三条会话" => (0.16, 0.385),
        "center" | "中央" | "屏幕中央" => (0.5, 0.5),
        _ => {
            return Err(format!(
                "不支持的点击位置: {s}（支持：input输入框 / send发送 / search搜索框 / chat1~3会话条目 / center中央）"
            ))
        }
    };
    let x = (w * fx) as u16;
    let y = (h * fy) as u16;
    chain_log(&format!("🖱️ vm_click_spot({s}) → ({x},{y})"));
    // ★ 双保险：先强制松开一切按键（防历史长按残留），再点击
    force_pointer_release();
    std::thread::sleep(std::time::Duration::from_millis(60));
    // 按下 + 松开（单击）——RFB PointerEvent 精确 6 字节（之前 7 字节尾巴会吞事件）
    let press: [u8; 6] = [5, 1, (x >> 8) as u8, (x & 0xFF) as u8, (y >> 8) as u8, (y & 0xFF) as u8];
    core.stream
        .write_all(&press)
        .map_err(|e| format!("发送点击失败: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(120));
    let release: [u8; 6] = [5, 0, (x >> 8) as u8, (x & 0xFF) as u8, (y >> 8) as u8, (y & 0xFF) as u8];
    if let Err(e) = core.stream.write_all(&release) {
        // 松开失败必须重试一次：丢了它 = 左键永远按住
        chain_log(&format!("⚠️ click_spot 松开写入失败（{e}），重试"));
        std::thread::sleep(std::time::Duration::from_millis(100));
        core.stream
            .write_all(&release)
            .map_err(|e| format!("发送点击松开失败: {e}"))?;
    }
    Ok(json!({ "ok": true, "spot": s, "x": x, "y": y }))
}

/// 取当前帧截图。
/// ★ 2026-08-15 修复：AI 是文本模型，看不懂 base64 图片（而且 base64 太大会被
///   LLM 工具结果截断，AI 一直"确认不了画面"）。
///   现在返回【本地文件路径】+ 小尺寸缩略图 base64：
///   - path：JPEG 文件路径，AI 必须用 analyze_image 工具读取（视觉模型解析成文字描述）
///   - dataUrl：小缩略图 base64（约 8~15KB），部分多模态模型可直接看
///   面板/guard 用 vbox_screenshot_png 原图不受影响。
#[tauri::command]
pub async fn vm_screenshot() -> Result<serde_json::Value, String> {
    let (path, data_url, w, h, full_path) = vbox_screenshot_ai()?;
    // ★★ 2026-08-17 接线：主模型是 deepseek-v4-flash（纯文本 API），看不懂 dataUrl 图片，
    //   之前只返回图片导致模型疯狂用 python/OCR 自救浪费 15 轮工具预算（读屏焦虑）。
    //   现在截图自动带读屏结果：优先 Windows OCR（快而准，用全尺寸图），
    //   失败降级 qwen2.5vl 场景描述。screenText 就是她的眼睛。
    let ocr_src = if std::path::Path::new(&full_path).exists() { full_path.clone() } else { path.clone() };
    let screen_text = match local_ocr_text(&ocr_src).await {
        Ok(t) => format!("[Windows OCR 读屏]\n{t}"),
        Err(e1) => match local_vision_ocr(&ocr_src).await {
            Ok(t) => format!("[本地视觉读屏（OCR失败: {e1}）]\n{t}"),
            Err(e2) => format!("[读屏失败 OCR: {e1} | 视觉: {e2}——请稍后再试一次 vm_screenshot]"),
        },
    };
    chain_log(&format!(
        "📸 vm_screenshot 完成，读屏 {} 字符：{}",
        screen_text.chars().count(),
        screen_text.chars().take(150).collect::<String>().replace('\n', " / ")
    ));
    Ok(json!({
        "path": path,
        "dataUrl": data_url,
        "width": w,
        "height": h,
        "screenText": screen_text,
        "note": "当前虚拟机屏幕截图。screenText=自动读屏结果，你以 screenText 为准判断界面和消息内容；path=完整截图文件路径。",
    }))
}

/// ★ Windows OCR 精确文字识别（调 python ocr.py，winsdk 引擎，比 qwen2.5vl 可靠）。
/// 2026-08-16：qwen2.5vl:3b 把微信界面误判成 Edge/Bing，改用 OCR 精确读出联系人名/消息。
async fn local_ocr_text(image_path: &str) -> Result<String, String> {
    // ★ CREATE_NO_WINDOW(0x08000000)：禁止弹出黑色控制台窗口（后台静默运行）
    let out = std::process::Command::new("python")
        .creation_flags(0x08000000)
        .arg(r"D:\AI-WeChat\ocr.py")
        .arg(image_path)
        .output()
        .map_err(|e| format!("调 Windows OCR 失败: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err("Windows OCR 返回空".into());
    }
    Ok(text)
}

/// AI 用截图：VBox 截图 → 缩小（最长边 640）→ JPEG q68 → 保存到附件目录，
/// 返回 (文件路径, 缩略图 base64, 宽, 高)。
/// 独立实现，不动 vbox_screenshot_png（面板/guard 需要原图）。
fn vbox_screenshot_ai() -> Result<(String, String, u32, u32, String), String> {
    let _busy = VboxBusy::try_acquire().ok_or("VBox 忙（正在切换虚拟机模式），跳过本次截图")?;
    let exe = "C:/Program Files/Oracle/VirtualBox/VBoxManage.exe";
    let tmp = vm_data_dir().join(format!("vm_shot_ai_{:?}.png", std::thread::current().id()));
    let _ = std::fs::create_dir_all(vm_data_dir());
    let out = std::process::Command::new(exe)
        .creation_flags(0x08000000)
        .args(["controlvm", VM_NAME, "screenshotpng"])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("VBoxManage 调用失败: {e}"))?;
    if !out.status.success() {
        let msg = format!("VBox 截图失败: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Err(msg);
    }
    let bytes = std::fs::read(&tmp).map_err(|e| format!("读取截图失败: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    let img = image::load_from_memory(&bytes).map_err(|e| format!("解析截图失败: {e}"))?;
    let (w, h) = (img.width(), img.height());
    // 缩小：最长边 640px（1024x768 → 640x480，数据量降 ~70%）
    let small = if w > 640 {
        let nh = ((h as u64 * 640) / w as u64) as u32;
        img.resize(640, nh.max(1), image::imageops::FilterType::Triangle)
    } else {
        img
    };
    // JPEG q68
    let rgb = small.to_rgb8();
    let mut jpg = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 68);
    encoder
        .encode_image(&rgb)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;
    // 保存到附件目录（AI 用 analyze_image 读取）
    let dir = crate::executors::builtin::attachment::attach_dir()?;
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S_%3f");
    let out_path = dir.join(format!("vm_shot_{stamp}.jpg"));
    std::fs::write(&out_path, &jpg).map_err(|e| format!("保存截图失败: {e}"))?;
    // ★ 2026-08-17：同时保存全尺寸 PNG——640px JPEG 太糊，Windows OCR 读出乱码，
    //   OCR/视觉必须用原图（实测全图 OCR 干净准确，缩图全是"辶丿噁"类乱码）。
    let full_path = dir.join(format!("vm_shot_{stamp}_full.png"));
    let _ = std::fs::write(&full_path, &bytes); // 失败不阻断（降级用缩图）
    let b64 = base64::engine::general_purpose::STANDARD.encode(&jpg);
    Ok((
        out_path.to_string_lossy().to_string(),
        format!("data:image/jpeg;base64,{b64}"),
        w,
        h,
        full_path.to_string_lossy().to_string(),
    ))
}

/// ★ 本地视觉识别（Ollama qwen2.5vl:3b）：把截图发给本地视觉模型，返回屏幕文字描述。
/// 2026-08-16 新增：替代云端 analyze_image——本地推理快、稳、免费，AI 直接读到文字。
/// 失败时返回 Err（调用方降级为仅路径提示）。
async fn local_vision_ocr(image_path: &str) -> Result<String, String> {
    let _t0 = std::time::Instant::now();
    chain_log("👁️ 本地视觉开始：截图发给 qwen2.5vl:3b 读屏幕…");
    // 读取图片并 base64（JPEG 已缩小，约 30~60KB）
    let bytes = std::fs::read(image_path).map_err(|e| format!("读取截图失败: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    // 调 Ollama /api/generate
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "model": "qwen2.5vl:3b",
        "prompt": "你是屏幕阅读助手。请用简体中文描述这张电脑截图：1) 屏幕上是什么窗口/界面；2) 微信窗口是否可见、是否被其他窗口遮挡（明确说出遮挡物，如记事本/浏览器/另一个微信窗口/弹窗/扫码登录框）；3) 逐条列出你能看到的所有文字内容（聊天消息、联系人名、按钮、时间等），尤其是聊天消息气泡里的文字。注意区分：新消息气泡、已读消息、列表项。",
        "images": [b64],
        "stream": false,
        "options": { "temperature": 0.1, "num_predict": 600 }
    });
    let resp = client
        .post("http://127.0.0.1:11434/api/generate")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| {
            chain_log(&format!("❌ 本地视觉连接失败: {e}"));
            format!("本地视觉服务连接失败（Ollama 未运行?）: {e}")
        })?;
    if !resp.status().is_success() {
        chain_log(&format!("❌ 本地视觉 HTTP {}", resp.status()));
        return Err(format!("本地视觉服务 HTTP {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("解析视觉响应失败: {e}"))?;
    let text = v.get("response").and_then(|r| r.as_str()).unwrap_or("").trim().to_string();
    if text.is_empty() {
        chain_log("❌ 本地视觉返回空结果");
        return Err("本地视觉返回空结果".into());
    }
    chain_log(&format!(
        "✅ 本地视觉完成（{:.1}s，{}字符）：{}",
        _t0.elapsed().as_secs_f64(),
        text.chars().count(),
        text.chars().take(120).collect::<String>()
    ));
    Ok(text)
}

/// 连接状态（★ 2026-08-14：查询前先尝试自动连接，AI 托管无需手动开面板）。
#[tauri::command]
pub fn vm_status() -> Result<serde_json::Value, String> {
    let _ = ensure_vnc_connected(); // 未连接时自动建立（失败不报错，返回真实状态）
    let g = lock_core()?;
    let core = g.as_ref();
    Ok(json!({
        "connected": core.map(|c| c.connected).unwrap_or(false),
        "width": core.map(|c| c.width).unwrap_or(0),
        "height": core.map(|c| c.height).unwrap_or(0),
        "readonly": READONLY.load(Ordering::SeqCst),
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
    let _busy = VboxBusy::acquire(Duration::from_secs(30));
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
    let _busy = VboxBusy::acquire(Duration::from_secs(30));
    let name = name.unwrap_or_else(|| VM_NAME.to_string());
    let running = run_vbox(&["list", "runningvms"])?;
    if running.lines().any(|l| l.starts_with(&name)) {
        return Ok(json!({ "running": true, "started": false }));
    }
    let out = run_vbox(&["startvm", &name, "--type", "headless"])?;
    Ok(json!({ "running": true, "started": true, "out": out }))
}

/// 虚拟机是否在运行。
fn vm_running() -> bool {
    run_vbox(&["list", "runningvms"])
        .map(|out| out.lines().any(|l| l.starts_with(VM_NAME)))
        .unwrap_or(false)
}

/// 查询虚拟机状态（showvminfo --machinereadable 的 VMState 字段）。
/// 取值示例：running / saved / saving / restoring / poweroff / aborted。
fn vm_state() -> String {
    run_vbox(&["showvminfo", VM_NAME, "--machinereadable"])
        .map(|out| {
            out.lines()
                .find(|l| l.starts_with("VMState="))
                .and_then(|l| l.split_once('='))
                .map(|(_, v)| v.trim_matches('"').to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// 等待虚拟机完成状态保存（savestate 是异步的：runningvms 里消失 ≠ 保存完成，
/// 期间 VM 处于 saving 状态、仍被会话锁着，此时 startvm 会报
/// "already locked by a session" —— 必须等 VMState 变为 saved 才能启动）。
fn wait_vm_saved(timeout_secs: u64) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match vm_state().as_str() {
            "saved" | "poweroff" | "aborted" => return Ok(()),
            "saving" | "restoring" | "running" => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "等待虚拟机状态保存超时（当前状态: {}），请在 VirtualBox 管理器查看",
                        vm_state()
                    ));
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
            other => {
                return Err(format!("虚拟机状态异常: {other}（请打开 VirtualBox 管理器查看）"));
            }
        }
    }
}

/// 打开虚拟机 GUI 窗口（直接使用虚拟机：登录微信、手动操作、日常使用）。
/// - 未运行：以 GUI 模式启动（VirtualBox 窗口弹出）；
/// - 无头运行中：保存状态（秒级，不关机）→ 以 GUI 模式恢复，直接看到虚拟机桌面。
#[tauri::command]
pub fn vm_open_gui() -> Result<serde_json::Value, String> {
    // 排他：等画面流/托管截图让路，避免 savestate/startvm 与截图并发争会话锁
    let _busy = VboxBusy::acquire(Duration::from_secs(30));
    if vm_running() {
        let _ = run_vbox(&["controlvm", VM_NAME, "savestate"])?;
        wait_vm_saved(120)?;
    }
    let out = run_vbox(&["startvm", VM_NAME, "--type", "gui"])?;
    Ok(json!({ "ok": true, "mode": "gui", "started": out.trim() }))
}

/// 回到无头模式（配合 AI 托管/自动回复）：保存状态 → headless 恢复。
/// 虚拟机窗口会关闭，虚拟机继续在后台运行，ClawDesk 可继续截图/操作。
#[tauri::command]
pub fn vm_close_gui() -> Result<serde_json::Value, String> {
    let _busy = VboxBusy::acquire(Duration::from_secs(30));
    if vm_running() {
        let _ = run_vbox(&["controlvm", VM_NAME, "savestate"])?;
        wait_vm_saved(120)?;
    }
    let out = run_vbox(&["startvm", VM_NAME, "--type", "headless"])?;
    Ok(json!({ "ok": true, "mode": "headless", "started": out.trim() }))
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
    chain_log(&format!(
        "📤 vm_send 开始: to={to} text=「{}」",
        text.chars().take(60).collect::<String>()
    ));
    check_writable()?;
    if text.trim().is_empty() {
        return Err("消息内容为空".into());
    }
    ensure_vnc_connected()?; // ★ 未连接时自动连接（AI 托管无需手动开面板）
    if !is_connected() {
        return Err("虚拟机 VNC 连接失败（虚拟机未运行或 VNC 服务未启动）".into());
    }
    // 白名单校验
    let list = crate::wechat_ui::whitelist_of("vm");
    if list.is_empty() {
        chain_log("❌ vm_send 白名单为空（vm_whitelist_set 未设置）");
        return Err("未设置可聊天对象（vm_whitelist_set）——AI 不允许发送消息".into());
    }
    let to_lower = to.to_lowercase();
    if !list.iter().any(|u| to_lower.contains(&u.to_lowercase())) {
        return Err(format!("{to} 不在可聊天白名单（{}）中，拒绝发送", list.join(" / ")));
    }
    // ★★ 2026-08-16 最终简化：vm_send 只做"点输入框→打字→回车发送"，
    //   不碰任何唤出/搜索/记事本中转（那些复杂流程反复触发扫码登录窗/留记事本）。
    //   ★ 前置条件（AI 职责）：调用前必须 vm_screenshot 确认微信主窗口在前台、
    //     且已打开目标联系人的聊天窗口（AI 自己用 vm_click_spot(chatN) 或搜索打开）。
    //   ★ 中文输入：type_unicode 直接打字（ASCII 标准 keysym 有效；中文尝试 Unicode keysym，
    //     若输入法在中文模式可上屏；否则 AI 可用 vm_paste_utf8 兜底）。
    // 1. 点击输入框聚焦
    let _ = vm_click_spot("input".to_string());
    crate::wechat_ui::wait_ms(800);
    // 2. 打字
    type_unicode(&text)?;
    crate::wechat_ui::wait_ms(800);
    // 3. 回车发送（微信 PC 端 Enter 发送）
    press_combo("enter")?;
    Ok(json!({
        "ok": true,
        "to": to,
        "chars": text.chars().count(),
        "note": "已通过虚拟机微信发送（Ctrl+F → 搜联系人 → 回车 → 输入 → 回车）",
    }))
}

// ─── 表情包 / 图片发送（宿主共享目录 → HTTP → 虚拟机下载 → 微信发图）───

/// 共享目录（宿主侧，AI 把表情包/图片放这里，虚拟机可下载）。
pub fn share_dir() -> std::path::PathBuf {
    vm_data_dir().join("share")
}

/// 共享文件名消毒：仅允许普通文件名（禁止路径穿越与命令注入字符）。
fn safe_share_name(raw: &str) -> Option<String> {
    // 简单 %XX 解码（URL 编码的空格等）
    let mut name = String::with_capacity(raw.len());
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                name.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        name.push(b[i] as char);
        i += 1;
    }
    let name = name.trim();
    if name.is_empty()
        || name.starts_with('.')
        || name.contains(['\\', '/', ':', '?', '#', '&'])
        || name.chars().any(|c| c.is_control())
    {
        return None;
    }
    Some(name.to_string())
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// 启动宿主共享 HTTP 服务（端口 8090，目录 <data>/share）。
/// 虚拟机通过 http://10.0.2.2:8090/文件名 下载（NAT 下宿主 = 10.0.2.2）。
/// 只绑定回环地址：NAT 的 10.0.2.2 即宿主 loopback，虚拟机可达，但局域网不可达（防扫描/穿越）。
#[tauri::command]
pub fn vm_share_serve() -> Result<serde_json::Value, String> {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_ok() {
        std::thread::spawn(|| {
            let dir = share_dir();
            let _ = std::fs::create_dir_all(&dir);
            let listener = match std::net::TcpListener::bind("127.0.0.1:8090") {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[VM] 共享 HTTP 服务启动失败: {e}");
                    return;
                }
            };
            eprintln!("[VM] 共享 HTTP 服务已启动 http://10.0.2.2:8090（目录 {}）", dir.display());
            // 并发连接上限：防 VM 内/本地程序连接洪泛打崩线程
            static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            /// 请求结束（含 return）时自动释放连接计数。
            struct ConnGuard;
            impl Drop for ConnGuard {
                fn drop(&mut self) {
                    ACTIVE.fetch_sub(1, Ordering::Relaxed);
                }
            }
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                if ACTIVE.fetch_add(1, Ordering::Relaxed) >= 8 {
                    ACTIVE.fetch_sub(1, Ordering::Relaxed);
                    continue; // 直接断开多余连接
                }
                let dir = dir.clone();
                std::thread::spawn(move || {
                    use std::io::{Read, Write};
                    let _conn = ConnGuard; // 请求结束（含 return）时释放连接计数
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let name = path.trim_start_matches('/').split(['?', '#']).next().unwrap_or("");
                    let Some(name) = safe_share_name(name) else {
                        let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
                        return;
                    };
                    let file = dir.join(&name);
                    if !file.is_file() {
                        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                        return;
                    }
                    if let Ok(data) = std::fs::read(&file) {
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            data.len()
                        );
                        let _ = s.write_all(head.as_bytes());
                        let _ = s.write_all(&data);
                    }
                });
            }
        });
    }
    Ok(json!({ "serving": true, "dir": share_dir().to_string_lossy(), "guestUrl": "http://10.0.2.2:8090" }))
}

/// 调用本地 IndexTTS2 克隆服务合成语音（vm_clone_preview / vm_tts_speak 共用）。
async fn tts_synthesize(text: &str, voice: &std::path::Path) -> Result<Vec<u8>, String> {
    if !voice.is_file() {
        return Err(format!("参考音频不存在: {}（可在面板选择音色）", voice.display()));
    }
    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:8000/tts")
        .json(&serde_json::json!({ "text": text, "voice_path": voice }))
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| format!("IndexTTS2 服务连接失败（请确认克隆服务已启动）: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("IndexTTS2 HTTP {}", resp.status()));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("读取音频失败: {e}"))
}

/// 试听克隆音色：调用本地 IndexTTS2 克隆服务合成一段语音（默认用示例女声样本）。
#[tauri::command]
pub async fn vm_clone_preview(text: Option<String>) -> Result<serde_json::Value, String> {
    let text = text.unwrap_or_else(|| "你好呀，我是你的专属语音助手，以后我就用这个声音陪你聊天啦。".into());
    let voice = voice_path();
    let bytes = tts_synthesize(&text, &voice).await?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(json!({ "audioBase64": b64, "bytes": bytes.len(), "voice": voice }))
}

/// AI 语音播放：克隆音色合成 → 播放到宿主默认播放设备（=VB-Cable → 虚拟机微信麦克风）。
/// 用于微信语音消息 / 语音通话中让 AI "开口说话"。
#[tauri::command]
pub async fn vm_tts_speak(text: String) -> Result<serde_json::Value, String> {
    if text.trim().is_empty() {
        return Err("文本为空".into());
    }
    let voice = voice_path();
    let bytes = tts_synthesize(&text, &voice).await?;
    // SoundPlayer 仅支持 WAV；临时文件带序号，避免并发调用互相覆盖
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "clawdesk_tts_speak_{}_{}.wav",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("写入音频失败: {e}"))?;
    let ps = format!(
        "$p = New-Object System.Media.SoundPlayer '{}'; $p.PlaySync()",
        tmp.to_string_lossy()
    );
    let out = std::process::Command::new("powershell")
        .creation_flags(0x08000000)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
        .output()
        .map_err(|e| format!("播放音频失败: {e}"))?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Err(format!(
            "播放失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(json!({
        "ok": true,
        "chars": text.chars().count(),
        "bytes": bytes.len(),
        "note": "已用克隆音色播放（进入 VB-Cable → 虚拟机微信麦克风）",
    }))
}

/// 把宿主共享目录里的文件拉进虚拟机（Win+R → 粘贴 powershell 下载命令 → 回车）。
/// 返回虚拟机内的目标路径（C:\Users\Administrator\Pictures\<文件名>）。
#[tauri::command]
pub fn vm_fetch_file(name: String) -> Result<serde_json::Value, String> {
    check_writable()?;
    ensure_vnc_connected()?; // ★ 未连接时自动连接
    if !is_connected() {
        return Err("虚拟机 VNC 连接失败（虚拟机未运行或 VNC 服务未启动）".into());
    }
    let name = name.trim();
    // 严格白名单：文件名会拼进 guest 机的 PowerShell 命令，任何特殊字符都是注入面
    if name.is_empty()
        || name.starts_with('.')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err("文件名不合法（只允许字母/数字/._-，且不能以 . 开头）".into());
    }
    let file = share_dir().join(name);
    if !file.is_file() {
        return Err(format!("共享目录里没有这个文件：{}（先把文件放到 {}）", name, share_dir().display()));
    }
    let guest_path = format!(r"C:\Users\Administrator\Pictures\{name}");
    let cmd = format!(
        "powershell -c \"Invoke-WebRequest -Uri http://10.0.2.2:8090/{name} -OutFile '{guest_path}'\""
    );
    // Win+R 打开运行框
    press_combo("win+r")?;
    crate::wechat_ui::wait_ms(800);
    paste_and_send(&cmd)?;
    crate::wechat_ui::wait_ms(400);
    press_combo("enter")?;
    crate::wechat_ui::wait_ms(3000);
    Ok(json!({
        "ok": true,
        "hostFile": file.to_string_lossy(),
        "guestPath": guest_path,
        "hint": "文件已下载到虚拟机。发送方法：微信聊天框点 + → 文件/照片 → 对话框按 Ctrl+L → 输入上面的路径 → 回车 → 发送",
    }))
}

fn run_vbox(args: &[&str]) -> Result<String, String> {
    let exe = "C:\\Program Files\\Oracle\\VirtualBox\\VBoxManage.exe";
    let out = std::process::Command::new(exe)
        .creation_flags(0x08000000)
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
            // ★ 2026-08-14 修复：原逻辑对 "f" 先 strip_prefix("f") 得 Some("")，
            //   "" 解析失败 → return None → ctrl+f 等组合键全部报「不支持的按键」！
            //   正确：只有 rest 非空且是 1..=12 才当 F 功能键；单字符 "f" 走 ASCII。
            if let Some(rest) = lower.strip_prefix('f') {
                if !rest.is_empty() {
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
    chain_log(&format!("⌨️ press_combo({key})"));
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
/// ★ 2026-08-16 修复：剪贴板设置后延迟 100ms 太短——VNC ClientCutText 同步到
///   VM 剪贴板需要时间，Ctrl+V 常粘出空内容 → 搜索框空 → 回车打开错误会话
///   （用户反馈"选到文件传输助手"的根因）。延迟加到 500ms。
pub fn paste_and_send(text: &str) -> Result<(), String> {
    let _ = vm_paste(text.to_string())?;
    std::thread::sleep(Duration::from_millis(500));
    press_combo("ctrl+v")
}

/// 逐字符键盘输入。
/// ★ 2026-08-16 修复：原来对所有字符（含 ASCII）都加 0x01000000 Unicode keysym，
///   导致微信自绘输入框连 ASCII 都不认（消息根本发不出去）。
///   现在：ASCII（<0x80）用标准 keysym（=ASCII 码，等价真实键盘），
///   非 ASCII 用 Unicode keysym（0x01000000+码点，仅记事本等标准控件认）。
pub fn type_unicode(text: &str) -> Result<(), String> {
    chain_log(&format!("⌨️ type_unicode: 「{}」", text.chars().take(40).collect::<String>()));
    for ch in text.chars() {
        let cp = ch as u32;
        let ks = if cp < 0x80 { cp } else { 0x0100_0000 + cp };
        vm_key(ks, true)?;
        vm_key(ks, false)?;
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

/// ★ 记事本中转剪贴板：把中文文本正确放进 VM 剪贴板（Unicode 正确）。
/// 原理：微信自绘输入框不认 Unicode keysym 直接打字，但记事本（标准 Edit 控件）
///   认。所以：记事本打字 → Ctrl+A Ctrl+C 复制（VM 剪贴板 = 正确 Unicode）→
///   关记事本 → 微信输入框 Ctrl+V 粘贴即正确中文。
#[tauri::command]
pub fn vm_paste_utf8(text: String) -> Result<serde_json::Value, String> {
    chain_log(&format!(
        "📋 vm_paste_utf8: 「{}」（{}字符，经记事本中转入剪贴板）",
        text.chars().take(40).collect::<String>(),
        text.chars().count()
    ));
    check_writable()?;
    ensure_vnc_connected()?;
    set_clipboard_utf8(&text)?;
    Ok(json!({ "ok": true, "chars": text.chars().count(), "note": "中文已通过记事本中转放入虚拟机剪贴板，用 vm_key(ctrl+v) 粘贴到微信输入框" }))
}

/// ★ 解锁虚拟机（AI 托管：检测到锁屏时调用，自动输入密码 13669403240）。
/// 2026-08-16：用户要求"锁屏让她自己开"——AI 不再干等，自己解锁后继续操作。
#[tauri::command]
pub fn vm_unlock() -> Result<serde_json::Value, String> {
    check_writable()?;
    ensure_vnc_connected()?;
    // 点击屏幕中央唤醒锁屏（若锁屏界面需要点击才出密码框）
    let _ = vm_click_spot("center".to_string());
    crate::wechat_ui::wait_ms(1000);
    // 输入密码（数字 ASCII，标准 keysym，锁屏密码框认）
    type_unicode("13669403240")?;
    crate::wechat_ui::wait_ms(500);
    press_combo("enter")?;
    crate::wechat_ui::wait_ms(3000);
    Ok(json!({ "ok": true, "note": "已尝试输入密码 13669403240 解锁" }))
}

/// ★ 视觉定位（LocateAnything-3B 本地模型）：在截图中定位目标元素，返回像素坐标。
/// 2026-08-17：解决 AI 不会精确点击的问题——用自然语言描述目标（输入框/发送按钮/搜索框），
///   LocateAnything 返回坐标框（0-1000 归一化），换算成屏幕像素后供 vm_click 点击。
/// 提示词用英文效果最佳。
#[tauri::command]
pub async fn vm_locate(target: String) -> Result<serde_json::Value, String> {
    let _t0 = std::time::Instant::now();
    chain_log(&format!("🎯 vm_locate 开始: target={target}"));
    let (path, _data_url, w, h, full_path) = vbox_screenshot_ai()?;
    // 全尺寸图定位更准（缩图会丢小目标）
    let locate_src = if std::path::Path::new(&full_path).exists() { full_path } else { path };
    let bytes = std::fs::read(&locate_src).map_err(|e| format!("读取截图失败: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    // 提示词：英文定位模板（第 6 节：Locate the {phrase}）
    let prompt = format!("Locate the {} in the screenshot. Output only the bounding box.", target.trim());
    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "model": "LocateAnything-3B-Q4_K_M.gguf",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{b64}")}}
            ]
        }],
        "stream": false,
        "max_tokens": 64
    });
    let resp = client
        .post("http://127.0.0.1:18080/v1/chat/completions")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("LocateAnything 服务连接失败（端口 18080）: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("LocateAnything HTTP {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("解析响应失败: {e}"))?;
    let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    // 解析 <box><x1><y1><x2><y2></box>（0-1000 归一化），取第一个框（纯字符串解析，无 regex 依赖）
    let nums: Vec<u32> = {
        let start = text.find("<box>").map(|i| i + 5).unwrap_or(text.len());
        let end = text[start..].find("</box>").map(|i| start + i).unwrap_or(text.len());
        text[start..end]
            .split('>')
            .filter_map(|p| p.trim().trim_start_matches('<').parse::<u32>().ok())
            .collect()
    };
    if nums.len() < 4 {
        chain_log(&format!("❌ vm_locate 无坐标框（{:.1}s，返回: {}）", _t0.elapsed().as_secs_f64(), text));
        return Err(format!("LocateAnything 未返回坐标框（返回: {text}）。请换个描述试试，英文效果最好。"));
    }
    // 0-1000 归一化 → 屏幕像素（LocateAnything 的 x 对应宽、y 对应高）
    let px = |c: u32, dim: u32| -> u32 { ((c as f64 / 1000.0) * dim as f64).round() as u32 };
    let x1 = px(nums[0], w);
    let y1 = px(nums[1], h);
    let x2 = px(nums[2], w);
    let y2 = px(nums[3], h);
    let cx = (x1 + x2) / 2;
    let cy = (y1 + y2) / 2;
    chain_log(&format!(
        "✅ vm_locate 成功（{:.1}s）: box=({x1},{y1})-({x2},{y2}) center=({cx},{cy})",
        _t0.elapsed().as_secs_f64()
    ));
    Ok(json!({
        "ok": true,
        "target": target,
        "box": [x1, y1, x2, y2],
        "center": [cx, cy],
        "x": cx,
        "y": cy,
        "width": w,
        "height": h,
        "raw": text,
        "note": "用 vm_click(x,y) 点击 center 坐标；若定位不准换英文描述重试",
    }))
}

fn set_clipboard_utf8(text: &str) -> Result<(), String> {
    // Win+R 打开运行框
    press_combo("win+r")?;
    crate::wechat_ui::wait_ms(800);
    type_unicode("notepad")?;
    crate::wechat_ui::wait_ms(500);
    press_combo("enter")?;
    crate::wechat_ui::wait_ms(2000);
    // 记事本打字（标准控件认 Unicode keysym）
    type_unicode(text)?;
    crate::wechat_ui::wait_ms(500);
    // Ctrl+A 全选 + Ctrl+C 复制
    press_combo("ctrl+a")?;
    crate::wechat_ui::wait_ms(300);
    press_combo("ctrl+c")?;
    crate::wechat_ui::wait_ms(300);
    // Alt+F4 关记事本（剪贴板已就绪）
    press_combo("alt+f4")?;
    crate::wechat_ui::wait_ms(500);
    // ★ 处理"是否保存更改？"对话框：Alt+N = 不保存（有内容时必弹）
    press_combo("alt+n")?;
    crate::wechat_ui::wait_ms(800);
    Ok(())
}
