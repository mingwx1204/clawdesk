//! Edge TTS（微软神经网络语音）合成命令 —— 免费、无需 API Key、20+ 拟人音色。
//!
//! 协议来源：开源项目 edge-tts（https://github.com/rany2/edge-tts）
//! - WebSocket 连接 speech.platform.bing.com，需自定义 headers（浏览器无法直连）
//! - Sec-MS-GEC 签名放在 URL query 参数
//! - 返回 MP3 音频（base64），前端用 Audio 播放
//!
//! 音色特点（全部 Neural 神经网络自然语音，非旧版机械音）：
//! - zh-CN-XiaoxiaoNeural 晓晓（女，温暖亲切，最拟人）
//! - zh-CN-YunxiNeural   云希（男，阳光少年）
//! - zh-CN-YunjianNeural 云健（男，沉稳成熟）
//! - 支持语气风格 style（开心/温柔/平静/严肃…）

use base64::Engine;
use futures_util::{SinkExt, TryStreamExt};
use reqwest_websocket::{Message, Upgrade};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use uuid::Uuid;

use crate::commands::AppState;

// ── 协议常量（edge-tts 移植）────────────────────────────────
const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const GEC_VERSION: &str = "1-143.0.3650.75";
const WIN_EPOCH: i64 = 11_644_473_600; // Unix → Windows 文件时间偏移（秒）
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36 Edg/143.0.0.0";
const AUDIO_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// 音色定义（前端音色下拉列表数据源）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsVoiceInfo {
    /// 微软语音 ID（如 zh-CN-XiaoxiaoNeural）
    pub id: String,
    /// 中文名（如 晓晓）
    pub name: String,
    /// 性别（女声 / 男声）
    pub gender: String,
    /// 音色描述
    pub desc: String,
    /// 地区标签（普通话 / 粤语 / 台湾 / 东北 / 陕西 / 多语言）
    pub region: String,
    /// 支持的语气风格（空 = 仅自然）
    pub styles: Vec<String>,
}

