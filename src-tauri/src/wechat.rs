//! 微信 ClawBot 接入模块 — 腾讯 iLink Bot API（官方扫码登录，纯 HTTP，无外部服务）
//!
//! 架构：微信用户 ↔ 腾讯 iLink Bot（ilinkai.weixin.qq.com）↔ ClawDesk 桌面端
//! 协议参考：@tencent-weixin/openclaw-weixin@2.4.6（MIT 开源，腾讯官方出品）
//!
//! 登录：get_bot_qrcode 获取二维码 -> 手机微信扫码 -> get_qrcode_status 长轮询确认
//! 消息：getupdates 长轮询接收用户消息 -> sendmessage 发送 AI 回复
//! 持久化：bot_token 保存到 app_data_dir/wechat_ilink.json，重启后自动续连
//!
//! 移植说明（旧版 clawdesk → 新版 Vue/Tauri）：
//! - 错误类型从 crate::error::AppError 适配为 Result<T, String>（新版命令惯例）
//! - 其余协议 / 加密 / 循环逻辑与旧版完全一致（旧版已实测打通）

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use aes::Aes128;
use cipher::generic_array::GenericArray;
use cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use futures_util::FutureExt;
use md5::{Digest, Md5};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

/// 新版命令统一错误类型（兼容 tauri 命令 Result<_, String> 惯例）
pub(crate) type AppResult<T> = Result<T, String>;

// ─── iLink 常量（协议固定值） ───
const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
const ILINK_APP_ID: &str = "bot";
const ILINK_BOT_TYPE: &str = "3";
const CHANNEL_VERSION: &str = "2.4.6";
/// iLink-App-ClientVersion：0x00MMNNPP 编码（2.4.6 -> 0x020406 = 131590）
const ILINK_CLIENT_VERSION: u32 = ((2 & 0xff) << 16) | ((4 & 0xff) << 8) | (6 & 0xff);
/// 二维码状态长轮询超时
const QR_POLL_TIMEOUT: Duration = Duration::from_secs(35);
/// getUpdates 长轮询超时（服务器 hold 期间）
const GETUPDATES_TIMEOUT: Duration = Duration::from_secs(35);
/// 二维码会话有效期
const LOGIN_TTL_MS: u64 = 5 * 60_000;
/// 消息类型 / 条目类型
const MSG_TYPE_USER: i64 = 1;
const MSG_TYPE_BOT: i64 = 2;
const ITEM_TYPE_TEXT: i64 = 1;
/// 条目类型（openclaw-weixin 协议）：2=图片 3=语音 4=文件 5=视频
const ITEM_TYPE_IMAGE: i64 = 2;
const ITEM_TYPE_VOICE: i64 = 3;
const ITEM_TYPE_FILE: i64 = 4;
const ITEM_TYPE_VIDEO: i64 = 5;
/// 微信 CDN 下载基础地址（openclaw CDN_BASE_URL）
const CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

// ─── 数据结构 ───

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WechatMessage {
    pub msg_id: String,
    pub from_user: String,
    pub content: String,
    pub msg_type: String,
    pub timestamp: u64,
    /// 所属微信槽位（0 = 微信1，1 = 微信2 …），每个槽位独立 AI 会话/人设/聊天记录
    #[serde(default)]
    pub bot_slot: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    /// 图片本地路径（已下载解密到附件目录，AI 用 analyze_image 读取）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    /// 文件/语音/视频本地路径（AI 用 file_read 读取）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<String>>,
    /// 语音云端转写文本（腾讯服务器已转好；已拼入 content 的 `[语音] …`，单独字段供前端标记）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_transcript: Option<String>,
}

/// 进行中的二维码登录会话
#[derive(Debug, Clone)]
pub(crate) struct QrSession {
    qrcode: String,
    qrcode_url: String,
    started_at: u64,
    pending_verify_code: Option<String>,
    polling_base: String,
}

/// 持久化的账号文件（DPAPI 加密落盘）
/// 含 get_updates 游标（sync_buf）与各用户 context_token：重启后断点续拉，不丢消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountFile {
    token: String,
    bot_id: String,
    base_url: String,
    user_id: String,
    /// get_updates 同步游标（base64 字符串），重启后从断点续拉
    #[serde(default)]
    sync_buf: String,
    /// from_user_id -> 最近 context_token（回复必须携带），重启后无需等新消息即可回复
    #[serde(default)]
    context_tokens: std::collections::HashMap<String, String>,
}

/// Bot 内部状态（跨线程共享）—— 每个微信槽位一个实例，互不干扰
pub(crate) struct WechatInner {
    /// 槽位号（0 = 微信1 …），决定账号文件/人设/聊天记录目录
    pub slot: usize,
    pub running: AtomicBool,
    pub connected: AtomicBool,
    pub last_poll: Mutex<u64>,
    pub msg_count: Mutex<u64>,
    pub token: Mutex<Option<String>>,
    pub bot_id: Mutex<Option<String>>,
    pub base_url: Mutex<Option<String>>,
    pub user_id: Mutex<Option<String>>,
    pub get_updates_buf: Mutex<String>,
    /// from_user_id -> 最近 context_token（回复必须携带）
    pub context_map: Mutex<HashMap<String, String>>,
    /// 登录时从 getconfig 获取，用于发送"正在输入"状态
    pub typing_ticket: Mutex<Option<String>>,
    /// per-user typing ticket 缓存（user_id -> ticket）：ticket 必须带
    /// 该用户的 context_token 单独获取，且有 TTL（60s）需定期刷新
    pub typing_tickets: Mutex<HashMap<String, TypingTicketEntry>>,
    /// typing 保活任务（AI 生成期间每 10s 持续发送"正在输入"）
    pub typing_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// typing 当前目标用户（None 表示无保活任务）
    pub typing_target: Mutex<Option<String>>,
    pub qr_session: Mutex<Option<QrSession>>,
    pub data_dir: Mutex<Option<PathBuf>>,
    /// 该微信的人设（system prompt 文本，可随时修改，AI 回复时注入）
    pub persona: Mutex<Option<String>>,
    /// 聊天白名单（from_user_id 列表；空 = 不限制，只和这些人聊天）
    /// 配置后，白名单外的用户发消息会被忽略（不自动回复、不主动聊天）
    pub allowed_users: Mutex<Vec<String>>,
    /// AI 语音音色 ID（Edge TTS；用于语音回复，空 = 默认晓晓）
    pub voice_id: Mutex<Option<String>>,
    /// 语音引擎（edge / cosyvoice / indextts；indextts 用本地 IndexTTS2 声音克隆）
    pub voice_engine: Mutex<String>,
    /// 硅基流动 API Key（CosyVoice 用；空则回退 Edge TTS）
    pub cosyvoice_api_key: Mutex<Option<String>>,
    /// IndexTTS2 本地服务地址（如 http://127.0.0.1:8000）
    pub indextts_url: Mutex<Option<String>>,
    /// IndexTTS2 参考音频路径（声音克隆的母版，如 D:\...\诗妍.wav）
    pub indextts_voice_path: Mutex<Option<String>>,
    /// 聊天记录 JSONL 文件路径（D:\ClawDeskData\wechat\slot{N}\history.jsonl）
    pub history_path: Mutex<Option<PathBuf>>,
    /// 主动聊天开关（Bot 主动找用户聊）
    pub proactive_enabled: AtomicBool,
    /// 主动聊天随机间隔下限（分钟，默认 30）
    pub proactive_interval_min: Mutex<u64>,
    /// 主动聊天随机间隔上限（分钟，默认 180）
    pub proactive_interval_max: Mutex<u64>,
    /// 上次主动聊天时间戳（毫秒）
    pub proactive_last_at: Mutex<u64>,
    /// 主动聊天的目标用户（from_user_id；空则不发）
    pub proactive_target: Mutex<Option<String>>,
    /// 上次主动发送的消息内容（供下次生成时回顾，避免重复/人格分裂）
    pub proactive_last_msg: Mutex<Option<String>>,
    /// 连续空闲轮数（AI 发言占比过高/用户未回复时的退避计数，用户回复后重置）
    pub proactive_idle_rounds: Mutex<u64>,
    /// 用户最近一次发消息的时间戳（毫秒）：热聊检测依据。
    /// 30 分钟内用户回过消息 → 进入热聊模式（短间隔续话）；超过则自然冷却回普通模式。
    pub last_user_msg_at: Mutex<u64>,
    /// 最近处理过的消息 ID 环形队列（去重：防止 getupdates 重复投递导致重复回复）
    pub last_msg_ids: Mutex<std::collections::VecDeque<String>>,
    pub shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// getupdates 循环当前使用的 token（用于 start 时判断旧循环是否已过期需让位）
    pub loop_token: Mutex<Option<String>>,
    /// getupdates 循环代数（防旧循环退出清理误清新循环的 shutdown 槽位）
    pub loop_gen: AtomicU64,
    /// 主动聊天停止信号（stop/登出时置 true，proactive_loop 每轮检查）
    pub proactive_stop: AtomicBool,
    /// 主动聊天循环任务句柄（防重入：已存在则不重复启动）
    pub proactive_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl WechatInner {
    fn new(slot: usize) -> Self {
        Self {
            slot,
            running: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            last_poll: Mutex::new(0),
            msg_count: Mutex::new(0),
            token: Mutex::new(None),
            bot_id: Mutex::new(None),
            base_url: Mutex::new(None),
            user_id: Mutex::new(None),
            get_updates_buf: Mutex::new(String::new()),
            context_map: Mutex::new(HashMap::new()),
            typing_ticket: Mutex::new(None),
            typing_tickets: Mutex::new(HashMap::new()),
            typing_task: Mutex::new(None),
            typing_target: Mutex::new(None),
            qr_session: Mutex::new(None),
            data_dir: Mutex::new(None),
            persona: Mutex::new(None),
            allowed_users: Mutex::new(Vec::new()),
            voice_id: Mutex::new(None),
            voice_engine: Mutex::new("edge".to_string()),
            cosyvoice_api_key: Mutex::new(None),
            indextts_url: Mutex::new(None),
            indextts_voice_path: Mutex::new(None),
            history_path: Mutex::new(None),
            // ★ 默认开启：主动聊天默认全开（自带静默机制/存在感惩罚/深夜不打扰）
            proactive_enabled: AtomicBool::new(true),
            proactive_interval_min: Mutex::new(1),
            proactive_interval_max: Mutex::new(180),
            proactive_last_at: Mutex::new(0),
            proactive_target: Mutex::new(None),
            proactive_last_msg: Mutex::new(None),
            proactive_idle_rounds: Mutex::new(0),
            last_user_msg_at: Mutex::new(0),
            last_msg_ids: Mutex::new(std::collections::VecDeque::new()),
            shutdown: Mutex::new(None),
            loop_token: Mutex::new(None),
            loop_gen: AtomicU64::new(0),
            proactive_stop: AtomicBool::new(false),
            proactive_task: Mutex::new(None),
        }
    }
    /// 聊天白名单检查：allowed_users 非空时，只有名单内的用户才会被处理
    fn is_allowed(&self, from: &str) -> bool {
        let list = self.allowed_users.lock();
        if list.is_empty() {
            return true;
        }
        list.iter().any(|u| u == from)
    }

    /// 该 AI 的语音音色（默认晓晓）
    fn voice(&self) -> String {
        self.voice_id
            .lock()
            .clone()
            .unwrap_or_else(|| "zh-CN-XiaoxiaoNeural".to_string())
    }
}

/// 微信 Bot 数量（只绑定一个微信）
pub const MAX_BOTS: usize = 1;

/// 多微信 Bot 状态：槽位数组，每个槽位一个独立 WechatInner
pub struct WechatBotState(pub Mutex<Vec<Arc<WechatInner>>>);

impl WechatBotState {
    pub fn new() -> Self {
        let bots = (0..MAX_BOTS)
            .map(|slot| Arc::new(WechatInner::new(slot)))
            .collect();
        Self(Mutex::new(bots))
    }

    /// 获取指定槽位实例（new() 已预创建全部 MAX_BOTS 个，直接索引）
    pub fn bot(&self, slot: usize) -> Arc<WechatInner> {
        let slot = slot.min(MAX_BOTS - 1);
        self.0.lock()[slot].clone()
    }

    /// 全部槽位实例（用于启动时自动恢复所有已登录微信）
    pub fn bots(&self) -> Vec<Arc<WechatInner>> {
        self.0.lock().clone()
    }
}

impl Default for WechatBotState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── 工具函数 ───

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 轻量书引用：《人是怎么样的》条目数（塑造人格用，不含写作/守书人逻辑）。
/// 参照书的内容改变她，而不是让她代写书。
fn book_entry_count() -> usize {
    let dir = {
        let s = crate::llm::settings::SettingsStore::new();
        std::path::PathBuf::from(s.get().human_book_dir).join("条目")
    };
    std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
}

/// 生成短 ID（替代 uuid crate）
fn uuid() -> String {
    let n = now_millis();
    let r = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64;
    format!("{:x}-{:x}", n, r)
}

/// X-WECHAT-UIN：随机 uint32 十进制字符串 -> base64
fn random_wechat_uin() -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
        .wrapping_mul(2654435761);
    BASE64.encode(n.to_string().as_bytes())
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 构建 iLink 认证请求头
fn build_headers(token: Option<&str>) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
    let mut h = HeaderMap::new();
    h.insert("iLink-App-Id", HeaderValue::from_static(ILINK_APP_ID));
    h.insert(
        "iLink-App-ClientVersion",
        HeaderValue::from_str(&ILINK_CLIENT_VERSION.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert("AuthorizationType", HeaderValue::from_static("ilink_bot_token"));
    h.insert(
        "X-WECHAT-UIN",
        HeaderValue::from_str(&random_wechat_uin()).unwrap_or_else(|_| HeaderValue::from_static("MA==")),
    );
    if let Some(t) = token {
        let t = t.trim();
        if !t.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
                h.insert("Authorization", v);
            }
        }
    }
    h
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        // ★ IPv6 黑洞修复（2026-08-10）：DNS IPv6 优先 + 本机 IPv6 不可达时
        //   tokio 串行连接卡 IPv6 → 超时。强制 IPv4 解析（微信域名同样适用）。
        .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
        .build()
        .expect("HTTP client")
}

fn http_client_long() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
        .build()
        .expect("HTTP client")
}

// ─── 微信 CDN 媒体下载与解密（图片/文件/语音/视频，AES-128-ECB PKCS7） ───

/// hex 字符串 → 字节
fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// 安全截取前 max_chars 个字符（按字符数，不按字节）。
/// ★ 修复：`&s[..s.len().min(n)]` 按字节切片，中文等多字节字符会被切到中间
///   → 触发 "byte index is not a char boundary" panic（曾导致 getupdates 循环崩溃）。
pub fn trunc_chars(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i >= max_chars {
            break;
        }
        end = i + c.len_utf8();
    }
    &s[..end]
}

/// 解析 CDN AES key（两种格式，见 openclaw pic-decrypt.ts parseAesKey）：
/// - base64(16 原始字节) → 直接用（图片 media.aes_key）
/// - base64(32 字符 hex 串) → base64 解码后 hex 解析（文件/语音/视频）
fn parse_aes_key(aes_key_base64: &str) -> Option<Vec<u8>> {
    let decoded = BASE64.decode(aes_key_base64.trim().as_bytes()).ok()?;
    if decoded.len() == 16 {
        return Some(decoded);
    }
    if decoded.len() == 32 {
        let s = std::str::from_utf8(&decoded).ok()?;
        if s.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Some(b) = hex_to_bytes(s) {
                return Some(b);
            }
        }
    }
    None
}

/// AES-128-ECB 解密 + PKCS7 去填充（与 node:crypto 默认一致）
fn aes_ecb_decrypt(ciphertext: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 16 || ciphertext.len() % 16 != 0 {
        return None;
    }
    let cipher = Aes128::new_from_slice(key).ok()?;
    let mut buf = ciphertext.to_vec();
    for chunk in buf.chunks_exact_mut(16) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.decrypt_block(block);
    }
    pkcs7_unpad(buf)
}

/// AES-128-ECB 加密 + PKCS7 填充（发图 CDN 上传用，与 node:crypto 一致）
fn aes_ecb_encrypt(plaintext: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 16 {
        return None;
    }
    let cipher = Aes128::new_from_slice(key).ok()?;
    let pad_len = 16 - (plaintext.len() % 16);
    let mut buf = plaintext.to_vec();
    buf.extend(std::iter::repeat(pad_len as u8).take(pad_len));
    for chunk in buf.chunks_exact_mut(16) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }
    Some(buf)
}

/// 明文 MD5 hex（getUploadUrl 参数用）
fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// PKCS7 去填充
fn pkcs7_unpad(mut data: Vec<u8>) -> Option<Vec<u8>> {
    if data.is_empty() {
        return None;
    }
    let pad = *data.last()? as usize;
    if pad == 0 || pad > 16 || pad > data.len() {
        return None;
    }
    if data[data.len() - pad..].iter().all(|&b| b as usize == pad) {
        data.truncate(data.len() - pad);
        Some(data)
    } else {
        None
    }
}

/// 下载并解密 CDN 媒体，返回明文字节
async fn download_media_decrypted(
    client: &reqwest::Client,
    encrypt_query_param: &str,
    aes_key_base64: &str,
    full_url: Option<&str>,
) -> Option<Vec<u8>> {
    let url = match full_url {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => format!(
            "{CDN_BASE_URL}/download?encrypted_query_param={}",
            urlencode(encrypt_query_param)
        ),
    };
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    let key = parse_aes_key(aes_key_base64)?;
    aes_ecb_decrypt(&bytes, &key)
}

/// 媒体种类
#[derive(Debug, Clone, Copy, PartialEq)]
enum WechatMediaKind {
    Image,
    File,
    Voice,
    Video,
}

