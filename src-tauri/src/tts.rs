//! TTS 语音朗读 — 基于 Win32 SAPI (ISpVoice COM)
//!
//! 为什么用 Win32 SAPI 而不是 WebView2 speechSynthesis：
//! WebView2 的 speechSynthesis 首次初始化后缓存音频输出设备，不跟随系统
//! "默认播放设备"的切换（表现为：用户切到耳机仍从扬声器播放，禁用音响才停）。
//! Win32 SAPI 的 ISpVoice 每次 SetOutput(NULL) 都使用系统当前默认音频端点，
//! 因此能正确跟随耳机/扬声器切换。
//!
//! 支持音色：枚举系统已安装语音（Microsoft Huihui/Kangkang/晓晓等），
//! 用户可切换音色，Speak 前按名称匹配 token 并 SetVoice。

use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Media::Speech::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::WaitForSingleObject;

/// 语音信息（前端展示用）
#[derive(Serialize, Clone)]
pub struct VoiceInfo {
    /// 语音名称（如 "Microsoft Huihui Desktop"）
    pub name: String,
    /// 语言（如 "zh-CN"）
    pub lang: String,
}

enum TtsMsg {
    Speak(String),
    /// 切换音色（传语音名称，空=自动中文）
    SetVoice(String),
    Stop,
}

/// 全局 TTS 线程发送端
static TTS_SENDER: Mutex<Option<Sender<TtsMsg>>> = Mutex::new(None);

/// 启动 TTS 后台线程（仅一次）
fn ensure_tts_thread() {
    let mut guard = TTS_SENDER.lock().unwrap();
    if guard.is_some() {
        return;
    }
    let (tx, rx) = channel();
    *guard = Some(tx);
    std::thread::spawn(move || {
        tts_loop(rx);
    });
}

/// 把 Rust String 转为 UTF-16 宽字符 Vec（SAPI 需要）
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 打开语音 token 类别并枚举
fn open_voice_category() -> Option<ISpObjectTokenCategory> {
    unsafe {
        let cat: Result<ISpObjectTokenCategory, _> =
            CoCreateInstance::<_, ISpObjectTokenCategory>(&SpObjectTokenCategory, None, CLSCTX_ALL);
        let cat = cat.ok()?;
        let cat_key = to_wide("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech\\Voices\\Tokens");
        cat.SetId(PCWSTR(cat_key.as_ptr()), false).ok()?;
        Some(cat)
    }
}

/// 枚举系统所有语音
fn enum_voices() -> Vec<VoiceInfo> {
    let mut out = Vec::new();
    let cat = match open_voice_category() {
        Some(c) => c,
        None => return out,
    };
    unsafe {
        if let Ok(tokens) = cat.EnumTokens(PCWSTR(std::ptr::null()), PCWSTR(std::ptr::null())) {
            loop {
                let mut token: Option<ISpObjectToken> = None;
                if tokens.Next(1, &mut token, None).is_err() || token.is_none() {
                    break;
                }
                if let Some(t) = &token {
                    // 读取名称（空键名返回 token 名称）和语言
                    let name = t
                        .GetStringValue(PCWSTR(to_wide("").as_ptr()))
                        .ok()
                        .and_then(|s| s.to_string().ok())
                        .unwrap_or_default();
                    let lang = t
                        .GetStringValue(PCWSTR(to_wide("Language").as_ptr()))
                        .ok()
                        .and_then(|s| s.to_string().ok())
                        .unwrap_or_default();
                    if !name.is_empty() {
                        out.push(VoiceInfo { name, lang });
                    }
                }
            }
        }
    }
    out
}

/// 按名称选择语音 token（空名称=自动选中文），找不到返回 None
fn pick_token_by_name(name: &str) -> Option<ISpObjectToken> {
    let cat = open_voice_category()?;
    unsafe {
        let tokens = cat
            .EnumTokens(PCWSTR(std::ptr::null()), PCWSTR(std::ptr::null()))
            .ok()?;
        let want_auto = name.trim().is_empty();
        let mut fallback_zh: Option<ISpObjectToken> = None;
        loop {
            let mut token: Option<ISpObjectToken> = None;
            if tokens.Next(1, &mut token, None).is_err() || token.is_none() {
                break;
            }
            if let Some(t) = &token {
                let token_name = t
                    .GetStringValue(PCWSTR(to_wide("").as_ptr()))
                    .ok()
                    .and_then(|s| s.to_string().ok())
                    .unwrap_or_default();
                let lang = t
                    .GetStringValue(PCWSTR(to_wide("Language").as_ptr()))
                    .ok()
                    .and_then(|s| s.to_string().ok())
                    .unwrap_or_default();
                if !want_auto && token_name == name {
                    return token;
                }
                if want_auto && lang.starts_with("zh") && fallback_zh.is_none() {
                    fallback_zh = token;
                }
            }
        }
        if want_auto {
            fallback_zh
        } else {
            None
        }
    }
}