/// 便捷函数：字符串数组 → Vec<String>（避免类型推断歧义）。
fn ss(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// 内置中文/多语言音色库（从微软 voices 列表精选）。
fn builtin_voices() -> Vec<TtsVoiceInfo> {
    vec![
        TtsVoiceInfo { id: "zh-CN-XiaoxiaoNeural".into(), name: "晓晓".into(), gender: "女声".into(), desc: "温暖亲切，最接近真人，适合日常聊天".into(), region: "普通话".into(), styles: ss(&["cheerful","empathetic","calm","gentle","serious","newscast","sad","angry","excited","poetry-reading"]) },
        TtsVoiceInfo { id: "zh-CN-XiaoyiNeural".into(), name: "晓伊".into(), gender: "女声".into(), desc: "活泼可爱，少女感".into(), region: "普通话".into(), styles: ss(&["cheerful","empathetic","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-YunxiNeural".into(), name: "云希".into(), gender: "男声".into(), desc: "阳光少年，清爽有活力".into(), region: "普通话".into(), styles: ss(&["cheerful","angry","sad","excited","fearful","serious","gentle","lyrical"]) },
        TtsVoiceInfo { id: "zh-CN-YunjianNeural".into(), name: "云健".into(), gender: "男声".into(), desc: "沉稳成熟，像邻家大哥".into(), region: "普通话".into(), styles: ss(&["cheerful","angry","sad","serious","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-YunxiaNeural".into(), name: "云夏".into(), gender: "男声".into(), desc: "青春活力，少年音".into(), region: "普通话".into(), styles: ss(&["cheerful","angry","sad","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-YunyangNeural".into(), name: "云扬".into(), gender: "男声".into(), desc: "专业播音腔，适合新闻播报".into(), region: "普通话".into(), styles: ss(&["newscast","serious"]) },
        TtsVoiceInfo { id: "zh-CN-XiaochenNeural".into(), name: "晓辰".into(), gender: "女声".into(), desc: "清澈明亮，邻家女孩".into(), region: "普通话".into(), styles: ss(&["cheerful","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-XiaohanNeural".into(), name: "晓涵".into(), gender: "女声".into(), desc: "甜美温柔，轻声细语".into(), region: "普通话".into(), styles: ss(&["cheerful","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-XiaomengNeural".into(), name: "晓梦".into(), gender: "女声".into(), desc: "亲切自然，讲故事风格".into(), region: "普通话".into(), styles: ss(&["cheerful","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-XiaomoNeural".into(), name: "晓墨".into(), gender: "女声".into(), desc: "成熟知性，职场女性".into(), region: "普通话".into(), styles: ss(&["cheerful","serious","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-XiaoruiNeural".into(), name: "晓睿".into(), gender: "女声".into(), desc: "温柔睿智，成熟可靠".into(), region: "普通话".into(), styles: ss(&["cheerful","serious"]) },
        TtsVoiceInfo { id: "zh-CN-XiaoshuangNeural".into(), name: "晓双".into(), gender: "女声".into(), desc: "活泼儿童音，适合童话故事".into(), region: "普通话".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-CN-XiaoxuanNeural".into(), name: "晓萱".into(), gender: "女声".into(), desc: "亲切柔和，主播音质".into(), region: "普通话".into(), styles: ss(&["cheerful","gentle"]) },
        TtsVoiceInfo { id: "zh-CN-XiaoyanNeural".into(), name: "晓颜".into(), gender: "女声".into(), desc: "优美动听，文艺范儿".into(), region: "普通话".into(), styles: ss(&["cheerful","gentle","sad"]) },
        TtsVoiceInfo { id: "zh-CN-liaoning-XiaobeiNeural".into(), name: "晓北".into(), gender: "女声".into(), desc: "东北口音，豪爽有趣".into(), region: "东北话".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-CN-shaanxi-XiaoniNeural".into(), name: "晓妮".into(), gender: "女声".into(), desc: "陕西口音，方言特色".into(), region: "陕西话".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-HK-HiuGaaiNeural".into(), name: "曉佳".into(), gender: "女声".into(), desc: "粤语女声，亲切自然".into(), region: "粤语".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-HK-HiuMaanNeural".into(), name: "曉曼".into(), gender: "女声".into(), desc: "粤语女声，温柔清晰".into(), region: "粤语".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-HK-WanLungNeural".into(), name: "雲龍".into(), gender: "男声".into(), desc: "粤语男声，沉稳有力".into(), region: "粤语".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-TW-HsiaoChenNeural".into(), name: "曉臻".into(), gender: "女声".into(), desc: "台湾国语女声，甜美".into(), region: "台湾".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-TW-HsiaoYuNeural".into(), name: "曉雨".into(), gender: "女声".into(), desc: "台湾国语女声，活泼".into(), region: "台湾".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "zh-TW-YunJheNeural".into(), name: "雲哲".into(), gender: "男声".into(), desc: "台湾国语男声，温和".into(), region: "台湾".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "en-US-EmmaMultilingualNeural".into(), name: "Emma".into(), gender: "女声".into(), desc: "多语言女声，中英文混读自然（默认推荐）".into(), region: "多语言".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "en-US-AndrewMultilingualNeural".into(), name: "Andrew".into(), gender: "男声".into(), desc: "多语言男声，中英文混读自然".into(), region: "多语言".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "en-US-AvaMultilingualNeural".into(), name: "Ava".into(), gender: "女声".into(), desc: "多语言女声，清晰专业".into(), region: "多语言".into(), styles: ss(&[]) },
        TtsVoiceInfo { id: "en-US-BrianMultilingualNeural".into(), name: "Brian".into(), gender: "男声".into(), desc: "多语言男声，沉稳专业".into(), region: "多语言".into(), styles: ss(&[]) },
    ]
}

/// 语气风格中文名映射（前端展示）。
#[allow(dead_code)]
pub fn style_label(style: &str) -> &'static str {
    match style {
        "cheerful" => "😄 开心",
        "empathetic" => "💗 温柔共情",
        "calm" => "😌 平静",
        "gentle" => "🌸 温和",
        "serious" => "📌 严肃",
        "newscast" => "📰 新闻播报",
        "sad" => "😢 悲伤",
        "angry" => "😠 生气",
        "excited" => "🎉 兴奋",
        "fearful" => "😨 害怕",
        "lyrical" => "🎵 抒情",
        "poetry-reading" => "📖 诗歌朗诵",
        _ => "🌿 自然（无语气）",
    }
}

/// 生成 Sec-MS-GEC 签名（URL query 参数）。
/// ticks = (unix + WIN_EPOCH) 取整到 5 分钟 → ×1e7 → 拼接 token → SHA256 大写 hex。
fn generate_sec_ms_gec() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut ticks = now + WIN_EPOCH;
    ticks -= ticks % 300; // 向下取整到 5 分钟
    let ticks_100ns = ticks * 10_000_000;
    let str_to_hash = format!("{ticks_100ns}{TRUSTED_CLIENT_TOKEN}");
    let digest = Sha256::digest(str_to_hash.as_bytes());
    let hex = digest.iter().map(|b| format!("{:02X}", b)).collect::<String>();
    eprintln!("[TTS] Sec-MS-GEC ticks={ticks_100ns}");
    hex
}

/// JS 风格时间戳（如 "Thu, 01 Jan 2026 00:00:00 GMT+0000 (Coordinated Universal Time)"）。
fn date_to_string() -> String {
    use chrono::{Datelike, Timelike, Utc};
    let dt = Utc::now();
    let wd = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][dt.weekday().num_days_from_sunday() as usize];
    let mon = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(dt.month() - 1) as usize];
    format!(
        "{wd}, {day:02} {mon} {year} {hh:02}:{mm:02}:{ss:02} GMT+0000 (Coordinated Universal Time)",
        day = dt.day(),
        year = dt.year(),
        hh = dt.hour(),
        mm = dt.minute(),
        ss = dt.second(),
    )
}

/// 移除服务端不支持的字符（垂直制表符等，OCR 文本常见）。
fn clean_text(text: &str) -> String {
    text.chars()
        .map(|c| {
            let code = c as u32;
            if (0..=8).contains(&code) || (11..=12).contains(&code) || (14..=31).contains(&code) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// XML 转义。
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 生成 SSML（支持语气风格 express-as）。
fn make_ssml(voice: &str, text: &str, rate_percent: i32, style: &str) -> String {
    let escaped = escape_xml(&clean_text(text));
    let rate = format!("{rate_percent:+}%");
    let voice_part = format!(
        "<voice name='{voice}'><prosody pitch='+0Hz' rate='{rate}' volume='+0%'>{escaped}</prosody></voice>"
    );
    if !style.is_empty() {
        // 语气风格需要 mstts 命名空间
        format!(
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xmlns:mstts='http://www.w3.org/2001/mstts' xml:lang='en-US'><voice name='{voice}'><mstts:express-as style='{style}'>{escaped}</mstts:express-as></voice></speak>"
        )
    } else {
        format!(
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>{voice_part}</speak>"
        )
    }
}

/// 合成结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsResult {
    /// base64 编码的 MP3 音频
    pub audio_base64: String,
    /// 音频字节数
    pub bytes: usize,
    /// 使用的音色 ID
    pub voice: String,
}

/// 列出内置音色（设置界面下拉数据源）。
#[tauri::command]
pub fn tts_list_voices() -> Vec<TtsVoiceInfo> {
    builtin_voices()
}

/// Edge TTS 文本合成 → base64 MP3。
///
/// - text:  要朗读的文本
/// - voice: 音色 ID（默认 zh-CN-XiaoxiaoNeural 晓晓）
/// - rate:  语速倍率 0.5~2.0（1.0 = 正常）
/// - style: 语气风格（空 = 自然；如 cheerful / gentle / serious…）
#[tauri::command]
pub async fn tts_speak(
    _state: State<'_, AppState>,
    text: String,
    voice: Option<String>,
    rate: Option<f64>,
    style: Option<String>,
) -> Result<TtsResult, String> {
    let voice = voice.unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".into());
    let rate = rate.unwrap_or(1.0).clamp(0.5, 2.0);
    let style = style.unwrap_or_default();
    let audio = synthesize_audio(&text, &voice, rate, &style).await?;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio);
    eprintln!("[TTS] ✅ 合成完成: {} bytes", audio.len());
    Ok(TtsResult {
        audio_base64: audio_b64,
        bytes: audio.len(),
        voice,
    })
}

/// Edge TTS 合成核心（返回 MP3 字节）。供 tts_speak 与微信语音回复（wechat_send_voice）复用。
/// 校验音色/风格合法性（防 SSML 注入）、长文本截断。
pub(crate) async fn synthesize_audio(
    text: &str,
    voice: &str,
    rate: f64,
    style: &str,
) -> Result<Vec<u8>, String> {
    // 校验音色 ID 合法性（防注入 SSML）
    if !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("非法的音色 ID".into());
    }
    let rate_percent = ((rate.clamp(0.5, 2.0) - 1.0) * 100.0).round() as i32;
    // 风格白名单（防 SSML 注入）
    let style = if style.is_empty() || !style.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        String::new()
    } else {
        style.to_string()
    };

    let text = text.trim();
    if text.is_empty() {
        return Err("文本为空".into());
    }
    // 长文本限制（服务端单次 SSML 限制约 4096 字节，超长取前 3000 字符并提示）
    let text: String = text.chars().take(3000).collect();

    eprintln!("[TTS] 合成开始: voice={voice} rate={rate_percent}% style={style:?} text_len={}", text.chars().count());

    let conn_id = Uuid::new_v4().simple().to_string();
    let gec = generate_sec_ms_gec();
    let url = format!(
        "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1?TrustedClientToken={TRUSTED_CLIENT_TOKEN}&ConnectionId={conn_id}&Sec-MS-GEC={gec}&Sec-MS-GEC-Version={GEC_VERSION}"
    );

    // 建立连接（带浏览器指纹 headers，服务端校验）
    // reqwest-websocket 0.6 API：upgrade() → send() 完成握手 → into_websocket() 取 WebSocket
    let client = reqwest::Client::new();
    let mut ws = client
        .get(&url)
        .header("Pragma", "no-cache")
        .header("Cache-Control", "no-cache")
        .header("Origin", "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold")
        .header("User-Agent", USER_AGENT)
        .header("Accept-Encoding", "gzip, deflate, br, zstd")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Cookie", format!("muid={};", Uuid::new_v4().simple()))
        .upgrade()
        .send()
        .await
        .map_err(|e| format!("连接 Edge TTS 失败: {e}"))?
        .into_websocket()
        .await
        .map_err(|e| format!("连接 Edge TTS 失败: {e}"))?;

    // 1. speech.config
    let config_body = format!(
        "{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"true\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"{AUDIO_FORMAT}\"}}}}}}}}"
    );
    let config_msg = format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{}\r\n",
        date_to_string(),
        config_body
    );
    ws.send(Message::Text(config_msg))
        .await
        .map_err(|e| format!("发送 config 失败: {e}"))?;

    // 2. SSML
    let ssml = make_ssml(&voice, &text, rate_percent, &style);
    let request_id = Uuid::new_v4().simple().to_string();
    let ssml_msg = format!(
        "X-RequestId:{request_id}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n{ssml}",
        date_to_string()
    );
    ws.send(Message::Text(ssml_msg))
        .await
        .map_err(|e| format!("发送 SSML 失败: {e}"))?;

    // 3. 接收音频帧（BINARY：前 2 字节 = header 长度，随后 headers + 音频数据）
    let mut audio: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);

    while let Some(msg) = tokio::time::timeout_at(deadline, ws.try_next())
        .await
        .map_err(|_| "Edge TTS 合成超时（30s）".to_string())?
        .map_err(|e| format!("接收音频失败: {e}"))?
    {
        match msg {
            Message::Text(t) => {
                if t.contains("Path:turn.end") {
                    break;
                }
            }
            Message::Binary(bytes) => {
                if bytes.len() < 2 {
                    continue;
                }
                let header_len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
                if 2 + header_len > bytes.len() {
                    continue;
                }
                let headers = String::from_utf8_lossy(&bytes[2..2 + header_len]);
                if headers.contains("Content-Type:audio") || headers.contains("Path:audio") {
                    audio.extend_from_slice(&bytes[2 + header_len..]);
                }
            }
            Message::Close { .. } => break,
            _ => {}
        }
    }

    if audio.is_empty() {
        return Err("未收到音频数据（请检查音色 ID 与网络）".into());
    }
    Ok(audio)
}

// ─── CosyVoice 2（硅基流动 / OpenAI 兼容 API）───
// 真人级开源音色（阿里 FunAudioLLM/CosyVoice2-0.5B），免费额度，比 Edge TTS 自然很多。
// 文档：https://docs.siliconflow.cn/cn/api-reference/audio/create-speech

/// CosyVoice 2 音色列表（硅基流动预置，真人实录级主播音色）
#[allow(dead_code)]
pub fn cosyvoice_voices() -> Vec<(&'static str, &'static str)> {
    vec![
        ("alex", "Alex · 男声 沉稳磁性"),
        ("anna", "Anna · 女声 温暖自然"),
        ("bella", "Bella · 女声 甜美"),
        ("eric", "Eric · 男声 阳光"),
        ("jason", "Jason · 男声 成熟"),
        ("lily", "Lily · 女声 清新"),
        ("maria", "Maria · 女声 优雅"),
        ("roger", "Roger · 男声 浑厚"),
        ("sarah", "Sarah · 女声 亲切"),
        ("steve", "Steve · 男声 低沉"),
    ]
}

/// CosyVoice 2 合成（硅基流动 OpenAI 兼容 /v1/audio/speech）。
/// - api_key: 硅基流动 API Key（sk-...，控制台免费申请）
/// - voice: 音色名（如 anna / alex；传空默认 anna）
/// - 返回 mp3 字节
pub(crate) async fn synthesize_audio_cosyvoice(
    text: &str,
    voice: &str,
    api_key: &str,
) -> Result<Vec<u8>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("文本为空".into());
    }
    let text: String = text.chars().take(1000).collect(); // CosyVoice 单次限长
    let voice = if voice.is_empty() { "anna" } else { voice };
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.siliconflow.cn/v1/audio/speech")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "model": "FunAudioLLM/CosyVoice2-0.5B",
            "input": text,
            "voice": voice,
            "response_format": "mp3",
            "speed": 1.0,
        }))
        .send()
        .await
        .map_err(|e| format!("CosyVoice 请求失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("CosyVoice HTTP {status}: {}", crate::wechat::trunc_chars(&body, 200)));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("读取音频失败: {e}"))?;
    if bytes.is_empty() {
        return Err("CosyVoice 返回空音频".into());
    }
    Ok(bytes.to_vec())
}

// ─── IndexTTS2 本地声音克隆（方案 B）───
// 本地 FastAPI 服务（indexTTS2/server.py，默认 http://127.0.0.1:8000）。
// 零样本克隆：给一段参考音频（voice_path），AI 就用这个音色说话。

/// IndexTTS2 本地服务合成（POST /tts → wav 字节）。
/// - base_url: 本地服务地址（默认 http://127.0.0.1:8000）
/// - voice_path: 参考音频绝对路径（"诗妍.wav"等，10~30 秒清晰人声）
/// - 返回 wav 字节
pub(crate) async fn synthesize_audio_indextts(
    text: &str,
    voice_path: &str,
    base_url: &str,
) -> Result<Vec<u8>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("文本为空".into());
    }
    let text: String = text.chars().take(500).collect();
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/tts", base_url.trim_end_matches('/')))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "text": text,
            "voice_path": voice_path,
        }))
        .timeout(std::time::Duration::from_secs(120)) // 模型推理较慢，120s 超时
        .send()
        .await
        .map_err(|e| format!("IndexTTS2 服务请求失败（请确认本地服务已启动）: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("IndexTTS2 HTTP {status}: {}", crate::wechat::trunc_chars(&body, 200)));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("读取音频失败: {e}"))?;
    if bytes.is_empty() {
        return Err("IndexTTS2 返回空音频".into());
    }
    Ok(bytes.to_vec())
}