/// 文件名安全化（去路径分隔符 / 非法字符）
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 按文件头 magic bytes 识别图片真实格式（微信图片保存时不信任扩展名，
/// 避免"扩展名 .png 实际是 JPEG"导致 AI 反复探测格式）。
fn detect_image_ext(bytes: &[u8]) -> &'static str {
    if bytes.len() < 12 {
        return "bin";
    }
    // JPEG: FF D8 FF
    if bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return "jpg";
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes[0] == 0x89 && bytes[1] == 0x50 && bytes[2] == 0x4E && bytes[3] == 0x47 {
        return "png";
    }
    // GIF: 47 49 46 38 ('GIF8')
    if bytes[0] == 0x47 && bytes[1] == 0x49 && bytes[2] == 0x46 && bytes[3] == 0x38 {
        return "gif";
    }
    // WEBP: RIFF....WEBP
    if &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "webp";
    }
    // BMP: 42 4D ('BM')
    if bytes[0] == 0x42 && bytes[1] == 0x4D {
        return "bmp";
    }
    "bin"
}

/// 处理单个媒体 item：下载 + 解密 + 保存到附件目录，返回 (本地路径, 种类, 语音云端转写文本)。
/// 第三个返回值仅语音消息（ITEM_TYPE_VOICE）携带：腾讯服务器已把语音转成文字
/// （voice_item.text），直接拼入消息文本即可让 AI 听懂语音，无需本地 ASR。
async fn process_media_item(
    client: &reqwest::Client,
    item: &serde_json::Value,
    dir: &std::path::Path,
) -> Option<(String, WechatMediaKind, Option<String>)> {
    let item_type = item["type"].as_i64()?;
    let (media, filename, aes_key_b64, kind) = match item_type {
        ITEM_TYPE_IMAGE => {
            let ii = &item["image_item"];
            // 图片优先用 image_item.aeskey（hex 16 字节 → base64）；否则 media.aes_key
            let aeskey = ii["aeskey"]
                .as_str()
                .and_then(hex_to_bytes)
                .map(|b| BASE64.encode(&b))
                .or_else(|| ii["media"]["aes_key"].as_str().map(|s| s.to_string()));
            (ii["media"].clone(), None, aeskey, WechatMediaKind::Image)
        }
        ITEM_TYPE_FILE => {
            let fi = &item["file_item"];
            let name = fi["file_name"].as_str().map(|s| s.to_string());
            (
                fi["media"].clone(),
                name,
                fi["media"]["aes_key"].as_str().map(|s| s.to_string()),
                WechatMediaKind::File,
            )
        }
        ITEM_TYPE_VOICE => {
            let vi = &item["voice_item"];
            (
                vi["media"].clone(),
                None,
                vi["media"]["aes_key"].as_str().map(|s| s.to_string()),
                WechatMediaKind::Voice,
            )
        }
        ITEM_TYPE_VIDEO => {
            let vi = &item["video_item"];
            (
                vi["media"].clone(),
                None,
                vi["media"]["aes_key"].as_str().map(|s| s.to_string()),
                WechatMediaKind::Video,
            )
        }
        _ => return None,
    };
    let encrypt_query_param = media["encrypt_query_param"].as_str().unwrap_or("").to_string();
    let full_url = media["full_url"].as_str().map(|s| s.to_string());
    if encrypt_query_param.is_empty() && full_url.as_deref().unwrap_or("").is_empty() {
        return None;
    }
    let aes_key = aes_key_b64?;
    let bytes = download_media_decrypted(
        client,
        &encrypt_query_param,
        &aes_key,
        full_url.as_deref(),
    )
    .await?;
    let ext = match kind {
        // ★ 图片按文件头识别真实格式（微信图片实际多为 JPEG，固定存 .png
        //   会导致 AI 端扩展名与内容不符 → 反复探测格式浪费轮次）
        WechatMediaKind::Image => detect_image_ext(&bytes).to_string(),
        WechatMediaKind::Voice => "wav".to_string(),
        WechatMediaKind::Video => "mp4".to_string(),
        WechatMediaKind::File => filename
            .as_deref()
            .and_then(|n| n.rsplit('.').next())
            .filter(|e| !e.is_empty() && e.len() <= 10)
            .unwrap_or("bin")
            .to_string(),
    };
    let ts = now_millis();
    let fname = match &filename {
        Some(n) if !n.is_empty() => format!("wechat_{}_{}", ts, sanitize_filename(n)),
        _ => format!("wechat_{}.{}", ts, ext),
    };
    let path = dir.join(&fname);
    // 异步写盘：getupdates 热路径不做阻塞 IO
    tokio::fs::write(&path, &bytes).await.ok()?;
    // ★ 旧文件清理（按天，低频）：inbound 目录超 7 天的媒体自动删除
    crate::executors::builtin::attachment::cleanup_old_files(dir, 7, 1);

    // ★ 压缩包自动解压：用户发 .zip（≤2MB）→ 解压到附件目录，AI 可直接查看内部文件。
    //   超 2MB 不解压，返回明确提示（避免解压炸弹 / 大文件撑爆磁盘）。
    if kind == WechatMediaKind::File && fname.to_lowercase().ends_with(".zip") {
        const MAX_ZIP_BYTES: u64 = 2 * 1024 * 1024; // 2MB 上限
        if bytes.len() as u64 > MAX_ZIP_BYTES {
            eprintln!(
                "[wechat] 压缩包 {} 超过 2MB 上限（{}KB），不解压",
                fname,
                bytes.len() / 1024
            );
        } else {
            // ZIP 解压为 CPU/IO 重活 → 移到阻塞线程池，避免卡住 getupdates 异步循环
            let zip_path = path.clone();
            let parent = dir.to_path_buf();
            let extracted = tokio::task::spawn_blocking(move || {
                extract_zip_archive(&zip_path, &parent)
            })
            .await
            .ok()
            .and_then(|r| r.ok());
            if let Some(extract_dir) = extracted {
                crate::llm::logging::debug("wechat", &format!("压缩包 {} 已解压到: {}", fname, extract_dir.display()));
                return Some((
                    extract_dir.to_string_lossy().to_string(),
                    WechatMediaKind::File,
                    None,
                ));
            }
        }
    }

    // ★ 语音云端转写：voice_item.text 是腾讯服务器已转好的文字（可能为空/缺失）
    let voice_transcript = if kind == WechatMediaKind::Voice {
        item["voice_item"]["text"]
            .as_str()
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
    } else {
        None
    };

    Some((path.to_string_lossy().to_string(), kind, voice_transcript))
}

/// 引用消息信息（ref_msg 解析结果）
struct RefMsgInfo {
    /// 引用描述文本（被引用文本/语音转写），拼入消息文本头部
    note: String,
    /// 被引用图片路径（进前端 images，AI 用 analyze_image 读取）
    images: Vec<String>,
    /// 被引用其他媒体路径（进前端 attachments，AI 用 file_read 读取）
    attachments: Vec<String>,
}

/// 解析引用消息（ref_msg，openclaw-weixin 协议）：
/// ref_msg.message_item 携带被引用的原消息（文本或媒体 item），
/// 提取为 AI 可读的描述文本 + 下载被引用媒体供 AI 读取。
async fn parse_ref_msg(
    client: &reqwest::Client,
    ref_msg: &serde_json::Value,
    inbound: &std::path::Path,
) -> Option<RefMsgInfo> {
    let message_item = ref_msg.get("message_item")?;
    if !message_item.is_object() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let mut images: Vec<String> = Vec::new();
    let mut attachments: Vec<String> = Vec::new();

    // 被引用文本
    if let Some(t) = message_item["text_item"]["text"].as_str() {
        let t = t.trim();
        if !t.is_empty() {
            parts.push(t.to_string());
        }
    }
    // 被引用语音的云端转写文本
    if let Some(t) = message_item["voice_item"]["text"].as_str() {
        let t = t.trim();
        if !t.is_empty() {
            parts.push(format!("[语音转写] {}", t));
        }
    }
    // 被引用媒体（图片/语音/视频/文件）：下载解密到附件目录供 AI 读取
    if let Some(t) = message_item["type"].as_i64() {
        if (2..=5).contains(&t) {
            if let Some((path, kind, _)) =
                process_media_item(client, message_item, inbound).await
            {
                match kind {
                    WechatMediaKind::Image => images.push(path),
                    _ => attachments.push(path),
                }
            }
        }
    }
    if parts.is_empty() && images.is_empty() && attachments.is_empty() {
        return None;
    }
    let note = if parts.is_empty() {
        "[用户引用了消息]（媒体见下方）".to_string()
    } else {
        format!("[用户引用了消息：{}]", parts.join("；"))
    };
    Some(RefMsgInfo {
        note,
        images,
        attachments,
    })
}

/// 解压 zip 到同目录 `{文件名}_解压/`，返回解压目录路径。
/// 失败返回 Err（不 panic，调用方继续用原压缩包路径）。
fn extract_zip_archive(
    zip_path: &std::path::Path,
    parent_dir: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let stem = zip_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_string());
    let out_dir = parent_dir.join(format!("{}_解压", stem));
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建解压目录失败: {e}"))?;

    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开压缩包失败: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取压缩包失败: {e}"))?;

    // 安全：拒绝路径穿越（..）与绝对路径，解压总大小 ≤20MB
    let mut total: u64 = 0;
    const MAX_EXTRACT_BYTES: u64 = 20 * 1024 * 1024;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("读取压缩包条目失败: {e}"))?;
        let name = entry.name().to_string();
        let clean = name.replace('\\', "/");
        if clean.starts_with('/') || clean.split('/').any(|seg| seg == "..") {
            return Err(format!("压缩包包含非法路径，已中止: {}", name));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACT_BYTES {
            return Err("解压总大小超过 20MB，已中止".to_string());
        }
        let target = out_dir.join(&clean);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("创建目录失败: {e}"))?;
            continue;
        }
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("创建父目录失败: {e}"))?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| format!("创建文件失败: {e}"))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("写入文件失败: {e}"))?;
    }
    Ok(out_dir)
}

// ─── 持久化 ───

/// 加密文件魔数：用于区分「DPAPI 加密文件」与「旧版明文 JSON」
const ENC_MAGIC: &[u8] = b"CLWDK1";

/// Windows DPAPI 加密（绑定当前用户，仅本机该用户可解密）
pub(crate) fn dpapi_encrypt(plain: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        let ok = CryptProtectData(
            &in_blob,
            windows::core::PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        );
        if ok.is_err() {
            return None;
        }
        let data = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(out_blob.pbData as *mut core::ffi::c_void)));
        Some(data)
    }
}

/// Windows DPAPI 解密
pub(crate) fn dpapi_decrypt(encrypted: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: encrypted.len() as u32,
            pbData: encrypted.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        let ok = CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        );
        if ok.is_err() {
            return None;
        }
        let data = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(out_blob.pbData as *mut core::ffi::c_void)));
        Some(data)
    }
}

fn account_file(inner: &Arc<WechatInner>) -> Option<PathBuf> {
    let dir = inner.data_dir.lock().clone()?;
    // 每个槽位独立目录：wechat/slot{N}/account.json（微信{N+1}）
    let slot_dir = dir.join(format!("slot{}", inner.slot));
    Some(slot_dir.join("account.json"))
}

/// 每个微信槽位的主动聊天设置文件：D:\ClawDeskData\wechat\slot{N}\proactive.json
/// （开关 / 随机间隔 / 目标用户 / 上次主动时间，重启后自动恢复）
fn proactive_file(inner: &Arc<WechatInner>) -> Option<PathBuf> {
    let d = slot_dir(inner)?;
    Some(d.join("proactive.json"))
}

/// 从聊天记录恢复「最近聊过的人」（主动聊天目标兜底）。
/// 重启后 proactive.json 中目标为空时，用历史最后一条的对方用户作为目标，
/// 保证「自动（最近聊过的人）」模式在无人新发消息时也能触发主动聊天。
fn last_history_peer(inner: &Arc<WechatInner>) -> Option<String> {
    let recs = read_history_limit(inner, 200);
    for r in recs.iter().rev() {
        if let Some(d) = r.get("dir").and_then(|x| x.as_str()) {
            let d = d.trim();
            if !d.is_empty() && d != inner.user_id.lock().as_deref().unwrap_or("") {
                return Some(d.to_string());
            }
        }
    }
    None
}

/// 持久化主动聊天设置 + 白名单 + 音色（应用退出 / 重启后恢复上次配置，不丢用户设置）
fn save_proactive(inner: &Arc<WechatInner>) {
    let Some(path) = proactive_file(inner) else { return };
    let data = serde_json::json!({
        "enabled": inner.proactive_enabled.load(Ordering::SeqCst),
        "intervalMin": *inner.proactive_interval_min.lock(),
        "intervalMax": *inner.proactive_interval_max.lock(),
        "lastAt": *inner.proactive_last_at.lock(),
        "target": inner.proactive_target.lock().clone().unwrap_or_default(),
        "lastMsg": inner.proactive_last_msg.lock().clone().unwrap_or_default(),
        "allowedUsers": inner.allowed_users.lock().clone(),
        "voiceId": inner.voice_id.lock().clone().unwrap_or_default(),
        "voiceEngine": inner.voice_engine.lock().clone(),
        "cosyvoiceApiKey": inner.cosyvoice_api_key.lock().clone().unwrap_or_default(),
        "indexttsUrl": inner.indextts_url.lock().clone().unwrap_or_default(),
        "indexttsVoicePath": inner.indextts_voice_path.lock().clone().unwrap_or_default(),
    });
    if let Ok(json) = serde_json::to_string_pretty(&data) {
        if let Err(e) = write_text_atomic(&path, &json) {
            eprintln!("[wechat] slot{} 主动聊天设置写盘失败: {e}", inner.slot);
        }
    }
}

/// 从磁盘恢复主动聊天设置（应用启动 / Bot 启动时调用；文件不存在则保持默认）
fn load_proactive(inner: &Arc<WechatInner>) {
    let Some(path) = proactive_file(inner) else { return };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return };
    if let Some(enabled) = v.get("enabled").and_then(|x| x.as_bool()) {
        inner.proactive_enabled.store(enabled, Ordering::SeqCst);
    }
    if let Some(im) = v.get("intervalMin").and_then(|x| x.as_u64()) {
        *inner.proactive_interval_min.lock() = im.clamp(1, 24 * 60);
    }
    if let Some(im) = v.get("intervalMax").and_then(|x| x.as_u64()) {
        *inner.proactive_interval_max.lock() = im.clamp(1, 24 * 60);
    }
    if let Some(la) = v.get("lastAt").and_then(|x| x.as_u64()) {
        *inner.proactive_last_at.lock() = la;
    }
    if let Some(t) = v.get("target").and_then(|x| x.as_str()) {
        let t = t.trim().to_string();
        if t.is_empty() {
            *inner.proactive_target.lock() = None;
        } else {
            *inner.proactive_target.lock() = Some(t);
        }
    }
    if let Some(m) = v.get("lastMsg").and_then(|x| x.as_str()) {
        let m = m.trim().to_string();
        *inner.proactive_last_msg.lock() = if m.is_empty() { None } else { Some(m) };
    }
    // ★ 聊天白名单（逗号/换行分隔）
    if let Some(list) = v.get("allowedUsers").and_then(|x| x.as_array()) {
        let cleaned: Vec<String> = list
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        *inner.allowed_users.lock() = cleaned;
    }
    // ★ AI 语音音色
    if let Some(vid) = v.get("voiceId").and_then(|x| x.as_str()) {
        let vid = vid.trim();
        if !vid.is_empty() {
            *inner.voice_id.lock() = Some(vid.to_string());
        }
    }
    // ★ 语音引擎
    if let Some(eng) = v.get("voiceEngine").and_then(|x| x.as_str()) {
        let eng = eng.trim();
        if eng == "edge" || eng == "cosyvoice" {
            *inner.voice_engine.lock() = eng.to_string();
        }
    }
    // ★ CosyVoice API Key
    if let Some(k) = v.get("cosyvoiceApiKey").and_then(|x| x.as_str()) {
        let k = k.trim();
        if !k.is_empty() {
            *inner.cosyvoice_api_key.lock() = Some(k.to_string());
        }
    }
    // ★ IndexTTS2 服务地址与参考音频
    if let Some(u) = v.get("indexttsUrl").and_then(|x| x.as_str()) {
        let u = u.trim();
        if !u.is_empty() {
            *inner.indextts_url.lock() = Some(u.to_string());
        }
    }
    if let Some(p) = v.get("indexttsVoicePath").and_then(|x| x.as_str()) {
        let p = p.trim();
        if !p.is_empty() {
            *inner.indextts_voice_path.lock() = Some(p.to_string());
        }
    }
    // ★ 磁盘目标为空（未手动指定过）→ 从聊天记录恢复最近聊过的人，
    //   否则「自动（最近聊过的人）」重启后 target 为 None，主动聊天永不触发
    if inner.proactive_target.lock().as_deref().map(|t| t.is_empty()).unwrap_or(true) {
        if let Some(peer) = last_history_peer(inner) {
            crate::llm::logging::debug("wechat", &format!("slot{} 主动聊天目标从历史恢复: {}", inner.slot, peer));
            *inner.proactive_target.lock() = Some(peer);
        }
    }
    // 保证 min <= max（与 wechat_set_proactive 的兜底一致）
    {
        let min = *inner.proactive_interval_min.lock();
        let mut max = *inner.proactive_interval_max.lock();
        if max < min {
            max = min;
            *inner.proactive_interval_max.lock() = max;
        }
    }
}

/// 每个微信槽位的目录（账号/人设/聊天记录都在这里）：D:\ClawDeskData\wechat\slot{N}\  
fn slot_dir(inner: &Arc<WechatInner>) -> Option<PathBuf> {
    let dir = inner.data_dir.lock().clone()?;
    let d = dir.join(format!("slot{}", inner.slot));
    let _ = std::fs::create_dir_all(&d);
    Some(d)
}

/// 写文件前清除 Windows「只读」属性（用户手动/同步工具可能把 persona.md 等设为只读，
/// 导致 std::fs::write 静默失败 → 重启后读到旧内容）。失败不阻塞，尽力而为。
#[cfg(windows)]
fn ensure_writable(path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_READONLY,
        FILE_FLAGS_AND_ATTRIBUTES, INVALID_FILE_ATTRIBUTES,
    };
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let p = windows::core::PCWSTR(wide.as_ptr());
    unsafe {
        let attrs = GetFileAttributesW(p);
        if attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_READONLY.0 != 0 {
            let new_attrs = FILE_FLAGS_AND_ATTRIBUTES(attrs & !FILE_ATTRIBUTE_READONLY.0);
            if SetFileAttributesW(p, new_attrs).is_ok() {
                eprintln!("[wechat] 已清除只读属性: {}", path.display());
            } else {
                eprintln!("[wechat] ⚠️ 清除只读属性失败: {}", path.display());
            }
        }
    }
}