/// 后台线程主循环（持有 ISpVoice）
fn tts_loop(rx: Receiver<TtsMsg>) {
    let mut voice: Option<ISpVoice> = None;
    let mut current_voice: String = String::new(); // 当前音色名，空=自动中文

    // 初始化 COM（单线程套间）
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    while let Ok(msg) = rx.recv() {
        match msg {
            TtsMsg::Speak(text) => {
                // 惰性初始化 SAPI voice
                if voice.is_none() {
                    match unsafe { CoCreateInstance::<_, ISpVoice>(&SpVoice, None, CLSCTX_ALL) } {
                        Ok(v) => {
                            // 关键：SetOutput(None) 使用系统当前默认音频设备
                            let _ = unsafe { v.SetOutput(None, false) };
                            voice = Some(v);
                        }
                        Err(e) => {
                            eprintln!("[TTS] SAPI 初始化失败: {e:?}");
                            continue;
                        }
                    }
                }
                // 应用当前音色
                if let Some(v) = &voice {
                    if let Some(token) = pick_token_by_name(&current_voice) {
                        let _ = unsafe { v.SetVoice(&token) };
                    }
                }
                if let Some(v) = &voice {
                    let wide = to_wide(&text);
                    // SPF_ASYNC 异步朗读，不阻塞线程
                    let hr = unsafe { v.Speak(PCWSTR(wide.as_ptr()), SPF_ASYNC.0 as u32, None) };
                    if hr.is_err() {
                        eprintln!("[TTS] Speak 失败: {hr:?}");
                        continue;
                    }
                    // 等待朗读完成，期间检测 Stop 消息
                    let complete = unsafe { v.SpeakCompleteEvent() };
                    loop {
                        let wait = unsafe { WaitForSingleObject(complete, 300) };
                        if wait == WAIT_OBJECT_0 {
                            break; // 朗读完成
                        }
                        // 检查是否有停止请求
                        match rx.try_recv() {
                            Ok(TtsMsg::Stop) => {
                                let empty = to_wide("");
                                let _ = unsafe {
                                    v.Speak(PCWSTR(empty.as_ptr()), SPF_PURGEBEFORESPEAK.0 as u32, None)
                                };
                                break;
                            }
                            Err(TryRecvError::Disconnected) => break,
                            Err(TryRecvError::Empty) => continue,
                            Ok(_) => continue,
                        }
                    }
                    unsafe { let _ = CloseHandle(complete); }
                }
            }
            TtsMsg::SetVoice(name) => {
                current_voice = name;
                // 立即切换音色
                if let Some(v) = &voice {
                    if let Some(token) = pick_token_by_name(&current_voice) {
                        let _ = unsafe { v.SetVoice(&token) };
                    }
                }
            }
            TtsMsg::Stop => {
                if let Some(v) = &voice {
                    // SPF_PURGEBEFORESPEAK 清空队列并停止当前朗读
                    let empty = to_wide("");
                    let _ = unsafe {
                        v.Speak(PCWSTR(empty.as_ptr()), SPF_PURGEBEFORESPEAK.0 as u32, None)
                    };
                }
            }
        }
    }

    unsafe {
        let _ = CoUninitialize();
    }
}

/// 朗读一段文本（自动跟随系统当前默认播放设备）
#[tauri::command]
pub async fn tts_speak(app: AppHandle, text: String) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    ensure_tts_thread();
    let sender = TTS_SENDER.lock().unwrap().clone();
    if let Some(tx) = sender {
        let _ = tx.send(TtsMsg::Speak(text));
        let _ = app.emit("tts-state", "speaking");
    }
    Ok(())
}

/// 切换音色（传语音名称，空=自动中文）
#[tauri::command]
pub fn tts_set_voice(voice: String) -> Result<(), String> {
    ensure_tts_thread();
    let sender = TTS_SENDER.lock().unwrap().clone();
    if let Some(tx) = sender {
        let _ = tx.send(TtsMsg::SetVoice(voice));
    }
    Ok(())
}

/// 枚举系统可用音色
#[tauri::command]
pub fn tts_list_voices() -> Result<Vec<VoiceInfo>, String> {
    Ok(enum_voices())
}

/// 停止朗读
#[tauri::command]
pub async fn tts_stop(app: AppHandle) -> Result<(), String> {
    let sender = TTS_SENDER.lock().unwrap().clone();
    if let Some(tx) = sender {
        let _ = tx.send(TtsMsg::Stop);
        let _ = app.emit("tts-state", "stopped");
    }
    Ok(())
}