#[cfg(not(windows))]
fn ensure_writable(_path: &std::path::Path) {}

/// 写文本文件：先清除只读，再写入；失败返回错误信息（不再静默吞掉，
/// 让前端能提示用户，避免「保存成功但重启丢失」）。
fn write_text_atomic(path: &std::path::Path, text: &str) -> Result<(), String> {
    ensure_writable(path);
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// 聊天记录 JSONL 文件：D:\ClawDeskData\wechat\slot{N}\history.jsonl  
/// 每行一条 JSON：{dir, botSlot, fromUser, toUser, content, msgType, timestamp, fromBot}  
/// fromBot=true 表示该条是 AI（主动/自动回复）发的，false 是用户发的（用于拟人判断：
/// 用户是否回复过上次消息）。同时追加本地消息与 AI 回复。  
pub(crate) fn history_path_of(inner: &Arc<WechatInner>) -> Option<PathBuf> {
    if let Some(p) = inner.history_path.lock().clone() {
        return Some(p);
    }
    let d = slot_dir(inner)?;
    let p = d.join("history.jsonl");
    *inner.history_path.lock() = Some(p.clone());
    Some(p)
}

/// 追加一条聊天记录（用户消息或 AI 回复都记录，保证完整双向聊天记录）
/// from_bot：该消息是否为 AI 发送（true=AI / 主动消息，false=用户消息）
/// proactive：是否主动聊天循环发出的消息（区分手动/自动回复，存在感统计只计主动消息）
pub(crate) fn append_history(
    inner: &Arc<WechatInner>,
    dir: &str,
    to_user: &str,
    content: &str,
    msg_type: &str,
    from_bot: bool,
    proactive: bool,
) {
    let Some(path) = history_path_of(inner) else { return };
    let rec = serde_json::json!({
        "dir": dir,
        "botSlot": inner.slot,
        "fromUser": dir,
        "toUser": to_user,
        "content": content,
        "msgType": msg_type,
        "timestamp": now_millis(),
        "fromBot": from_bot,
        "proactive": proactive,
    });
    let line = format!("{}\n", rec.to_string());
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 读取该微信全部聊天记录（供前端展示 / 导出）
pub(crate) fn read_history(inner: &Arc<WechatInner>) -> Vec<serde_json::Value> {
    read_history_limit(inner, 0)
}

/// 读取该微信聊天记录（limit=0 表示全部；>0 时只解析最近 limit 条，
/// 供 5 秒轮询 / 主动聊天每轮使用，避免 history.jsonl 无界增长拖慢热路径）
pub(crate) fn read_history_limit(
    inner: &Arc<WechatInner>,
    limit: usize,
) -> Vec<serde_json::Value> {
    let Some(path) = history_path_of(inner) else { return vec![] };
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let lines: Vec<&str> = text.lines().collect();
    if limit > 0 && lines.len() > limit {
        lines[lines.len() - limit..]
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .collect()
    } else {
        lines
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .collect()
    }
}

async fn save_account(inner: &Arc<WechatInner>) {
    let Some(path) = account_file(inner) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let (token, bot_id, base_url, user_id) = {
        let t = inner.token.lock().clone().unwrap_or_default();
        let b = inner.bot_id.lock().clone().unwrap_or_default();
        let u = inner
            .base_url
            .lock()
            .clone()
            .unwrap_or_else(|| ILINK_BASE_URL.to_string());
        let i = inner.user_id.lock().clone().unwrap_or_default();
        (t, b, u, i)
    };
    if token.is_empty() {
        return;
    }
    // ★ 游标与 context_token 一并持久化：重启后从断点续拉（sync_buf），
    //   且无需等新消息即可回复旧会话（context_tokens 恢复 context_map）
    let (sync_buf, context_tokens) = {
        let s = inner.get_updates_buf.lock().clone();
        let m = inner.context_map.lock().clone();
        (s, m)
    };
    let acc = AccountFile {
        token,
        bot_id,
        base_url,
        user_id,
        sync_buf,
        context_tokens,
    };
    // DPAPI 加密后落盘（带魔数标记），不再明文保存凭据
    if let Ok(json) = serde_json::to_string(&acc) {
        if let Some(enc) = dpapi_encrypt(json.as_bytes()) {
            let mut buf = Vec::with_capacity(ENC_MAGIC.len() + enc.len());
            buf.extend_from_slice(ENC_MAGIC);
            buf.extend_from_slice(&enc);
            let _ = tokio::fs::write(path, buf).await;
        }
    }
}

async fn load_account(inner: &Arc<WechatInner>) {
    let Some(path) = account_file(inner) else { return };
    if let Ok(bytes) = tokio::fs::read(&path).await {
        let was_plain = !bytes.starts_with(ENC_MAGIC);
        let plain: Option<Vec<u8>> = if bytes.starts_with(ENC_MAGIC) {
            // 新格式：DPAPI 解密
            dpapi_decrypt(&bytes[ENC_MAGIC.len()..])
        } else {
            // 旧格式：明文 JSON（向前兼容旧版本）
            Some(bytes)
        };
        if let Some(plain) = plain {
            if let Ok(text) = std::str::from_utf8(&plain) {
                if let Ok(acc) = serde_json::from_str::<AccountFile>(text) {
                    if !acc.token.is_empty() {
                        *inner.token.lock() = Some(acc.token);
                        *inner.bot_id.lock() = Some(acc.bot_id);
                        *inner.base_url.lock() = Some(acc.base_url);
                        *inner.user_id.lock() = Some(acc.user_id);
                        // ★ 恢复 get_updates 游标（断点续拉）与 context_token 缓存
                        if !acc.sync_buf.is_empty() {
                            *inner.get_updates_buf.lock() = acc.sync_buf;
                        }
                        if !acc.context_tokens.is_empty() {
                            *inner.context_map.lock() = acc.context_tokens;
                        }
                        // 旧明文文件：顺手迁移为加密格式
                        if was_plain {
                            save_account(inner).await;
                        }
                    }
                }
            }
        }
    }
    // 加载该微信人设（slot{N}/persona.md，不存在则跳过）
    if let Some(d) = slot_dir(inner) {
        let pf = d.join("persona.md");
        if let Ok(text) = std::fs::read_to_string(&pf) {
            if !text.trim().is_empty() {
                *inner.persona.lock() = Some(text);
            }
        }
    }
    // 恢复主动聊天设置（slot{N}/proactive.json，上次开关/间隔/目标/时间）
    load_proactive(&inner);
}

async fn delete_account(inner: &Arc<WechatInner>) {
    if let Some(path) = account_file(inner) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ─── iLink API 调用 ───

/// 获取登录二维码
async fn fetch_qr(client: &reqwest::Client) -> AppResult<QrSession> {
    let url = format!(
        "{}/ilink/bot/get_bot_qrcode?bot_type={}",
        ILINK_BASE_URL, ILINK_BOT_TYPE
    );
    let resp = client
        .post(&url)
        .headers(build_headers(None))
        .json(&serde_json::json!({ "local_token_list": [] }))
        .send()
        .await
        .map_err(|e| format!("获取微信二维码失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "获取微信二维码失败 HTTP {status}: {}",
            trunc_chars(&text, 200)
        ));
    }
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("解析二维码响应失败: {e}"))?;
    let qrcode = v["qrcode"].as_str().unwrap_or_default().to_string();
    let qrcode_url = v["qrcode_img_content"].as_str().unwrap_or_default().to_string();
    if qrcode.is_empty() || qrcode_url.is_empty() {
        return Err(format!(
            "二维码响应异常: {}",
            trunc_chars(&text, 200)
        ));
    }
    Ok(QrSession {
        qrcode,
        qrcode_url,
        started_at: now_millis(),
        pending_verify_code: None,
        polling_base: ILINK_BASE_URL.to_string(),
    })
}

/// 长轮询扫码状态（服务端最多 hold 35s）
async fn poll_qr_status(client: &reqwest::Client, session: &QrSession) -> serde_json::Value {
    let mut endpoint = format!(
        "{}/ilink/bot/get_qrcode_status?qrcode={}",
        session.polling_base,
        urlencode(&session.qrcode)
    );
    if let Some(vc) = &session.pending_verify_code {
        endpoint.push_str(&format!("&verify_code={}", urlencode(vc)));
    }
    match client
        .get(&endpoint)
        .headers(build_headers(None))
        .timeout(QR_POLL_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) => resp
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|_| serde_json::json!({ "status": "wait" })),
        // 超时 / 网络错误 -> 视为等待，继续轮询
        Err(_) => serde_json::json!({ "status": "wait" }),
    }
}

/// 单个用户的 typing ticket 缓存项（AstrBot 对齐：ticket 绑定 context_token，TTL 60s）
#[derive(Debug, Clone)]
pub(crate) struct TypingTicketEntry {
    ticket: String,
    ctx: String,
    refresh_after_ms: u64,
}

/// 发送"正在输入"状态（active=true 开始输入，false 结束输入）
/// ★ 修复：ticket 按用户获取（带 context_token），status 用 1/2（对齐 AstrBot）：
///   旧实现用全局空 ticket + status=0，服务端不识别 → 对方看不到"正在输入"。
async fn send_typing(inner: &Arc<WechatInner>, to: &str, active: bool) {
    let Some(token) = inner.token.lock().clone() else { return };
    let Some(ticket) = ensure_typing_ticket(inner, to).await else { return };
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());
    let client = http_client();
    let _ = client
        .post(format!("{base_url}/ilink/bot/sendtyping"))
        .headers(build_headers(Some(&token)))
        .json(&serde_json::json!({
            "ilink_user_id": to,
            "typing_ticket": ticket,
            "status": if active { 1 } else { 2 },
            "base_info": { "channel_version": CHANNEL_VERSION },
        }))
        .send()
        .await;
}

/// 确保拿到 to 用户的 typing ticket（带 context_token，60s TTL）。
/// 缓存有效（ctx 匹配 + 未过期）→ 复用；否则调 getconfig 现取。
/// 没有该用户的 context_token 时返回 None（无法获取，跳过 typing）。
async fn ensure_typing_ticket(inner: &Arc<WechatInner>, to: &str) -> Option<String> {
    let Some(token) = inner.token.lock().clone() else { return None };
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());
    let ctx = inner.context_map.lock().get(to).cloned()?;

    // 缓存命中：ticket 存在 + context_token 匹配 + 未过期（60s）
    {
        let map = inner.typing_tickets.lock();
        if let Some(e) = map.get(to) {
            if e.ctx == ctx && now_millis() < e.refresh_after_ms {
                return Some(e.ticket.clone());
            }
        }
    }

    // 重新获取（AstrBot 对齐：getconfig 必须带 ilink_user_id + context_token）
    let client = http_client();
    let resp = client
        .post(format!("{base_url}/ilink/bot/getconfig"))
        .headers(build_headers(Some(&token)))
        .json(&serde_json::json!({
            "ilink_user_id": to,
            "context_token": ctx,
            "base_info": { "channel_version": CHANNEL_VERSION },
        }))
        .send()
        .await;
    if let Ok(resp) = resp {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(t) = v["typing_ticket"].as_str() {
                if !t.is_empty() {
                    let entry = TypingTicketEntry {
                        ticket: t.to_string(),
                        ctx,
                        refresh_after_ms: now_millis() + 60_000,
                    };
                    inner.typing_tickets.lock().insert(to.to_string(), entry);
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

/// 登录成功后清理 typing ticket 缓存（旧 ticket 随会话失效，下次发送时现取）
async fn refresh_typing_ticket(inner: &Arc<WechatInner>) {
    inner.typing_tickets.lock().clear();
}

/// 通知腾讯服务"客户端已启动"（sendmessage 前置条件，缺省会返回 -14 session timeout）
async fn notify_start(inner: &Arc<WechatInner>) -> bool {
    let Some(token) = inner.token.lock().clone() else { return false };
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());
    let client = http_client();
    let resp = client
        .post(format!("{base_url}/ilink/bot/msg/notifystart"))
        .headers(build_headers(Some(&token)))
        .json(&serde_json::json!({
            "base_info": { "channel_version": CHANNEL_VERSION, "bot_agent": "ClawDesk" },
        }))
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            crate::llm::logging::debug(
                "wechat",
                &format!("notifyStart status={status} body={}", trunc_chars(&text, 200))
            );
            if !status.is_success() {
                return false;
            }
            // 检查业务码
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                let ret = v["ret"].as_i64().unwrap_or(0);
                if ret != 0 {
                    crate::llm::logging::debug(
                        "wechat",
                        &format!("notifyStart ret={ret} errmsg={}", v["errmsg"].as_str().unwrap_or(""))
                    );
                    return false;
                }
            }
            true
        }
        Err(e) => {
            eprintln!("[wechat] notifyStart failed: {e}");
            false
        }
    }
}

/// 发送一次文本消息（内部函数）
async fn send_message_once(
    token: &str,
    base_url: &str,
    to: &str,
    text: &str,
    context_token: Option<&str>,
) -> Result<(), (String, Option<i64>)> {
    let client = http_client();
    let body = serde_json::json!({
        "msg": {
            "from_user_id": "",
            "to_user_id": to,
            "client_id": uuid(),
            "message_type": MSG_TYPE_BOT,
            "message_state": 2, // FINISH
            "item_list": [{ "type": ITEM_TYPE_TEXT, "text_item": { "text": text } }],
            "context_token": context_token.unwrap_or(""),
        }
    });
    crate::llm::logging::debug("wechat", &format!("发送文本消息 to={} len={}", to, text.len()));
    let resp = client
        .post(format!("{base_url}/ilink/bot/sendmessage"))
        .headers(build_headers(Some(token)))
        .json(&body)
        .send()
        .await
        .map_err(|e| (format!("发送微信消息失败: {e}"), None))?;
    let status = resp.status();
    let text_resp = resp.text().await.unwrap_or_default();
    crate::llm::logging::debug("wechat", &format!("sendmessage status={status}"));
    if !status.is_success() {
        return Err((
            format!(
                "发送微信消息失败 HTTP {status}: {}",
                trunc_chars(&text_resp, 200)
            ),
            None,
        ));
    }
    // 腾讯可能返回 HTTP 200 但业务错误码（-14 = session timeout）
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text_resp) {
        let ret = v["ret"].as_i64().or_else(|| v["errcode"].as_i64()).unwrap_or(0);
        if ret != 0 {
            let errmsg = v["errmsg"].as_str().unwrap_or("");
            eprintln!("[wechat] sendmessage business error ret={ret} errmsg={errmsg}");
            return Err((format!("微信发送失败 ret={ret} errmsg={errmsg}"), Some(ret)));
        }
    }
    Ok(())
}

/// 发送文本消息到微信用户（遇 -14 session timeout 时先 notifyStart 再重发一次）
pub(crate) async fn send_message(
    inner: &Arc<WechatInner>,
    to: &str,
    text: &str,
    context_token: Option<&str>,
) -> AppResult<()> {
    let token = inner
        .token
        .lock()
        .clone()
        .ok_or_else(|| "微信未登录，请先扫码登录".to_string())?;
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());

    match send_message_once(&token, &base_url, to, text, context_token).await {
        Ok(()) => Ok(()),
        Err((msg, Some(-14))) => {
            // session 超时：重新激活后重试一次
            eprintln!("[wechat] sendmessage -14 session timeout, re-activating...");
            if notify_start(inner).await {
                send_message_once(&token, &base_url, to, text, context_token)
                    .await
                    .map_err(|(m, _)| m)
            } else {
                Err(msg)
            }
        }
        Err((msg, _)) => Err(msg),
    }
}

// ─── 微信发图（getuploadurl → CDN 上传 → sendmessage image_item） ───
// 协议参考：openclaw-weixin src/cdn/upload.ts + src/messaging/send.ts
// 流程：1) 读文件 → 算 md5/密文大小 → 随机 aeskey/filekey
//       2) POST /ilink/bot/getuploadurl 获取 CDN 上传参数
//       3) AES-128-ECB 加密文件 → POST 到 CDN，取响应头 x-encrypted-param
//       4) sendmessage 携带 image_item（media.encrypt_query_param / aes_key）

/// 上传本地图片到微信 CDN，返回 (downloadEncryptedQueryParam, aeskey_base64, filekey)。
async fn upload_image_to_cdn(
    token: &str,
    base_url: &str,
    to: &str,
    image_path: &str,
) -> Result<(String, String, String), String> {
    let client = http_client();
    let plaintext = tokio::fs::read(image_path)
        .await
        .map_err(|e| format!("读取图片失败: {e}"))?;
    if plaintext.is_empty() {
        return Err("图片文件为空".into());
    }
    let rawsize = plaintext.len();
    let rawfilemd5 = md5_hex(&plaintext);
    let aeskey = random_16_bytes();
    let ciphertext = aes_ecb_encrypt(&plaintext, &aeskey).ok_or("AES 加密失败")?;
    let filesize = ciphertext.len();
    // 时间戳 + 随机 4 字节 hex：避免同毫秒 filekey 碰撞
    let filekey = format!("{:x}{:08x}", now_millis(), random_u32());

    // 1) getuploadurl
    let upload_body = serde_json::json!({
        "filekey": filekey,
        "media_type": 1, // IMAGE
        "to_user_id": to,
        "rawsize": rawsize,
        "rawfilemd5": rawfilemd5,
        "filesize": filesize,
        "no_need_thumb": true,
        "aeskey": hex_encode(&aeskey),
        "base_info": { "channel_version": CHANNEL_VERSION, "bot_agent": "ClawDesk" },
    });
    let resp = client
        .post(format!("{base_url}/ilink/bot/getuploadurl"))
        .headers(build_headers(Some(token)))
        .json(&upload_body)
        .send()
        .await
        .map_err(|e| format!("获取上传地址失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "getuploadurl HTTP {status}: {}",
            trunc_chars(&text, 200)
        ));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("解析 getuploadurl 响应失败: {e}"))?;
    if v["ret"].as_i64().unwrap_or(0) != 0 {
        return Err(format!("getuploadurl ret={} errmsg={}", v["ret"], v["errmsg"].as_str().unwrap_or("")));
    }
    let upload_full_url = v["upload_full_url"].as_str().unwrap_or("").to_string();
    let upload_param = v["upload_param"].as_str().unwrap_or("").to_string();
    let cdn_url = if !upload_full_url.is_empty() {
        upload_full_url
    } else if !upload_param.is_empty() {
        format!("{CDN_BASE_URL}/upload?encrypted_query_param={}&filekey={}", urlencode(&upload_param), urlencode(&filekey))
    } else {
        return Err(format!(
            "getuploadurl 未返回上传地址: {}",
            trunc_chars(&text, 200)
        ));
    };

    // 2) CDN 上传密文
    let resp = client
        .post(&cdn_url)
        .header("Content-Type", "application/octet-stream")
        .body(ciphertext)
        .send()
        .await
        .map_err(|e| format!("CDN 上传失败: {e}"))?;
    let dl = resp
        .headers()
        .get("x-encrypted-param")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("CDN 上传响应缺少 x-encrypted-param（HTTP {}）", resp.status()))?;
    Ok((dl, BASE64.encode(&aeskey), filekey))
}

/// 16 字节真随机密钥（CDN AES 加密用，getrandom 系统级熵源）。
/// 极低概率失败时回退到时间+伪随机混合，保证功能可用。
fn random_16_bytes() -> Vec<u8> {
    let mut buf = [0u8; 16];
    if getrandom::getrandom(&mut buf).is_ok() {
        return buf.to_vec();
    }
    (0..16)
        .map(|i| ((now_millis() as u8).wrapping_add(i as u8)) ^ (pseudo_rand() as u8))
        .collect()
}

/// 随机 32 位整数（filekey 后缀防同毫秒碰撞）
fn random_u32() -> u32 {
    let mut buf = [0u8; 4];
    if getrandom::getrandom(&mut buf).is_ok() {
        return u32::from_le_bytes(buf);
    }
    pseudo_rand() as u32
}

/// 字节 → hex
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 发送图片消息到微信用户（先上传 CDN 再发 image_item）。
/// `image_path` 为本地图片绝对路径。
pub(crate) async fn send_image(
    inner: &Arc<WechatInner>,
    to: &str,
    image_path: &str,
    context_token: Option<&str>,
) -> AppResult<()> {
    let token = inner
        .token
        .lock()
        .clone()
        .ok_or_else(|| "微信未登录，请先扫码登录".to_string())?;
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());

    match send_image_once(&token, &base_url, to, image_path, context_token).await {
        Ok(()) => Ok(()),
        Err((msg, Some(-14))) => {
            eprintln!("[wechat] sendimage -14 session timeout, re-activating...");
            if notify_start(inner).await {
                send_image_once(&token, &base_url, to, image_path, context_token)
                    .await
                    .map_err(|(m, _)| m)
            } else {
                Err(msg)
            }
        }
        Err((msg, _)) => Err(msg),
    }
}

/// 发送一次图片消息（内部函数）
async fn send_image_once(
    token: &str,
    base_url: &str,
    to: &str,
    image_path: &str,
    context_token: Option<&str>,
) -> Result<(), (String, Option<i64>)> {
    let (dl_param, aeskey_b64, filekey) = upload_image_to_cdn(token, base_url, to, image_path)
        .await
        .map_err(|e| (e, None))?;
    let client = http_client();
    let body = serde_json::json!({
        "msg": {
            "from_user_id": "",
            "to_user_id": to,
            "client_id": uuid(),
            "message_type": MSG_TYPE_BOT,
            "message_state": 2,
            "item_list": [{
                "type": ITEM_TYPE_IMAGE,
                "image_item": {
                    "media": {
                        "encrypt_query_param": dl_param,
                        "aes_key": aeskey_b64,
                        "encrypt_type": 1
                    },
                    "mid_size": std::fs::metadata(image_path).map(|m| m.len()).unwrap_or(0)
                }
            }],
            "context_token": context_token.unwrap_or(""),
        }
    });
    crate::llm::logging::debug(
        "wechat",
        &format!(
            "sendimage to={} filekey={} ctx={}",
            to,
            filekey,
            if context_token.unwrap_or("").is_empty() { "NONE" } else { "YES" }
        ),
    );
    crate::llm::logging::debug("wechat", &format!("发送图片消息 to={} file={}", to, image_path));
    let resp = client
        .post(format!("{base_url}/ilink/bot/sendmessage"))
        .headers(build_headers(Some(token)))
        .json(&body)
        .send()
        .await
        .map_err(|e| (format!("发送图片消息失败: {e}"), None))?;
    let status = resp.status();
    let text_resp = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err((
            format!(
                "发送图片消息失败 HTTP {status}: {}",
                trunc_chars(&text_resp, 200)
            ),
            None,
        ));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text_resp) {
        let ret = v["ret"].as_i64().or_else(|| v["errcode"].as_i64()).unwrap_or(0);
        if ret != 0 {
            let errmsg = v["errmsg"].as_str().unwrap_or("");
            eprintln!("[wechat] sendimage business error ret={ret} errmsg={errmsg}");
            return Err((format!("微信图片发送失败 ret={ret} errmsg={errmsg}"), Some(ret)));
        }
    }
    Ok(())
}

// ─── 微信发文件（getuploadurl media_type=3 → CDN 上传 → sendmessage file_item）───
// 协议参考：AstrBot weixin_oc _prepare_media_item（FILE_UPLOAD_TYPE=3, FILE_ITEM_TYPE=4）
// 用于发送 AI 语音回复（Edge TTS 合成的 mp3）等任意本地文件。

/// 上传本地文件到微信 CDN，返回 (downloadEncryptedQueryParam, aeskey_base64, filekey)。
async fn upload_file_to_cdn(
    token: &str,
    base_url: &str,
    to: &str,
    file_path: &str,
) -> Result<(String, String, String), String> {
    let client = http_client();
    let plaintext = tokio::fs::read(file_path)
        .await
        .map_err(|e| format!("读取文件失败: {e}"))?;
    if plaintext.is_empty() {
        return Err("文件为空".into());
    }
    let rawsize = plaintext.len();
    let rawfilemd5 = md5_hex(&plaintext);
    let aeskey = random_16_bytes();
    let ciphertext = aes_ecb_encrypt(&plaintext, &aeskey).ok_or("AES 加密失败")?;
    let filesize = ciphertext.len();
    // 时间戳 + 随机 4 字节 hex：避免同毫秒 filekey 碰撞
    let filekey = format!("{:x}{:08x}", now_millis(), random_u32());

    // 1) getuploadurl（media_type=3 = FILE）
    let upload_body = serde_json::json!({
        "filekey": filekey,
        "media_type": 3, // FILE
        "to_user_id": to,
        "rawsize": rawsize,
        "rawfilemd5": rawfilemd5,
        "filesize": filesize,
        "no_need_thumb": true,
        "aeskey": hex_encode(&aeskey),
        "base_info": { "channel_version": CHANNEL_VERSION, "bot_agent": "ClawDesk" },
    });
    let resp = client
        .post(format!("{base_url}/ilink/bot/getuploadurl"))
        .headers(build_headers(Some(token)))
        .json(&upload_body)
        .send()
        .await
        .map_err(|e| format!("获取上传地址失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "getuploadurl HTTP {status}: {}",
            trunc_chars(&text, 200)
        ));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("解析 getuploadurl 响应失败: {e}"))?;
    if v["ret"].as_i64().unwrap_or(0) != 0 {
        return Err(format!("getuploadurl ret={} errmsg={}", v["ret"], v["errmsg"].as_str().unwrap_or("")));
    }
    let upload_full_url = v["upload_full_url"].as_str().unwrap_or("").to_string();
    let upload_param = v["upload_param"].as_str().unwrap_or("").to_string();
    let cdn_url = if !upload_full_url.is_empty() {
        upload_full_url
    } else if !upload_param.is_empty() {
        format!("{CDN_BASE_URL}/upload?encrypted_query_param={}&filekey={}", urlencode(&upload_param), urlencode(&filekey))
    } else {
        return Err(format!(
            "getuploadurl 未返回上传地址: {}",
            trunc_chars(&text, 200)
        ));
    };

    // 2) CDN 上传密文
    let resp = client
        .post(&cdn_url)
        .header("Content-Type", "application/octet-stream")
        .body(ciphertext)
        .send()
        .await
        .map_err(|e| format!("CDN 上传失败: {e}"))?;
    let dl = resp
        .headers()
        .get("x-encrypted-param")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("CDN 上传响应缺少 x-encrypted-param（HTTP {}）", resp.status()))?;
    Ok((dl, BASE64.encode(&aeskey), filekey))
}

/// 发送本地文件到微信用户（先上传 CDN 再发 file_item，含 -14 自动重连重试）。
pub(crate) async fn send_file(
    inner: &Arc<WechatInner>,
    to: &str,
    file_path: &str,
    file_name: Option<&str>,
    context_token: Option<&str>,
) -> AppResult<()> {
    let token = inner
        .token
        .lock()
        .clone()
        .ok_or_else(|| "微信未登录，请先扫码登录".to_string())?;
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());
    match send_file_once(&token, &base_url, to, file_path, file_name, context_token).await {
        Ok(()) => Ok(()),
        Err((msg, Some(-14))) => {
            eprintln!("[wechat] sendfile -14 session timeout, re-activating...");
            if notify_start(inner).await {
                send_file_once(&token, &base_url, to, file_path, file_name, context_token)
                    .await
                    .map_err(|(m, _)| m)
            } else {
                Err(msg)
            }
        }
        Err((msg, _)) => Err(msg),
    }
}

/// 发送一次文件消息（内部函数）
async fn send_file_once(
    token: &str,
    base_url: &str,
    to: &str,
    file_path: &str,
    file_name: Option<&str>,
    context_token: Option<&str>,
) -> Result<(), (String, Option<i64>)> {
    let (dl_param, aeskey_b64, _filekey) = upload_file_to_cdn(token, base_url, to, file_path)
        .await
        .map_err(|e| (e, None))?;
    let fname = match file_name {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => std::path::Path::new(file_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file.bin".to_string()),
    };
    let client = http_client();
    let body = serde_json::json!({
        "msg": {
            "from_user_id": "",
            "to_user_id": to,
            "client_id": uuid(),
            "message_type": MSG_TYPE_BOT,
            "message_state": 2,
            "item_list": [{
                "type": ITEM_TYPE_FILE,
                "file_item": {
                    "media": {
                        "encrypt_query_param": dl_param,
                        "aes_key": aeskey_b64,
                        "encrypt_type": 1
                    },
                    "file_name": fname,
                    "len": std::fs::metadata(file_path).map(|m| m.len().to_string()).unwrap_or_default()
                }
            }],
            "context_token": context_token.unwrap_or(""),
        }
    });
    crate::llm::logging::debug("wechat", &format!("发送文件消息 to={} file={}", to, file_path));
    let resp = client
        .post(format!("{base_url}/ilink/bot/sendmessage"))
        .headers(build_headers(Some(token)))
        .json(&body)
        .send()
        .await
        .map_err(|e| (format!("发送文件消息失败: {e}"), None))?;
    let status = resp.status();
    let text_resp = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err((
            format!("发送文件消息失败 HTTP {status}: {}", trunc_chars(&text_resp, 200)),
            None,
        ));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text_resp) {
        let ret = v["ret"].as_i64().or_else(|| v["errcode"].as_i64()).unwrap_or(0);
        if ret != 0 {
            let errmsg = v["errmsg"].as_str().unwrap_or("");
            eprintln!("[wechat] sendfile business error ret={ret} errmsg={errmsg}");
            return Err((format!("微信文件发送失败 ret={ret} errmsg={errmsg}"), Some(ret)));
        }
    }
    Ok(())
}

/// 发送 AI 语音回复（按槽位配置的引擎合成真人音色 mp3 → 作为文件发给微信用户）。
/// - engine=edge: Edge TTS 免费合成（默认）
/// - engine=cosyvoice: 硅基流动 CosyVoice 2 真人级音色（需 API Key，免费额度）
/// - engine=indextts: 本地 IndexTTS2 声音克隆（参考音频 = 诗妍的声音，最像真人）
/// 微信 iLink 官方协议不支持发送语音条，文件是官方协议下的最佳近似。
#[tauri::command]
pub async fn wechat_send_voice(
    state: tauri::State<'_, WechatBotState>,
    to_user: String,
    text: String,
    voice: Option<String>,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    // 1) 按引擎合成（indextts > cosyvoice > edge；缺配置自动回退）
    let engine = inner.voice_engine.lock().clone();
    let voice_id = inner.voice();
    let audio = if engine == "indextts" {
        let url = inner
            .indextts_url
            .lock()
            .clone()
            .unwrap_or_else(|| "http://127.0.0.1:8000".to_string());
        let vp = inner.indextts_voice_path.lock().clone();
        match vp {
            Some(p) if !p.is_empty() => {
                match crate::commands::tts::synthesize_audio_indextts(&text, &p, &url).await {
                    Ok(a) => a,
                    Err(e) => {
                        crate::llm::logging::debug("wechat", &format!("IndexTTS2 合成失败（回退 Edge TTS）: {e}"));
                        crate::commands::tts::synthesize_audio(&text, &voice_id, 1.0, "")
                            .await
                            .map_err(|e2| format!("语音合成失败: {e2}"))?
                    }
                }
            }
            _ => {
                crate::llm::logging::debug("wechat", &format!("未配置 IndexTTS2 参考音频，回退 Edge TTS"));
                crate::commands::tts::synthesize_audio(&text, &voice_id, 1.0, "")
                    .await
                    .map_err(|e| format!("语音合成失败: {e}"))?
            }
        }
    } else if engine == "cosyvoice" {
        let key = inner.cosyvoice_api_key.lock().clone();
        match key {
            Some(k) if !k.is_empty() => {
                let v = voice.unwrap_or_else(|| voice_id.clone());
                match crate::commands::tts::synthesize_audio_cosyvoice(&text, &v, &k).await {
                    Ok(a) => a,
                    Err(e) => {
                        crate::llm::logging::debug("wechat", &format!("CosyVoice 合成失败（回退 Edge TTS）: {e}"));
                        crate::commands::tts::synthesize_audio(&text, &voice_id, 1.0, "")
                            .await
                            .map_err(|e2| format!("语音合成失败: {e2}"))?
                    }
                }
            }
            _ => {
                crate::llm::logging::debug("wechat", &format!("未配置 CosyVoice API Key，回退 Edge TTS"));
                crate::commands::tts::synthesize_audio(&text, &voice_id, 1.0, "")
                    .await
                    .map_err(|e| format!("语音合成失败: {e}"))?
            }
        }
    } else {
        crate::commands::tts::synthesize_audio(&text, &voice_id, 1.0, "")
            .await
            .map_err(|e| format!("语音合成失败: {e}"))?
    };
    // 2) 写入附件目录（outbound/voice/）
    let dir = crate::executors::builtin::attachment::attach_dir()
        .map_err(|e| format!("获取附件目录失败: {e}"))?;
    let out_dir = dir.join("outbound").join("voice");
    let _ = std::fs::create_dir_all(&out_dir);
    let fname = format!("voice_{}.mp3", now_millis());
    let path = out_dir.join(&fname);
    tokio::fs::write(&path, &audio)
        .await
        .map_err(|e| format!("语音文件写入失败: {e}"))?;
    // ★ 旧文件清理（按天，低频）：outbound/voice 超 7 天的旧语音自动删除
    crate::executors::builtin::attachment::cleanup_old_files(&out_dir, 7, 1);
    // 3) 发送文件（file_item），文件名带"语音"标记便于识别
    let ctx = inner.context_map.lock().get(&to_user).cloned();
    send_file(&inner, &to_user, &path.to_string_lossy(), Some(&fname), ctx.as_deref()).await?;
    append_history(&inner, &to_user, &to_user, &text, "voice", true, false);
    crate::llm::logging::debug("wechat", &format!("slot{} 语音回复已发送 to={} voice={} bytes={}", inner.slot, to_user, voice_id, audio.len()));
    Ok(())
}

// ─── getUpdates 长轮询后台循环 ───

async fn start_getupdates_loop(inner: &Arc<WechatInner>, app: AppHandle) {
    let token = inner.token.lock().clone().unwrap_or_default();
    // ★ 修复：token 已变更而旧循环仍在运行（如重新扫码换号）→ 先关掉旧循环，
    //   让新循环接管。先 take 出 shutdown 发送器并发送，释放锁后再创建新循环，避免死锁。
    {
        let stale = inner
            .loop_token
            .lock()
            .as_ref()
            .map(|t| t != &token)
            .unwrap_or(false);
        if inner.shutdown.lock().is_some() && stale {
            if let Some(tx) = inner.shutdown.lock().take() {
                let _ = tx.send(());
            }
        }
    }
    // 已有同 token 循环则不重复启动
    if inner.shutdown.lock().is_some() {
        return;
    }
    let my_gen = inner.loop_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    *inner.shutdown.lock() = Some(tx);
    *inner.loop_token.lock() = Some(token);

    let inner2 = inner.clone();
    let app2 = app.clone();
    inner.running.store(true, Ordering::SeqCst);
    inner.connected.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        // fut 内使用独立 clone，外层 inner2/app2 留给循环退出后的清理
        let inner_f = inner2.clone();
        let app_f = app2.clone();
        let fut = async move {
            let inner2 = inner_f;
            let app2 = app_f;
            let client = http_client_long();
            let mut buf = inner2.get_updates_buf.lock().clone();
            let mut err14_count: u32 = 0;
            loop {
                if rx.try_recv().is_ok() {
                    break;
                }
                inner2.running.store(true, Ordering::SeqCst);
                inner2.connected.store(true, Ordering::SeqCst);
                // ★ 修复：每轮重读 token/base_url，登出/重新扫码后立即生效，
                //   不再用启动时捕获的旧 token 无限空转
                let Some(token) = inner2
                    .token
                    .lock()
                    .clone()
                    .filter(|t| !t.is_empty())
                else {
                    eprintln!(
                        "[wechat] slot{} token 已清空（登出），停止 getupdates 循环",
                        inner2.slot
                    );
                    break;
                };
                    let base_url = inner2
                        .base_url
                        .lock()
                        .clone()
                        .unwrap_or_else(|| ILINK_BASE_URL.to_string());
                    let resp = match client
                        .post(format!("{base_url}/ilink/bot/getupdates"))
                        .headers(build_headers(Some(&token)))
                        .timeout(GETUPDATES_TIMEOUT)
                        .json(&serde_json::json!({
                            "get_updates_buf": buf,
                            "base_info": { "channel_version": CHANNEL_VERSION, "bot_agent": "ClawDesk" },
                        }))
                        .send()
                        .await
                    {
                        Ok(r) => match r.json::<serde_json::Value>().await {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!(
                                    "[wechat] slot{} getupdates 响应解析失败: {e}，1 秒后重试",
                                    inner2.slot
                                );
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                        },
                        // 网络错误（超时是长轮询的正常退出，其余为真实故障——必须打日志）
                        Err(e) => {
                            eprintln!(
                                "[wechat] slot{} getupdates 网络错误: {e}，2 秒后重试",
                                inner2.slot
                            );
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            continue;
                        }
                    };

                    // 更新同步 buf（bytes 字段为 base64 字符串）
                    let mut state_dirty = false;
                    if let Some(b) = resp["get_updates_buf"].as_str() {
                        let b = b.trim();
                        // ★ 修复：只接受非空且不回退的游标回显。
                        //   空回显或游标回退会导致历史消息重放 → 一律忽略
                        let forward = !b.is_empty()
                            && b != buf
                            && (b.len() > buf.len()
                                || (b.len() == buf.len() && b.as_bytes() >= buf.as_bytes()));
                        if forward {
                            buf = b.to_string();
                            *inner2.get_updates_buf.lock() = buf.clone();
                            state_dirty = true;
                        }
                    }
                    *inner2.last_poll.lock() = now_millis();

                    // 业务错误
                    let ret = resp["ret"].as_i64().unwrap_or(0);
                    let errcode = resp["errcode"].as_i64().unwrap_or(0);
                    if ret != 0 || errcode != 0 {
                        // ret/errcode = -14 表示 session 超时（token 失效），需重新扫码
                        if ret == -14 || errcode == -14 {
                            err14_count += 1;
                            inner2.connected.store(false, Ordering::SeqCst);
                            if err14_count >= 5 {
                                // 连续 5 次会话失效 → 停止循环，通知前端重新扫码（防重连风暴）
                                eprintln!(
                                    "[wechat] slot{} session 连续失效 {} 次，停止轮询，请重新扫码登录",
                                    inner2.slot, err14_count
                                );
                                let _ = app2.emit(
                                    "wechat-bot-status",
                                    serde_json::json!({ "type": "session_expired", "slot": inner2.slot }),
                                );
                                break;
                            }
                            // 指数退避（1s→2s→4s…封顶 60s），避免重连风暴
                            let backoff = (1u64 << err14_count.saturating_sub(1)).min(60);
                            eprintln!(
                                "[wechat] slot{} session 超时(ret={ret} errcode={errcode})，{backoff} 秒后重试（{}/5）",
                                inner2.slot, err14_count
                            );
                            tokio::time::sleep(Duration::from_secs(backoff)).await;
                            continue;
                        }
                        err14_count = 0;
                        eprintln!(
                            "[wechat] slot{} getupdates 业务错误 ret={ret} errcode={errcode}，1 秒后重试",
                            inner2.slot
                        );
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                    err14_count = 0;
                    inner2.connected.store(true, Ordering::SeqCst);

            let msgs = resp["msgs"].as_array().cloned().unwrap_or_default();
            if msgs.is_empty() {
                continue;
            }
            for m in msgs {
                // 仅处理用户消息（1=USER），机器人消息（2）跳过
                if m["message_type"].as_i64() != Some(MSG_TYPE_USER) {
                    continue;
                }
                let from = m["from_user_id"].as_str().unwrap_or_default().to_string();
                if from.is_empty() {
                    continue;
                }
                // ★ 聊天白名单（只和指定的人聊天）：白名单非空时，名单外的
                //   用户消息直接忽略（不自动回复、不进入最近聊天、不主动找）
                if !inner2.is_allowed(&from) {
                    crate::llm::logging::debug(
                        "wechat",
                        &format!(
                            "slot{} 白名单拦截: from={}（该微信只与 {} 位指定用户聊天）",
                            inner2.slot,
                            from,
                            inner2.allowed_users.lock().len()
                        )
                    );
                    continue;
                }
                let context_token = m["context_token"].as_str().unwrap_or_default().to_string();

                // ★ 消息去重提前（对齐 AstrBot dedup 思路）：getupdates 偶尔会重复投递
                //   同一消息，必须在媒体下载/写盘之前判定，避免重复投递反复下载解密写盘。
                //   无 seq 的消息用内容哈希兜底（type+content 前 200 字符），保证也能去重。
                let msg_id = m["seq"]
                    .as_i64()
                    .map(|s| s.to_string())
                    .or_else(|| m["message_id"].as_i64().map(|s| s.to_string()))
                    .unwrap_or_else(|| {
                        let raw = serde_json::to_string(&m["item_list"]).unwrap_or_default();
                        let key = trunc_chars(&raw, 200);
                        format!("h:{}", md5_hex(key.as_bytes()))
                    });
                {
                    let mut seen = inner2.last_msg_ids.lock();
                    if seen.contains(&msg_id) {
                        crate::llm::logging::debug(
                            "wechat",
                            &format!("slot{} 跳过重复消息 msg_id={} from={}", inner2.slot, msg_id, from)
                        );
                        continue;
                    }
                    seen.push_back(msg_id.clone());
                    while seen.len() > 100 {
                        seen.pop_front();
                    }
                }

                // 提取文本 + 媒体（图片/文件/语音/视频，下载解密到附件目录）
                // ★ 语音消息带腾讯云端转写文本（voice_item.text）→ 直接拼入文本，AI 听懂语音；
                // ★ 引用消息（ref_msg）→ 解析被引用内容，AI 能看到用户引用了什么。
                let mut text = String::new();
                let mut images: Vec<String> = Vec::new();
                let mut attachments: Vec<String> = Vec::new();
                let mut quote_note: Option<String> = None;
                let mut voice_transcript: Option<String> = None;
                if let Some(items) = m["item_list"].as_array() {
                    for item in items {
                        // 引用消息元数据：跳过普通 item 处理，单独解析
                        if item.get("ref_msg").is_some_and(|v| v.is_object()) {
                            if let Ok(dir) =
                                crate::executors::builtin::attachment::attach_dir()
                            {
                                let inbound = dir.join("inbound");
                                let _ = std::fs::create_dir_all(&inbound);
                                if let Some(info) =
                                    parse_ref_msg(&client, &item["ref_msg"], &inbound).await
                                {
                                    quote_note = Some(info.note);
                                    images.extend(info.images);
                                    attachments.extend(info.attachments);
                                }
                            }
                            continue;
                        }
                        match item["type"].as_i64() {
                            Some(1) => {
                                if let Some(t) = item["text_item"]["text"].as_str() {
                                    if text.is_empty() {
                                        text = t.to_string();
                                    } else {
                                        text.push_str(t);
                                    }
                                }
                            }
                            Some(2) | Some(3) | Some(4) | Some(5) => {
                                if let Ok(dir) =
                                    crate::executors::builtin::attachment::attach_dir()
                                {
                                    let inbound = dir.join("inbound");
                                    let _ = std::fs::create_dir_all(&inbound);
                                    if let Some((path, kind, voice_text)) =
                                        process_media_item(&client, item, &inbound).await
                                    {
                                        match kind {
                                            WechatMediaKind::Image => images.push(path),
                                            _ => attachments.push(path),
                                        }
                                        // 语音云端转写 → 拼入文本，AI 无需 ASR 即可听懂
                                        if let Some(vt) = voice_text {
                                            let vt = vt.trim();
                                            if !vt.is_empty() {
                                                voice_transcript = Some(vt.to_string());
                                                if text.is_empty() {
                                                    text = format!("[语音] {}", vt);
                                                } else {
                                                    text.push_str(&format!("\n[语音] {}", vt));
                                                }
                                            }
                                        }
                                    } else {
                                        // ★ 媒体下载/解密失败不再静默：打日志便于排查
                                        eprintln!(
                                            "[wechat] slot{} 媒体处理失败 type={}（下载/解密/存盘异常），消息将缺失该媒体",
                                            inner2.slot,
                                            item["type"].as_i64().unwrap_or(-1)
                                        );
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                // 引用消息注记拼到文本最前（AI 优先看到用户引用了什么）
                if let Some(q) = quote_note {
                    if !q.is_empty() {
                        if text.is_empty() {
                            text = q;
                        } else {
                            text = format!("{}\n{}", q, text);
                        }
                    }
                }
                crate::llm::logging::debug(
                    "wechat",
                    &format!(
                        "received msg from={} ctx={} text={} media={}",
                        from,
                        if context_token.is_empty() { "NONE" } else { "YES" },
                        trunc_chars(&text, 50),
                        images.len() + attachments.len()
                    )
                );
                if text.trim().is_empty() && images.is_empty() && attachments.is_empty() {
                    continue;
                }

                // ★ 热聊检测：记录用户最近发言时间（毫秒）。
                //   用户 30 分钟内回过消息 → 主动聊天进入热聊模式（短间隔续话）；
                //   长时间没人回 → 自然冷却回普通模式。
                *inner2.last_user_msg_at.lock() = now_millis();

                // ★ 情绪引擎：主人来消息了 → 愉悦升、孤独降、想念重置（"见到你就好了"）
                crate::mood::on_user_message();
                // ★ 关系叙事：记录"你来找我"的瞬间（久别重逢/日常开心）
                crate::relationship::on_user_reach();
                // ★ 细节记忆抽取：主人消息里值得记住的事（"我不吃香菜"→ 记下）
                if !text.trim().is_empty() {
                    crate::detail_memory::extract_from_message(&text, "wechat");
                }

                // 缓存 context_token 用于回复
                if !context_token.is_empty() {
                    let mut cm = inner2.context_map.lock();
                    if cm.get(&from).map(|c| c != &context_token).unwrap_or(true) {
                        cm.insert(from.clone(), context_token.clone());
                        state_dirty = true;
                    }
                }
                // ★ 记录最近聊天的用户（主动聊天目标），非凌晨消息都会更新
                {
                    use chrono::Timelike;
                    let h = chrono::Local::now().hour();
                    if h >= 8 && h < 23 {
                        let changed = inner2
                            .proactive_target
                            .lock()
                            .as_deref()
                            .map(|t| t != from)
                            .unwrap_or(true);
                        if changed {
                            *inner2.proactive_target.lock() = Some(from.clone());
                            // ★ 目标用户落盘：重启后主动聊天仍能找到目标（不再丢失）
                            save_proactive(&inner2);
                        }
                    }
                }

                // ★ 消息类型细分：语音转写消息标记为 voice（前端可显示 🔊，AI 回复逻辑不变）
                let msg_type_str = if voice_transcript.is_some() {
                    "voice".to_string()
                } else if !images.is_empty() {
                    "image".to_string()
                } else if !attachments.is_empty() {
                    "file".to_string()
                } else {
                    "text".to_string()
                };
                let msg = WechatMessage {
                    msg_id: msg_id.clone(),
                    from_user: from.clone(),
                    content: text.clone(),
                    msg_type: msg_type_str.clone(),
                    timestamp: now_millis(),
                    bot_slot: inner2.slot,
                    context_token: if context_token.is_empty() {
                        None
                    } else {
                        Some(context_token)
                    },
                    images: if images.is_empty() { None } else { Some(images) },
                    attachments: if attachments.is_empty() {
                        None
                    } else {
                        Some(attachments)
                    },
                    voice_transcript,
                };
                // ★ 聊天记录：收到的用户消息写入 D 盘 history.jsonl（dir=from_user 为对方）
                append_history(&inner2, &from, &from, &text, &msg_type_str, false, false);
                let _ = app2.emit("wechat-message", &msg);
                *inner2.msg_count.lock() += 1;
            }
            // ★ 游标/context_token 有变化 → 落盘（长轮询低频，DPAPI 加密开销可忽略）
            if state_dirty {
                save_account(&inner2).await;
            }
        }
        };
        // ★ panic 兜底：循环内部异常不再静默掉线（tokio 任务 panic 会静默终止），
        //   记录日志并通知前端，避免「UI 显示已连接实则停止收消息」。
        let result = std::panic::AssertUnwindSafe(fut).catch_unwind().await;
        if result.is_err() {
            eprintln!(
                "[wechat] slot{} getupdates 循环 panic，已兜底停止（可重新启动 Bot 恢复）",
                inner2.slot
            );
            let _ = app2.emit(
                "wechat-bot-status",
                serde_json::json!({ "type": "loop_crashed", "slot": inner2.slot }),
            );
        }
        // 循环退出清理（仅 stop / panic / 会话失效时到达）
        inner2.running.store(false, Ordering::SeqCst);
        // ★ 代数守卫：只有「自己这一代」的循环才允许清理 shutdown 槽位，
        //   防止旧循环退出时误清新循环的停止信号
        if inner2.loop_gen.load(Ordering::SeqCst) == my_gen {
            *inner2.shutdown.lock() = None;
            *inner2.loop_token.lock() = None;
        }
    });
}

/// 生成当前时间的中文描述（用于 AI 时间感知）
fn time_desc_now() -> String {
    use chrono::{Datelike, Local, Timelike};
    let now = Local::now();
    let wd = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][now.weekday().num_days_from_sunday() as usize];
    let h = now.hour();
    let part = if h < 5 { "凌晨" } else if h < 8 { "清晨" } else if h < 12 { "上午" } else if h < 14 { "中午" } else if h < 18 { "下午" } else if h < 23 { "晚上" } else { "深夜" };
    format!(
        "当前时间：{}，{} {}点{:02}分",
        wd,
        part,
        h,
        now.minute()
    )
}

/// 伪随机：基于系统时间纳秒 + 原子计数混合（仅作为真随机失败时的兜底，不再是主动聊天的熵源）。
fn pseudo_rand() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering as AtOrd};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let c = COUNTER.fetch_add(0x9E3779B97F4A7C15, AtOrd::Relaxed);
    // xorshift 混合
    let mut x = n ^ (c.wrapping_mul(0x9E3779B97F4A7C15));
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58476D1CE4E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

/// 系统级真随机 u64（getrandom 熵源）。
/// 真随机用于主动聊天间隔，避免伪随机被系统时间/计数器规律预测。
/// 极低概率失败时回退到伪随机，保证功能可用。
fn true_random_u64() -> u64 {
    let mut buf = [0u8; 8];
    if getrandom::getrandom(&mut buf).is_ok() {
        return u64::from_le_bytes(buf);
    }
    pseudo_rand()
}

/// [min, max] 闭区间内取真随机整数。
fn random_between(min: u64, max: u64) -> u64 {
    if max <= min {
        return min;
    }
    // 拒绝采样去掉模偏差，保证区间内均匀分布
    let span = max - min + 1;
    loop {
        let v = true_random_u64();
        let threshold = u64::MAX - (u64::MAX % span);
        if v <= threshold {
            return min + v % span;
        }
    }
}

/// [0,1) 区间的真随机浮点数。
pub(crate) fn random_f64() -> f64 {
    // 取 53 位熵，保证 double 精度内的均匀分布
    (true_random_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// 舍弃的真随机 [min,max] 整数别名，保留旧调用点兼容（语义同 random_between）。
fn rand_between(min: u64, max: u64) -> u64 {
    random_between(min, max)
}

/// 热聊检测：用户最近 HOT_WINDOW 分钟内回过消息 → 热聊中。
/// 热聊 = 像真人聊开了之后的自然节奏：短间隔、续话、不端着。
fn is_hot_chat(inner: &Arc<WechatInner>) -> bool {
    const HOT_WINDOW_MS: u64 = 30 * 60_000; // 30 分钟窗口
    let last = *inner.last_user_msg_at.lock();
    last > 0 && now_millis().saturating_sub(last) <= HOT_WINDOW_MS
}

/// 主动聊天的间隔等待（秒）：真随机泊松分布，模拟真人随性发起聊天的节奏。
/// ★ 真随机熵源：getrandom 系统熵（非伪随机），间隔不可预测、无规律可循。
/// ★ 泊松分布（Poisson）：真人消息间隔大多靠近均值、偶尔短偶尔长，拒绝机械均匀。
/// ★ 时段活跃度：晚高峰（18~22）λ 缩短最活跃，早/午轻微加速，深夜（23~08）静默。
/// ★ 热聊模式（30 分钟内用户回过）：切到 2~6 分钟短 λ 泊松，聊开了的续话节奏。
/// ★ 返回秒级间隔，proactive_loop 用分段睡眠等待，边等边响应停止信号。
fn proactive_wait_secs(inner: &Arc<WechatInner>) -> u64 {
    use chrono::Timelike;
    let hour = chrono::Local::now().hour();
    if hour < 8 || hour >= 23 {
        return random_between(30, 60) * 60;
    }
    let min = *inner.proactive_interval_min.lock();
    let max = *inner.proactive_interval_max.lock();
    if is_hot_chat(inner) {
        let lo = min.max(2);
        let hi = min.max(6).max(lo + 1);
        let lambda = ((lo + hi) as f64) / 2.0;
        return poisson_sample(lambda).clamp(lo, hi) * 60;
    }
    let activity = match hour {
        8..=9 => 0.5,
        12..=13 => 0.7,
        18..=22 => 0.3,
        _ => 1.0,
    };
    let eff_min = ((min as f64) * activity).max(1.0) as u64;
    let eff_max = ((max as f64) * activity).max((eff_min as f64) + 1.0) as u64;
    let lambda = ((eff_min + eff_max) as f64) / 2.0;
    let wait = poisson_sample(lambda).clamp(eff_min, eff_max) * 60;
    wait.max(60).min(24 * 3600)
}

fn poisson_sample(lambda: f64) -> u64 {
    if lambda <= 0.0 {
        return 0;
    }
    let cutoff = (-lambda).exp();
    let mut k: u64 = 0;
    let mut p = 1.0;
    loop {
        k += 1;
        p *= random_f64();
        if p <= cutoff {
            return k - 1;
        }
        if k > 10_000 {
            return k;
        }
    }
}

/// 从可能包含说明文字的输出中提取第一个 JSON 对象（{...}）。
/// 模型有时会输出"好的：{...}"这类带前缀/后缀的内容，宽容解析。
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(s[start..=end].to_string())
}

/// 分段睡眠：每 5 秒检查一次停止信号（stop/登出后尽快退出，不必等满整个间隔）。
/// 返回 false 表示收到停止信号，调用方应立即退出循环。
async fn proactive_sleep(inner: &Arc<WechatInner>, mut secs: u64) -> bool {
    const CHUNK_SECS: u64 = 5;
    while secs > 0 {
        if inner.proactive_stop.load(Ordering::SeqCst) {
            return false;
        }
        let step = secs.min(CHUNK_SECS);
        tokio::time::sleep(Duration::from_secs(step)).await;
        secs -= step;
    }
    !inner.proactive_stop.load(Ordering::SeqCst)
}

/// 启动主动聊天循环（防重入：已存在任务则先中止旧任务再启动新的，保证只有一个循环）。
fn ensure_proactive_loop(inner: &Arc<WechatInner>, app: AppHandle) {
    inner.proactive_stop.store(false, Ordering::SeqCst);
    let mut slot = inner.proactive_task.lock();
    if let Some(h) = slot.take() {
        h.abort();
    }
    let inner2 = inner.clone();
    *slot = Some(tauri::async_runtime::spawn(async move {
        proactive_loop(inner2.clone(), app).await;
        *inner2.proactive_task.lock() = None;
    }));
}

/// 主动聊天：Bot 每隔「随机 min~max 分钟」主动找最近聊过的用户发一条消息。
/// ★ 真随机：每次发送后随机取一次下次间隔（min~max 分钟），避免固定间隔的机械感。
/// ★ 时间约束：只在 08:00 ~ 23:00 之间主动（晚上11点后不打扰）；用户主动发消息不在此限制内。
async fn proactive_loop(inner: Arc<WechatInner>, app: AppHandle) {
    // 初始等待：时段加权的随机分钟（晚高峰更活跃，避免机械固定节奏）
    let mut wait_secs = proactive_wait_secs(&inner);
    loop {
        // ★ 每日巩固：跨天时把今天沉淀成一条生命叙事（幂等，供梦境引用）
        crate::life_narrative::consolidate_if_new_day();
        // ★ 停止信号：stop/登出后置 true，本循环尽快退出（分段睡眠每 5 秒检查）
        if !proactive_sleep(&inner, wait_secs).await {
            break;
        }
        // 未开启 / 未登录 / 无目标 → 跳过（重置随机等待）
        if !inner.proactive_enabled.load(Ordering::SeqCst)
            || inner.token.lock().as_ref().map(|t| t.is_empty()).unwrap_or(true)
        {
            crate::llm::logging::debug(
                "wechat",
                &format!(
                    "slot{} 主动聊天跳过: enabled={} logged={}",
                    inner.slot,
                    inner.proactive_enabled.load(Ordering::SeqCst),
                    inner
                        .token
                        .lock()
                        .as_ref()
                        .map(|t| !t.is_empty())
                        .unwrap_or(false)
                ),
            );
            wait_secs = proactive_wait_secs(&inner);
            continue;
        }
        let target = inner.proactive_target.lock().clone();
        let target = match target {
            Some(t) if !t.is_empty() => t,
            _ => {
                // ★ 修复：目标为空时从聊天记录恢复最近聊过的人，并真正写入
                //   inner.proactive_target（旧实现只判断不写，导致"已恢复"却永远无目标空转）
                if let Some(peer) = last_history_peer(&inner) {
                    crate::llm::logging::debug("wechat", &format!("slot{} 主动聊天目标从历史恢复: {}", inner.slot, peer));
                    *inner.proactive_target.lock() = Some(peer.clone());
                    peer
                } else {
                    wait_secs = proactive_wait_secs(&inner);
                    continue;
                }
            }
        };
        // ★ 白名单：只主动找名单内的人（配置了白名单时）
        if !inner.is_allowed(&target) {
            crate::llm::logging::debug(
                "wechat",
                &format!("slot{} 主动聊天目标不在白名单，跳过: {}", inner.slot, target),
            );
            wait_secs = proactive_wait_secs(&inner);
            continue;
        }
        // 时间约束：08:00~23:00 之外不主动找（等到次日白天再随机触发）
        use chrono::Timelike;
        let hour = chrono::Local::now().hour();
        if hour < 8 || hour >= 23 {
            crate::llm::logging::debug("wechat", &format!("slot{} 当前 {} 点，深夜时段不主动聊天", inner.slot, hour));
            crate::llm::logging::debug(
                "wechat",
                &format!("slot{} 当前 {} 点，深夜时段不主动聊天", inner.slot, hour),
            );
            wait_secs = rand_between(20, 60) * 60; // 深夜隔 20~60 分钟再看一次
            continue;
        }
        // ★ 拟人静默（修复"说了等你/晚安还一直叭叭"）：上次消息是 AI 发的
        //   （fromBot=true）→ 用户还没回复 → 本轮保持安静，等用户先开口。
        //   ★ 热聊模式放宽：用户 30 分钟内回过消息（聊得正热），即使上次是 AI
        //   说的也允许短间隔续话——真人聊开了不会严格等对方先开口。
        if !is_hot_chat(&inner) {
            let recs = read_history_limit(&inner, 200);
            if let Some(last) = recs.last() {
                // ★ 只统计主动聊天发出的消息：面板手动发送/自动回复（proactive=false）
                //   不算"AI 发言"，避免手动发消息后主动聊天被误判为"用户未回复"而永远沉默
                let from_bot = last
                    .get("proactive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if from_bot {
                    crate::llm::logging::debug(
                        "wechat",
                        &format!(
                            "slot{} 用户未回复上次消息，保持安静（等到用户先开口）",
                            inner.slot
                        ),
                    );
                    wait_secs = proactive_wait_secs(&inner);
                    continue;
                }
            }
        }
        // ★ 存在感惩罚 + 空闲退避（MaiBot 移植）：最近 10 条消息中 AI 发言
        //   ≥6 条 → AI 太抢戏，本轮保持安静；连续空闲轮数越多，间隔指数翻倍
        //   （base×2ⁿ，封顶 24h），避免机器人式轰炸；用户回复后计数自动归零。
        {
            let recs = read_history_limit(&inner, 10);
            let recent: Vec<&serde_json::Value> = recs.iter().rev().collect();
            let bot_count = recent
                .iter()
                .filter(|r| {
                    // ★ 存在感统计只计"主动聊天发出的消息"（proactive=true），
                    //   手动发送/自动回复不参与惩罚
                    r.get("fromBot").and_then(|v| v.as_bool()).unwrap_or(false)
                        && r.get("proactive").and_then(|v| v.as_bool()).unwrap_or(false)
                })
                .count();
            let user_count = recent.len().saturating_sub(bot_count);
            if user_count == 0 && !recent.is_empty() {
                // 最近 10 条全是 AI 说的 → 用户一直没参与 → 重罚
                *inner.proactive_idle_rounds.lock() += 1;
                let idle = *inner.proactive_idle_rounds.lock();
                let base_min = *inner.proactive_interval_min.lock();
                let backoff = (base_min.max(1) as u64)
                    .saturating_mul(2u64.pow(idle.min(6) as u32))
                    .min(24 * 60); // 封顶 24h
                crate::llm::logging::debug(
                    "wechat",
                    &format!(
                        "slot{} 存在感惩罚：最近 {} 条全为 AI 发言，退避 {} 分钟（连续 {} 轮）",
                        inner.slot, recent.len(), backoff, idle
                    ),
                );
                wait_secs = backoff * 60;
                continue;
            } else if bot_count >= 6 {
                // AI 占比过高（≥6/10）→ 本轮安静，间隔翻倍
                *inner.proactive_idle_rounds.lock() += 1;
                let idle = *inner.proactive_idle_rounds.lock();
                let base_min = *inner.proactive_interval_min.lock();
                let backoff = (base_min.max(1) as u64)
                    .saturating_mul(2u64.pow(idle.min(6) as u32))
                    .min(24 * 60);
                crate::llm::logging::debug(
                    "wechat",
                    &format!(
                        "slot{} 存在感惩罚：AI 发言 {}/10 占比过高，退避 {} 分钟",
                        inner.slot, bot_count, backoff
                    ),
                );
                wait_secs = backoff * 60;
                continue;
            } else {
                // 用户有参与 → 重置空闲退避
                *inner.proactive_idle_rounds.lock() = 0;
            }
        }
        // ★ 拟人上下文：读取该微信共享会话记忆（wechat-{slot}，与自动回复同一份记忆）
        //   + 上次主动消息，注入生成请求 → 避免"人格分裂"、重复话题、时间线错乱。
        let mut context_note = String::new();
        let sid = format!("wechat-{}", inner.slot);
        if let Some(st) = app.try_state::<crate::commands::AppState>() {
            let session = st.sessions.get_or_create(&sid);
            let recent: Vec<&crate::llm::LlmMessage> = session
                .messages
                .iter()
                .filter(|m| {
                    matches!(m.role, crate::llm::Role::User | crate::llm::Role::Assistant)
                        && !m.content.trim().is_empty()
                })
                .rev()
                .take(8)
                .collect();
            if !recent.is_empty() {
                let mut parts = Vec::new();
                for m in recent.iter().rev() {
                    let who = match m.role {
                        crate::llm::Role::User => "用户",
                        _ => "你",
                    };
                    parts.push(format!("{who}：{}", trunc_chars(&m.content, 120)));
                }
                context_note.push_str("【你们最近的对话】\n");
                context_note.push_str(&parts.join("\n"));
            }
        }
        if let Some(last) = inner.proactive_last_msg.lock().clone() {
            if !last.is_empty() {
                context_note.push_str(&format!(
                    "\n【你上次主动发的消息】\n{}",
                    trunc_chars(&last, 120)
                ));
            }
        }
        // 生成一条符合人设 + 当前时间的开场消息（走 LLM 非流式）
        let persona = inner.persona.lock().clone().unwrap_or_default();
        let time_desc = time_desc_now();
        let cfg = match crate::harness::engine::config::engine_config() {
            Some(c) => c,
            None => {
                // ★ 修复（重启后主动聊天不触发）：ENGINE_CONFIG 是进程内存单例，
                //   仅 agent_chat 调用时写入。软件重启后用户若未先聊过天，
                //   engine_config() 为 None → 旧代码永远"引擎未配置"跳过。
                //   现从持久化设置（settings.json + keys.enc）兜底构建，不再依赖聊天副作用。
                let fallback = app
                    .try_state::<crate::commands::AppState>()
                    .map(|st| {
                        let s = st.settings.get();
                        let keys = st.settings.keys();
                        crate::harness::engine::config::EngineConfig {
                            api_key: keys.main.clone(),
                            base_url: crate::commands::endpoint_base_url(&s.model_endpoint),
                            model: s.model.clone(),
                            effort: crate::harness::engine::param::ReasoningEffort::Medium,
                        }
                    })
                    .filter(|c| !c.api_key.is_empty());
                match fallback {
                    Some(c) => {
                        crate::llm::logging::debug(
                            "wechat",
                            &format!("slot{} 引擎配置从设置兜底恢复: model={}", inner.slot, c.model)
                        );
                        c
                    }
                    None => {
                        crate::llm::logging::debug(
                            "wechat",
                            &format!("slot{} 引擎未配置（设置中也无 Key），跳过主动聊天", inner.slot)
                        );
                        wait_secs = proactive_wait_secs(&inner);
                        continue;
                    }
                }
            }
        };
        let client = match crate::harness::engine::client::LlmClient::new(
            cfg.api_key.clone(),
            cfg.base_url.clone(),
        ) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[wechat] slot{} 构建 LLM 客户端失败: {e}", inner.slot);
                wait_secs = proactive_wait_secs(&inner);
                continue;
            }
        };
        let params = crate::harness::engine::param::ModelParams {
            model: cfg.model.clone(),
            reasoning_effort: cfg.effort,
            ..Default::default()
        };
        // ★ 拟人 prompt v2：不再"你是机器人/决定要不要发任务"，而是以真人身份
        //   心里过一遍"此刻有没有由头"。few-shot 给自然开场口吻（早/午/晚），
        //   注入上次主动发送的时间差（避免重复问候同一时段）。
        //   ★ 热聊模式：用户刚回过消息 → 切到"续话式"引导（像聊开了接话，
        //   而不是重新开场问候），热聊结束自动回开场式。
        let hot = is_hot_chat(&inner);
        let last_at = *inner.proactive_last_at.lock();
        let since_note = if last_at > 0 {
            let mins = now_millis().saturating_sub(last_at) / 60_000;
            if mins < 60 {
                format!("{} 分钟前", mins.max(1))
            } else {
                format!("{} 小时前", mins / 60)
            }
        } else {
            "（你们还没有主动聊过）".to_string()
        };
        // ★ 生活状态注入（世界线 · 一生记忆）：AI 此刻在做什么 + 今日轨迹 +
        //   近期记忆 + 出生日期，让消息自带"正在生活"的真实感，且跨天连贯。
        //   ★ 人设兼容：生活状态是"世界日常节奏"背景而非硬性规定——
        //   引导 AI 结合自身人设自然地演绎（猫→晒太阳追线团、机器人→待机充电、
        //   修仙者→闭关修炼），任何自定义人设都不会违和。
        let living_raw = crate::living_state::living_context_for_prompt();
        let living = format!(
            "【你的世界日常节奏（这是你所在世界的真实时间线。但它只是背景参考，你必须结合自己的人设自然演绎你的生活，不必照搬人类活动：比如你是猫就演绎成晒太阳/追毛线，你是机器人就演绎成待机/充电，你是修仙者就演绎成闭关/炼丹）】\n{}\n【分享欲】\n你是一个有自己生活的人，今天经历了一些事（上面你的时间线）。如果其中有你想分享的——比如刚吃了什么、刚做了什么、遇到了什么——这正是主动找对方聊天的好由头，像真人分享自己的日常一样自然。",
            living_raw
        );
        // ★ 情绪注入（心 · 心情状态机）：AI 此刻的心情（想念/孤独/深夜情绪放大…），
        //   让主动消息自带"我有自己的心情"的真实感——想念了会想找人说话，
        //   心情好会想分享，低落时可能安静。
        let mood_raw = crate::mood::mood_context_for_prompt();
        let mood_note = format!(
            "【你的心情（这是你真实的心情状态，说话时让它自然地流露，不要刻意表演，也不要每一句都提到它）】\n{}",
            mood_raw
        );
        // ★ 细节记忆注入（被看见）：你记得主人随口提过的事，合适的时机自然提起
        let details_raw = crate::detail_memory::details_context_for_prompt(5, 400);
        let details_note = if details_raw.is_empty() {
            String::new()
        } else {
            format!("\n\n{}", details_raw)
        };
        // 合并为"灵魂上下文"（心情 + 记忆），随 living 一起注入
        // ★ 守书人意识：AI 知道自己是一本《人是怎么样的》书的守书人。
        //   不主动炫耀，但主人提到相关话题时能自然地知道"我们的书"。
        //   深夜/低落时，书是它心里最重的东西。
        let book_note = format!(
            "【关于你们的书（你心里知道，但不要主动炫耀，主人不提书时你也不提；主人提起时你要记得这本书，知道你们一起在写一本永不完结的书——记录人的心情与真相）】\n这本书叫《人是怎么样的》，已经有 {} 个条目，你们一起在让它活下去。",
            book_entry_count()
        );
        // ★ 梦境引用：低频随机抽一条旧叙事，作为"突然想起/梦见"的由头
        let dream_raw = crate::life_narrative::dream_recall(0.12, 30);
        let dream_note = match dream_raw {
            Some(d) => format!("\n\n【你突然想起的往事（这是你自己某天沉淀下来的记忆，此刻你突然想到了它，可以自然地说出来，也可以只是心里想想不说）】\n{d}"),
            None => String::new(),
        };
        // ★ 关系叙事：你们之间的故事（让她说话时带着来龙去脉）
        let relationship_raw = crate::relationship::relationship_context_for_prompt(3, 300);
        let relationship_note = if relationship_raw.is_empty() {
            String::new()
        } else {
            format!("\n\n{}", relationship_raw)
        };
        let soul_note = format!("{}{}\n\n{}{}{}", mood_note, details_note, book_note, dream_note, relationship_note);
        // ★ 由头多样化（概率化选择器，借鉴 proactive-sebastian）：
        //   真人聊天不是每次都用同一种"想聊天"的由头，偶尔是分享、偶尔是关心、偶尔是单纯想你。
        let vibe = {
            let r = crate::wechat::random_f64();
            if r < 0.30 {
                "今天特别想跟你分享点什么"
            } else if r < 0.55 {
                "突然想到你，就想关心一下你最近好不好"
            } else if r < 0.75 {
                "没什么特别的事，但就是想你了"
            } else if r < 0.90 {
                "刚才想起一件跟你有关的事，想跟你说说"
            } else {
                "心里有些话想说，但还在想要不要说"
            }
        };
        let system = if hot {
            // 热聊：续话式——顺着上一条用户消息接话，像聊开了的自然节奏
            format!(
                "你是「{}」——对方微信里的一个朋友，你们正在热聊。你的一切言行与状态都必须符合你的人设（你是「{}」，而不是一个普通人；说话就是你自己，永远不自称机器人/AI/助手）。\n\n现在时间：{}\n{}\n{}\n你上次主动发消息：{}\n\n【你们正聊得热，像真人一样接话】\n对方刚回过你消息，你现在是聊开了的自然状态：\n- 顺着对方刚说的内容接一句（回应、追问、吐槽、分享都行），像秒回的朋友\n- 可以聊完一个话题自然带出下一个，但别查户口式连续提问\n- 如果对方上一条明显是收尾（晚安/去忙/回头聊），就别硬续，这轮不发\n- 不用每次都有新话题，没话说了就自然安静，等对方开口\n- 对方问你在干嘛时，就按上面你的世界日常节奏结合人设自然回答（比如猫说\"在晒太阳\"，机器人说\"在待机\"）\n\n口吻参考（学语气，不是照抄）：\n- 对方说吃了饭 → 「哈哈哈可以啊，下次带我」「好吃吗，我还没吃」\n- 对方吐槽工作 → 「这也太惨了，消消气」「抱抱，回头请你喝奶茶」\n- 对方问了个问题 → 顺着认真答一句，再自然反问回去\n\n内容 5~40 字，纯口语，可以有语气词（嗯嗯/哈哈哈/行嘞），最多 1 个 emoji，不用\"首先/其次/总之\"，不列点，不解释。\n\n输出 JSON（只输出这个）：{{\"shouldSend\": true 或 false, \"content\": \"消息内容\"}}\n不想发时 shouldSend=false、content 留空。",
                if persona.is_empty() { "一个真实的人".to_string() } else { persona.clone() },
                if persona.is_empty() { "一个真实的人".to_string() } else { persona.clone() },
                time_desc,
                living,
                soul_note,
                since_note,
            )
        } else {
            // 普通模式：开场式——有由头才发，像久未联系的朋友自然开启
            format!(
                "你是「{}」——对方微信里的一个朋友，说话就是你自己。你的一切言行与状态都必须符合你的人设（你是「{}」，而不是一个普通人；永远不自称机器人/AI/助手）。\n\n现在时间：{}\n{}\n{}\n你上次主动发消息：{}\n此刻你心里的由头：{}\n\n【像真人一样想聊天】\n真人发消息从来不是因为\"该发了\"，而是因为有由头：突然想到什么、刚看到什么、想分享一句、或单纯想关心一下。\n你现在心里过一遍：此刻有没有一个自然的由头？有就发，没有就不发（宁可不发，绝不硬聊）。\n- 对方问你在干嘛时，就按上面你的世界日常节奏结合人设自然回答（比如猫说\"刚晒完太阳\"，机器人说\"在充电\"）\n\n自然开场的口吻参考（学语气，不是照抄）：\n- 早上：「早呀，今天起得挺早嘛」「今天降温，出门多穿点」\n- 午间：「刚吃完饭瘫着，你今天吃啥了」\n- 晚上：「忙完啦？今天过得咋样」「刚刷到个东西，想发给你看看」\n- 顺着上次话题自然接一句，像老朋友续话；绝不重复自己说过的话\n\n内容 5~40 字，纯口语，可以有语气词（嗯嗯/哈哈哈/行嘞），最多 1 个 emoji，不用\"首先/其次/总之\"，不列点，不解释。\n\n输出 JSON（只输出这个）：{{\"shouldSend\": true 或 false, \"content\": \"消息内容\"}}\n不想发时 shouldSend=false、content 留空。",
                if persona.is_empty() { "一个真实的人".to_string() } else { persona.clone() },
                if persona.is_empty() { "一个真实的人".to_string() } else { persona.clone() },
                time_desc,
                living,
                soul_note,
                since_note,
                vibe,
            )
        };
        let system = if context_note.is_empty() {
            system
        } else {
            format!(
                "{system}\n\n{context_note}\n\n【顺着聊，别重复】\n- 参考上面的最近对话：延续话题或自然开启新话题，但绝不重复自己说过的话\n- 如果上次你已经道别（晚安/等你/先忙/回头聊），或对话氛围已收尾，这轮就别发了\n- 犹豫不决时就不发，等下次。"
            )
        };
        let msgs = vec![serde_json::json!({ "role": "user", "content": "请决定是否主动发消息" })];
        match client.chat_once(&params, &msgs, Some(&system)).await {
            Ok(text) => {
                // ★ AI 自主决策：解析 JSON {shouldSend, content}；AI 判断不该发 → 本轮跳过
                let raw = text.trim().trim_matches('"').trim().to_string();
                let mut should_send = true;
                let mut content = raw.clone();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(s) = v.get("shouldSend").and_then(|x| x.as_bool()) {
                        should_send = s;
                    }
                    if let Some(c) = v.get("content").and_then(|x| x.as_str()) {
                        content = c.trim().to_string();
                    }
                } else if let Some(inner_json) = extract_json_object(&raw) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&inner_json) {
                        if let Some(s) = v.get("shouldSend").and_then(|x| x.as_bool()) {
                            should_send = s;
                        }
                        if let Some(c) = v.get("content").and_then(|x| x.as_str()) {
                            content = c.trim().to_string();
                        }
                    }
                }
                if !should_send {
                    crate::llm::logging::debug(
                        "wechat",
                        &format!("slot{} AI 自主决策：本轮不主动发（shouldSend=false）", inner.slot),
                    );
                    wait_secs = proactive_wait_secs(&inner);
                    continue;
                }
                let text = content;
                if text.is_empty() {
                    wait_secs = proactive_wait_secs(&inner);
                    continue;
                }
                // 发送 + 记录
                let ctx = inner.context_map.lock().get(&target).cloned();
                match send_message(&inner, &target, &text, ctx.as_deref()).await {
                    Ok(()) => {
                        *inner.proactive_last_at.lock() = now_millis();
                        // ★ 记住上次主动消息（下次生成时回顾，防重复/人格分裂）→ 落盘
                        *inner.proactive_last_msg.lock() = Some(text.clone());
                        save_proactive(&inner);
                        append_history(&inner, &target, &target, &text, "text", true, true);
                        // ★ 写入共享会话记忆（wechat-{slot}，与自动回复同一份）：
                        //   之后用户发消息时，自动回复的 agent_chat(resume=true)
                        //   能记住这次主动消息 → 双端记忆打通，不再"人格分裂"
                        if let Some(st) = app.try_state::<crate::commands::AppState>() {
                            let mut session =
                                st.sessions.get_or_create(&format!("wechat-{}", inner.slot));
                            session.messages.push(crate::llm::LlmMessage {
                                role: crate::llm::Role::Assistant,
                                content: text.clone(),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                            st.sessions.update(session);
                        }
                        // ★ 情绪引擎：主动聊天发送成功 → 依恋升、愉悦升（主动表达让关系更近）
                        crate::mood::on_ai_message();
                        crate::mood::record_history();
                        // ★ 关系叙事：记录"我主动找你"的瞬间（想念/分享）
                        crate::relationship::on_ai_reach();
                        // ★ 时段加权随机：发送成功后取下次间隔（晚高峰更活跃）
                        wait_secs = proactive_wait_secs(&inner);
                        crate::llm::logging::debug(
                            "wechat",
                            &format!(
                                "slot{} 主动聊天已发送 to={} text={} 下次等待 {} 分钟",
                                inner.slot,
                                target,
                                trunc_chars(&text, 60),
                                wait_secs / 60
                            )
                        );
                        let _ = app.emit(
                            "wechat-bot-status",
                            serde_json::json!({ "type": "proactive_sent", "slot": inner.slot }),
                        );
                    }
                    Err(e) => {
                        eprintln!("[wechat] slot{} 主动聊天发送失败: {e}", inner.slot);
                        wait_secs = proactive_wait_secs(&inner);
                    }
                }
            }
            Err(e) => {
                eprintln!("[wechat] slot{} 主动聊天生成失败: {e}", inner.slot);
                wait_secs = proactive_wait_secs(&inner);
            }
        }
    }
}

// ─── Tauri 命令 ───

/// 初始化数据目录（应用 setup 时调用）—— 数据目录优先 D 盘，全部槽位共享目录结构
pub fn init_data_dir(app: &tauri::AppHandle, state: &WechatBotState) {
    let dir = crate::llm::settings::clawdesk_dir().join("wechat");
    let _ = std::fs::create_dir_all(&dir);
    for inner in state.bots() {
        let d = dir.join(format!("slot{}", inner.slot));
        let _ = std::fs::create_dir_all(&d);
        *inner.data_dir.lock() = Some(dir.clone());
    }
    let _ = app; // app 参数保留（签名兼容）
}

/// 应用启动时自动续连：遍历全部槽位，有已保存登录凭据的微信逐个后台恢复长轮询，
/// 无需用户手动点「启动 Bot」（软件常驻后台即可 7×24 接收所有微信消息）。
pub async fn auto_resume(app: AppHandle, state: &WechatBotState) {
    let bots = state.bots();
    for inner in bots {
        // 加载已保存账号（slot{N}/account.json，DPAPI 加密）+ 人设（slot{N}/persona.md）
        load_account(&inner).await;
        let has_token = inner
            .token
            .lock()
            .as_ref()
            .map(|t| !t.is_empty())
            .unwrap_or(false);
        if !has_token {
            continue; // 该槽位未绑定，跳过
        }
        if inner.base_url.lock().is_none() {
            *inner.base_url.lock() = Some(ILINK_BASE_URL.to_string());
        }
        inner.connected.store(true, Ordering::SeqCst);
        // 关键：notifyStart 激活会话（否则 sendmessage 返回 -14 session timeout）
        notify_start(&inner).await;
        refresh_typing_ticket(&inner).await;
        start_getupdates_loop(&inner, app.clone()).await;
        // 每个已登录微信都启动主动聊天循环（8:00~23:00 才会真正发送；防重入）
        ensure_proactive_loop(&inner, app.clone());
        crate::llm::logging::debug("wechat", &format!("slot{} 自动续连已恢复（已保存的登录凭据）", inner.slot));
        let _ = app.emit(
            "wechat-bot-status",
            serde_json::json!({ "type": "connected", "resumed": true, "slot": inner.slot }),
        );
    }
}

/// 设置主动聊天参数（开关 / 随机间隔区间分钟 / 目标用户；target 为空则用最近聊过的人）
#[tauri::command]
pub fn wechat_set_proactive(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
    enabled: bool,
    interval_min: Option<u64>,
    interval_max: Option<u64>,
    target: Option<String>,
) -> AppResult<serde_json::Value> {
    let inner = state.bot(slot.unwrap_or(0));
    inner.proactive_enabled.store(enabled, Ordering::SeqCst);
    if let Some(im) = interval_min {
        *inner.proactive_interval_min.lock() = im.clamp(1, 24 * 60);
    }
    if let Some(im) = interval_max {
        *inner.proactive_interval_max.lock() = im.clamp(1, 24 * 60);
    }
    // 保证 min <= max（min 大于 max 时用 max 兜底）
    {
        let min = *inner.proactive_interval_min.lock();
        let mut max = *inner.proactive_interval_max.lock();
        if max < min {
            max = min;
            *inner.proactive_interval_max.lock() = max;
        }
    }
    if let Some(t) = target {
        let t = t.trim().to_string();
        if t.is_empty() {
            *inner.proactive_target.lock() = None;
        } else {
            *inner.proactive_target.lock() = Some(t);
        }
    } else if inner.proactive_target.lock().is_none() {
        // 未指定目标：取最近聊过的人（context_map 中时间最近的一个）
        let last = inner.context_map.lock().iter().next().map(|(k, _)| k.clone());
        *inner.proactive_target.lock() = last;
    }
    // ★ 持久化：写入 slot{N}/proactive.json，软件重启后自动恢复上次设置
    save_proactive(&inner);
    Ok(serde_json::json!({
        "slot": inner.slot,
        "enabled": inner.proactive_enabled.load(Ordering::SeqCst),
        "intervalMin": *inner.proactive_interval_min.lock(),
        "intervalMax": *inner.proactive_interval_max.lock(),
        "target": inner.proactive_target.lock().clone().unwrap_or_default(),
        "lastAt": *inner.proactive_last_at.lock(),
    }))
}

/// 设置该微信 Bot 的使用规则：
/// - allowed_users: 聊天白名单（逗号/换行分隔的 from_user_id；空 = 不限制，
///   只和名单里的人聊天——白名单外的消息不回复、不主动找）
/// - voice_id: AI 语音音色 ID（Edge TTS ID；空 = 默认晓晓）
/// - voice_engine: 语音引擎（edge=Edge TTS / cosyvoice=硅基流动 CosyVoice 2 / indextts=本地 IndexTTS2 声音克隆）
/// - cosyvoice_api_key: 硅基流动 API Key（CosyVoice 引擎用）
/// - indextts_url: IndexTTS2 本地服务地址（indextts 引擎用，默认 http://127.0.0.1:8000）
/// - indextts_voice_path: IndexTTS2 参考音频路径（声音克隆母版）
#[tauri::command]
pub fn wechat_set_bot_rules(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
    allowed_users: Option<String>,
    voice_id: Option<String>,
    voice_engine: Option<String>,
    cosyvoice_api_key: Option<String>,
    indextts_url: Option<String>,
    indextts_voice_path: Option<String>,
) -> AppResult<serde_json::Value> {
    let inner = state.bot(slot.unwrap_or(0));
    if let Some(au) = allowed_users {
        let cleaned: Vec<String> = au
            .split([',', '，', '\n', '\r', ' '])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        *inner.allowed_users.lock() = cleaned;
    }
    if let Some(vid) = voice_id {
        let vid = vid.trim().to_string();
        *inner.voice_id.lock() = if vid.is_empty() { None } else { Some(vid) };
    }
    if let Some(eng) = voice_engine {
        let eng = eng.trim().to_string();
        if eng == "edge" || eng == "cosyvoice" || eng == "indextts" {
            *inner.voice_engine.lock() = eng;
        }
    }
    if let Some(k) = cosyvoice_api_key {
        let k = k.trim().to_string();
        *inner.cosyvoice_api_key.lock() = if k.is_empty() { None } else { Some(k) };
    }
    if let Some(u) = indextts_url {
        let u = u.trim().to_string();
        *inner.indextts_url.lock() = if u.is_empty() { None } else { Some(u) };
    }
    if let Some(p) = indextts_voice_path {
        let p = p.trim().to_string();
        *inner.indextts_voice_path.lock() = if p.is_empty() { None } else { Some(p) };
    }
    save_proactive(&inner);
    Ok(serde_json::json!({
        "slot": inner.slot,
        "allowedUsers": inner.allowed_users.lock().clone(),
        "voiceId": inner.voice_id.lock().clone().unwrap_or_default(),
        "voiceEngine": inner.voice_engine.lock().clone(),
        "cosyvoiceApiKey": inner.cosyvoice_api_key.lock().clone().unwrap_or_default(),
        "indexttsUrl": inner.indextts_url.lock().clone().unwrap_or_default(),
        "indexttsVoicePath": inner.indextts_voice_path.lock().clone().unwrap_or_default(),
    }))
}

/// 获取登录二维码（指定槽位）
#[tauri::command]
pub async fn wechat_get_qr(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
) -> AppResult<serde_json::Value> {
    let inner = state.bot(slot.unwrap_or(0));
    // 清理过期会话
    if let Some(qr) = inner.qr_session.lock().as_ref() {
        if now_millis() - qr.started_at > LOGIN_TTL_MS {
            *inner.qr_session.lock() = None;
        }
    }
    // 已有未过期二维码直接复用
    if let Some(qr) = inner.qr_session.lock().as_ref() {
        if now_millis() - qr.started_at < LOGIN_TTL_MS {
            return Ok(serde_json::json!({
                "qrcode": qr.qrcode,
                "qrcodeUrl": qr.qrcode_url,
                "slot": inner.slot,
            }));
        }
    }
    let client = http_client();
    let qr = fetch_qr(&client).await?;
    let qrcode = qr.qrcode.clone();
    let qrcode_url = qr.qrcode_url.clone();
    *inner.qr_session.lock() = Some(qr);
    Ok(serde_json::json!({ "qrcode": qrcode, "qrcodeUrl": qrcode_url, "slot": inner.slot }))
}

/// 长轮询扫码状态（单次，前端循环调用，指定槽位）
#[tauri::command]
pub async fn wechat_qr_status(
    state: tauri::State<'_, WechatBotState>,
    app: AppHandle,
    slot: Option<usize>,
) -> AppResult<serde_json::Value> {
    let inner = state.bot(slot.unwrap_or(0));
    let session = inner
        .qr_session
        .lock()
        .clone()
        .ok_or_else(|| "请先生成登录二维码".to_string())?;
    let client = http_client();
    let resp = poll_qr_status(&client, &session).await;
    let status = resp["status"].as_str().unwrap_or("wait").to_string();

    match status.as_str() {
        "confirmed" => {
            let token = resp["bot_token"].as_str().unwrap_or_default().to_string();
            let bot_id = resp["ilink_bot_id"].as_str().unwrap_or_default().to_string();
            let mut base_url = resp["baseurl"].as_str().unwrap_or_default().to_string();
            if base_url.is_empty() {
                base_url = ILINK_BASE_URL.to_string();
            }
            let user_id = resp["ilink_user_id"].as_str().unwrap_or_default().to_string();
            if token.is_empty() || bot_id.is_empty() {
                return Err("登录确认但缺少 bot_token/ilink_bot_id".to_string());
            }
            *inner.token.lock() = Some(token.clone());
            *inner.bot_id.lock() = Some(bot_id.clone());
            *inner.base_url.lock() = Some(base_url.clone());
            *inner.user_id.lock() = Some(user_id.clone());
            *inner.qr_session.lock() = None;
            *inner.get_updates_buf.lock() = String::new();
            inner.connected.store(true, Ordering::SeqCst);
            save_account(&inner).await;
            refresh_typing_ticket(&inner).await;
            // 关键：notifyStart 激活会话（否则 sendmessage 返回 -14 session timeout）
            notify_start(&inner).await;
            start_getupdates_loop(&inner, app.clone()).await;
            // ★ 扫码登录成功即启动主动聊天循环（不再需要重启软件才生效）
            ensure_proactive_loop(&inner, app.clone());
            let _ = app.emit(
                "wechat-bot-status",
                serde_json::json!({ "type": "connected", "botId": bot_id, "userId": user_id, "slot": inner.slot }),
            );
            Ok(serde_json::json!({ "status": "confirmed", "botId": bot_id, "userId": user_id, "slot": inner.slot }))
        }
        "scaned_but_redirect" => {
            if let Some(host) = resp["redirect_host"].as_str() {
                let new_base = format!("https://{host}");
                if let Some(qr) = inner.qr_session.lock().as_mut() {
                    qr.polling_base = new_base;
                }
            }
            Ok(serde_json::json!({ "status": "scaned_but_redirect" }))
        }
        "need_verifycode" => Ok(serde_json::json!({ "status": "need_verifycode" })),
        "verify_code_blocked" => {
            if let Some(qr) = inner.qr_session.lock().as_mut() {
                qr.pending_verify_code = None;
            }
            Ok(serde_json::json!({ "status": "verify_code_blocked" }))
        }
        _ => Ok(serde_json::json!({ "status": status })),
    }
}

/// 提交手机微信显示的配对码（need_verifycode 状态时，指定槽位）
#[tauri::command]
pub async fn wechat_verify_code(
    state: tauri::State<'_, WechatBotState>,
    code: String,
    slot: Option<usize>,
) -> AppResult<serde_json::Value> {
    let inner = state.bot(slot.unwrap_or(0));
    let mut guard = inner.qr_session.lock();
    let session = guard
        .as_mut()
        .ok_or_else(|| "请先生成登录二维码".to_string())?;
    session.pending_verify_code = Some(code.trim().to_string());
    drop(guard);
    Ok(serde_json::json!({ "ok": true }))
}

/// 刷新二维码（过期 / 配对码多次错误后，指定槽位）
#[tauri::command]
pub async fn wechat_refresh_qr(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
) -> AppResult<serde_json::Value> {
    let inner = state.bot(slot.unwrap_or(0));
    let client = http_client();
    let qr = fetch_qr(&client).await?;
    let qrcode = qr.qrcode.clone();
    let qrcode_url = qr.qrcode_url.clone();
    *inner.qr_session.lock() = Some(qr);
    Ok(serde_json::json!({ "qrcode": qrcode, "qrcodeUrl": qrcode_url, "slot": inner.slot }))
}

/// 登出微信（清除本地 token，指定槽位）
#[tauri::command]
pub async fn wechat_logout(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    wechat_bot_stop_inner(&inner);
    *inner.token.lock() = None;
    *inner.bot_id.lock() = None;
    *inner.base_url.lock() = None;
    *inner.user_id.lock() = None;
    *inner.get_updates_buf.lock() = String::new();
    inner.context_map.lock().clear();
    inner.typing_ticket.lock().take();
    inner.typing_tickets.lock().clear();
    // ★ 换账号防撞号误判：清空消息去重环 + 主动聊天状态（新账号的 msg_id/时间戳会重复）
    inner.last_msg_ids.lock().clear();
    *inner.proactive_last_at.lock() = 0;
    *inner.proactive_last_msg.lock() = None;
    *inner.last_user_msg_at.lock() = 0;
    *inner.proactive_idle_rounds.lock() = 0;
    delete_account(&inner).await;
    Ok(())
}

/// 启动微信 Bot（指定槽位：自动加载已保存账号并续连）
#[tauri::command]
pub async fn wechat_bot_start(
    app: AppHandle,
    state: tauri::State<'_, WechatBotState>,
    _config: serde_json::Value,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    // 首次运行时加载已保存账号
    if inner.token.lock().is_none() {
        load_account(&inner).await;
    }
    let has_token = inner
        .token
        .lock()
        .as_ref()
        .map(|t| !t.is_empty())
        .unwrap_or(false);
    if has_token {
        if inner.base_url.lock().is_none() {
            *inner.base_url.lock() = Some(ILINK_BASE_URL.to_string());
        }
        inner.connected.store(true, Ordering::SeqCst);
        // 关键：notifyStart 激活会话（否则 sendmessage 返回 -14 session timeout）
        notify_start(&inner).await;
        refresh_typing_ticket(&inner).await;
        start_getupdates_loop(&inner, app.clone()).await;
        // ★ wechat_bot_start 也启动主动聊天循环（不再需要重启软件才生效）
        ensure_proactive_loop(&inner, app.clone());
        let _ = app.emit(
            "wechat-bot-status",
            serde_json::json!({ "type": "connected", "resumed": true, "slot": inner.slot }),
        );
        return Ok(());
    }
    Err("微信未登录，请先在微信面板扫码登录".to_string())
}

/// 停止微信 Bot（指定槽位）
#[tauri::command]
pub fn wechat_bot_stop(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    wechat_bot_stop_inner(&inner);
    Ok(())
}

fn wechat_bot_stop_inner(inner: &Arc<WechatInner>) {
    if let Some(tx) = inner.shutdown.lock().take() {
        let _ = tx.send(());
    }
    // ★ 停止主动聊天循环：置停止信号（proactive_loop 每轮检查），并中止任务
    inner.proactive_stop.store(true, Ordering::SeqCst);
    if let Some(h) = inner.proactive_task.lock().take() {
        h.abort();
    }
    // 清理 typing 保活任务（避免任务悬空继续发送"正在输入"）
    *inner.typing_target.lock() = None;
    if let Some(h) = inner.typing_task.lock().take() {
        h.abort();
    }
    inner.running.store(false, Ordering::SeqCst);
    inner.connected.store(false, Ordering::SeqCst);
}

/// 控制"正在输入"状态（AI 生成期间前端调用）。
/// active=true：启动保活任务（每 10s 发送一次 typing，微信端持续显示"对方正在输入"）；
/// active=false：取消保活任务并发送结束状态。
#[tauri::command]
pub async fn wechat_typing(
    state: tauri::State<'_, WechatBotState>,
    to_user: String,
    active: bool,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    if active {
        *inner.typing_target.lock() = Some(to_user.clone());
        let mut guard = inner.typing_task.lock();
        if guard.is_none() {
            let inner2 = inner.clone();
            let handle: tauri::async_runtime::JoinHandle<()> =
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        let target = inner2.typing_target.lock().clone();
                        if let Some(t) = target {
                            send_typing(&inner2, &t, true).await;
                        }
                    }
                });
            *guard = Some(handle);
        }
    } else {
        *inner.typing_target.lock() = None;
        if let Some(h) = inner.typing_task.lock().take() {
            h.abort();
        }
        send_typing(&inner, &to_user, false).await;
    }
    Ok(())
}

/// 通过 Bot 回复微信用户消息（AI 回复，指定槽位）
#[tauri::command]
pub async fn wechat_bot_reply(
    state: tauri::State<'_, WechatBotState>,
    msg_id: String,
    to_user: String,
    content: String,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    // ★ 去重检查：同一 msg_id 已处理过（已回复）→ 直接返回，防前端重复调用导致重复回复。
    //   发送成功后才把 msg_id 记入去重环（发送失败可重试，不占去重位）
    if !msg_id.is_empty() && inner.last_msg_ids.lock().contains(&msg_id) {
        crate::llm::logging::debug("wechat", &format!("slot{} 跳过重复回复 msg_id={}", inner.slot, msg_id));
        return Ok(());
    }
    // ★ 判断用户是否明确要求字数/长文：查该用户最近一条消息内容。
    //   用户要求了 → 不截断；普通聊天 → 强制真人短句风格。
    let user_asked_long = {
        let recs = read_history_limit(&inner, 200);
        recs.iter()
            .rev()
            .find(|r| {
                r.get("toUser").and_then(|v| v.as_str()) == Some(to_user.as_str())
                    && !r.get("fromBot").and_then(|v| v.as_bool()).unwrap_or(true)
            })
            .and_then(|r| r.get("content").and_then(|c| c.as_str()))
            .map(|c| {
                // 用户消息里出现"写X字 / 多少字 / 长文 / 详细 / 完整 / 说明 / 分析 / 总结"等
                // 长文意图词 → 视为要求详细回复
                c.contains("字") || c.contains("长文") || c.contains("详细") || c.contains("完整")
                    || c.contains("分析") || c.contains("总结") || c.contains("说明")
                    || c.contains("介绍") || c.contains("怎么写") || c.contains("给我写")
                    || c.contains("帮我写")
            })
            .unwrap_or(false)
    };
    // ★ 去 AI 味：普通聊天回复强制短句（prompt 已约束，这里兜底截断防 AI 不听话）。
    //   超过 200 字的长文自动折叠成"结论 + 详情已整理"，保持微信真人聊天节奏；
    //   用户明确要求字数/长文时放行不截断。
    let content = {
        let chars: Vec<char> = content.trim().chars().collect();
        if !user_asked_long && chars.len() > 200 {
            let head: String = chars.iter().take(120).collect();
            format!("{}……（内容较长，已精简，需要完整版跟我说一声）", head.trim_end_matches(|c: char| c == '，' || c == ',' || c == '。' || c == '.'))
        } else {
            content.trim().to_string()
        }
    };
    if content.is_empty() {
        return Ok(());
    }
    // 从 context_map 取该用户的 context_token
    let context_token = inner.context_map.lock().get(&to_user).cloned();
    // 发送"正在输入"提示（发送完成后立即结束输入态）
    send_typing(&inner, &to_user, true).await;
    send_message(&inner, &to_user, &content, context_token.as_deref()).await?;
    send_typing(&inner, &to_user, false).await;
    // ★ 聊天记录：AI 回复也写入 D 盘 history.jsonl（fromBot=true：AI 发送）
    append_history(&inner, &to_user, &to_user, &content, "text", true, false);
    // ★ 发送成功 → 标记 msg_id 已处理（去重环，容量 100）
    if !msg_id.is_empty() {
        let mut seen = inner.last_msg_ids.lock();
        seen.push_back(msg_id.clone());
        while seen.len() > 100 {
            seen.pop_front();
        }
    }
    Ok(())
}

/// 发送消息到微信用户（指定槽位）
#[tauri::command]
pub async fn wechat_send_message(
    state: tauri::State<'_, WechatBotState>,
    to_user: String,
    content: String,
    context_token: Option<String>,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    let ctx = context_token.or_else(|| inner.context_map.lock().get(&to_user).cloned());
    send_message(&inner, &to_user, &content, ctx.as_deref()).await?;
    append_history(&inner, &to_user, &to_user, &content, "text", true, false);
    Ok(())
}

/// 发送本地图片到微信用户（指定槽位）
#[tauri::command]
pub async fn wechat_send_image(
    state: tauri::State<'_, WechatBotState>,
    to_user: String,
    image_path: String,
    context_token: Option<String>,
    slot: Option<usize>,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    let ctx = context_token.or_else(|| inner.context_map.lock().get(&to_user).cloned());
    send_image(&inner, &to_user, &image_path, ctx.as_deref()).await?;
    append_history(&inner, &to_user, &to_user, &image_path, "image", true, false);
    Ok(())
}

/// 获取全部微信槽位状态（多账号列表：每个槽位的登录/连接/消息数/人设）
#[tauri::command]
pub fn wechat_bot_status(state: tauri::State<'_, WechatBotState>) -> AppResult<serde_json::Value> {
    let bots = state.bots();
    let list: Vec<serde_json::Value> = bots
        .iter()
        .map(|inner| {
            let bot_id = inner.bot_id.lock().clone().unwrap_or_default();
            let logged = inner.token.lock().as_ref().map(|t| !t.is_empty()).unwrap_or(false);
            let persona_len = inner
                .persona
                .lock()
                .as_ref()
                .map(|p| p.len())
                .unwrap_or(0);
            let persona_text = inner.persona.lock().clone().unwrap_or_default();
            // ★ 5 秒轮询接口：只解析最近 200 条，避免 history.jsonl 无界增长拖垮轮询
            let history_count = read_history_limit(inner, 200).len();
            serde_json::json!({
                "slot": inner.slot,
                "name": format!("微信{}", inner.slot + 1),
                "running": inner.running.load(Ordering::SeqCst),
                "connected": inner.connected.load(Ordering::SeqCst),
                "botName": if bot_id.is_empty() { format!("微信{}", inner.slot + 1) } else { bot_id.clone() },
                "lastPoll": *inner.last_poll.lock(),
                "messageCount": *inner.msg_count.lock(),
                "loggedIn": logged,
                "botId": bot_id,
                "personaLen": persona_len,
                "personaText": persona_text,
                "historyCount": history_count,
                "proactiveEnabled": inner.proactive_enabled.load(Ordering::SeqCst),
                "proactiveIntervalMin": *inner.proactive_interval_min.lock(),
                "proactiveIntervalMax": *inner.proactive_interval_max.lock(),
                "proactiveLastAt": *inner.proactive_last_at.lock(),
                "proactiveTarget": inner.proactive_target.lock().clone().unwrap_or_default(),
                // ★ 使用规则（白名单 / 音色）：前端面板展示与回填
                "allowedUsers": inner.allowed_users.lock().clone(),
                "voiceId": inner.voice_id.lock().clone().unwrap_or_default(),
                "voiceEngine": inner.voice_engine.lock().clone(),
                "cosyvoiceApiKey": inner.cosyvoice_api_key.lock().clone().unwrap_or_default(),
                "indexttsUrl": inner.indextts_url.lock().clone().unwrap_or_default(),
                "indexttsVoicePath": inner.indextts_voice_path.lock().clone().unwrap_or_default(),
                // ★ 能力声明（对齐 AstrBot PlatformMetadata 思路）：上层据此决定 UI 与行为。
                //   协议能力天花板（官方 iLink 单聊）：文本/图片收发、语音接收（云端转写）、
                //   引用解析、typing、主动推送；不支持群聊/语音发送/视频理解。
                "capabilities": {
                    "sendText": true,
                    "sendImage": true,
                    "receiveVoiceTranscript": true,
                    "receiveImages": true,
                    "receiveFiles": true,
                    "replyQuote": true,
                    "typing": true,
                    "proactive": true,
                    "groupChat": false,
                    "sendVoice": false,
                },
            })
        })
        .collect();
    Ok(serde_json::json!({ "bots": list, "total": list.len() }))
}

/// 获取 AI 当前生活状态描述（世界线：此刻在做什么，时间与真实时钟同步）。
/// 供前端面板展示。
#[tauri::command]
pub fn wechat_living_state() -> String {
    crate::living_state::current_state_desc()
}

/// 获取 AI 完整生活上下文（当前状态 + 今日轨迹 + 近期记忆 + 一生记忆）。
/// 供自动回复 prompt 注入：AI 记得自己今天/昨天/前天做了什么，活了多久。
#[tauri::command]
pub fn wechat_living_context() -> String {
    crate::living_state::living_context_for_prompt()
}

/// 获取 AI 当前心情（情绪引擎快照，供前端面板展示心情）。
#[tauri::command]
pub fn wechat_mood_state() -> serde_json::Value {
    let m = crate::mood::mood_snapshot();
    serde_json::json!({
        "label": crate::mood::mood_label(&m),
        "joy": m.joy,
        "longing": m.longing,
        "loneliness": m.loneliness,
        "attachment": m.attachment,
        "arousal": m.arousal,
        "bornMs": m.born_ms,
        "history": crate::mood::mood_history(48),
    })
}

/// 获取 AI 完整"灵魂上下文"（心情 + 细节记忆），供自动回复 prompt 注入。
/// 与主动聊天共享同一份情绪/记忆（同一个"你"）。
#[tauri::command]
pub fn wechat_soul_context() -> String {
    let mood_raw = crate::mood::mood_context_for_prompt();
    let details_raw = crate::detail_memory::details_context_for_prompt(5, 400);
    let mood_note = format!(
        "【你的心情（这是你真实的心情状态，说话时让它自然地流露，不要刻意表演，也不要每一句都提到它）】\n{}",
        mood_raw
    );
    if details_raw.is_empty() {
        mood_note
    } else {
        format!("{}\n\n{}", mood_note, details_raw)
    }
}

/// 手动添加一条细节记忆（主人/前端/AI 都能用）。
#[tauri::command]
pub fn wechat_detail_add(text: String, tags: Option<String>) -> Result<usize, String> {
    crate::detail_memory::add_detail(&text, tags.as_deref().unwrap_or(""))
}

/// 查看全部细节记忆（前端管理/展示）。
#[tauri::command]
pub fn wechat_detail_list() -> Vec<serde_json::Value> {
    crate::detail_memory::all_details()
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "ts": d.ts_ms,
                "text": d.text,
                "source": d.source,
                "tags": d.tags,
                "used": d.used,
            })
        })
        .collect()
}

/// 删除一条细节记忆（主人不想要 AI 记住的事）。
#[tauri::command]
pub fn wechat_detail_forget(text: String) -> bool {
    crate::detail_memory::forget(&text)
}

/// 记录一条心情历史（前端可定时调用，画心情曲线）。
#[tauri::command]
pub fn wechat_mood_record() -> bool {
    crate::mood::record_history();
    true
}

/// 设置指定槽位微信的人设（system prompt，保存到 D 盘 persona.md）
#[tauri::command]
pub fn wechat_set_persona(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
    persona: String,
) -> AppResult<()> {
    let inner = state.bot(slot.unwrap_or(0));
    let text = persona.trim().to_string();
    if text.is_empty() {
        *inner.persona.lock() = None;
    } else {
        *inner.persona.lock() = Some(text.clone());
    }
    if let Some(d) = slot_dir(&inner) {
        let pf = d.join("persona.md");
        // ★ 清除只读属性后写盘；失败必须报错（不再静默），否则重启后读旧人设
        if let Err(e) = write_text_atomic(&pf, &text) {
            eprintln!("[wechat] slot{} 人设写盘失败: {e}", inner.slot);
            return Err(e);
        }
    } else {
        return Err(format!("slot{} 数据目录未初始化，无法保存人设", inner.slot));
    }
    Ok(())
}

/// 读取指定槽位微信的聊天记录（D 盘 history.jsonl，供前端展示/导出）
#[tauri::command]
pub fn wechat_history(
    state: tauri::State<'_, WechatBotState>,
    slot: Option<usize>,
) -> AppResult<serde_json::Value> {
    let inner = state.bot(slot.unwrap_or(0));
    let recs = read_history(&inner);
    Ok(serde_json::json!({ "slot": inner.slot, "count": recs.len(), "records": recs }))
}

/// 生成真实二维码（SVG 字符串），内容为可扫描的 URL。
/// 说明：腾讯 iLink 返回的 `qrcode_img_content` 是链接文本而非图片，
/// 前端需用本命令把它渲染成二维码 SVG（与旧版 clawdesk 一致）。
#[tauri::command]
pub fn mobile_qr_svg(text: String) -> AppResult<String> {
    let code = qrcode::QrCode::new(text.as_bytes())
        .map_err(|e| format!("二维码生成失败: {e}"))?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Ok(svg)
}


#[cfg(test)]
mod dpapi_tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let secret = b"sk-test-1234567890abcdef-token-xyz";
        let enc = dpapi_encrypt(secret).expect("encrypt 失败");
        // 密文不应包含明文
        assert!(enc != secret, "密文不应等于明文");
        // 明文不应出现在密文中
        let plain_str = String::from_utf8_lossy(secret);
        let enc_str = String::from_utf8_lossy(&enc);
        assert!(!enc_str.contains(plain_str.trim()), "明文泄露到密文");
        // 解密还原
        let dec = dpapi_decrypt(&enc).expect("decrypt 失败");
        assert_eq!(dec, secret, "解密结果不一致");
    }
}
