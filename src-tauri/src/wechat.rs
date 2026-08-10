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
use md5::{Digest, Md5};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// 持久化的账号文件
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountFile {
    token: String,
    bot_id: String,
    base_url: String,
    user_id: String,
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
    pub qr_session: Mutex<Option<QrSession>>,
    pub data_dir: Mutex<Option<PathBuf>>,
    /// 该微信的人设（system prompt 文本，可随时修改，AI 回复时注入）
    pub persona: Mutex<Option<String>>,
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
    pub shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
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
            qr_session: Mutex::new(None),
            data_dir: Mutex::new(None),
            persona: Mutex::new(None),
            history_path: Mutex::new(None),
            proactive_enabled: AtomicBool::new(false),
            proactive_interval_min: Mutex::new(1),
            proactive_interval_max: Mutex::new(180),
            proactive_last_at: Mutex::new(0),
            proactive_target: Mutex::new(None),
            proactive_last_msg: Mutex::new(None),
            proactive_idle_rounds: Mutex::new(0),
            shutdown: Mutex::new(None),
        }
    }
}

/// 最大微信槽位数（可同时接入 10 个微信）
pub const MAX_BOTS: usize = 10;

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
fn trunc_chars(s: &str, max_chars: usize) -> &str {
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

/// 处理单个媒体 item：下载 + 解密 + 保存到附件目录，返回 (本地路径, 种类)
async fn process_media_item(
    client: &reqwest::Client,
    item: &serde_json::Value,
    dir: &std::path::Path,
) -> Option<(String, WechatMediaKind)> {
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
    std::fs::write(&path, &bytes).ok()?;

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
        } else if let Ok(extract_dir) = extract_zip_archive(&path, dir) {
            eprintln!("[wechat] 压缩包 {} 已解压到: {}", fname, extract_dir.display());
            return Some((extract_dir.to_string_lossy().to_string(), WechatMediaKind::File));
        }
    }

    Some((path.to_string_lossy().to_string(), kind))
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
    let recs = read_history(inner);
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

/// 持久化主动聊天设置（应用退出 / 重启后恢复上次配置，不丢用户设置）
fn save_proactive(inner: &Arc<WechatInner>) {
    let Some(path) = proactive_file(inner) else { return };
    let data = serde_json::json!({
        "enabled": inner.proactive_enabled.load(Ordering::SeqCst),
        "intervalMin": *inner.proactive_interval_min.lock(),
        "intervalMax": *inner.proactive_interval_max.lock(),
        "lastAt": *inner.proactive_last_at.lock(),
        "target": inner.proactive_target.lock().clone().unwrap_or_default(),
        "lastMsg": inner.proactive_last_msg.lock().clone().unwrap_or_default(),
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
    // ★ 磁盘目标为空（未手动指定过）→ 从聊天记录恢复最近聊过的人，
    //   否则「自动（最近聊过的人）」重启后 target 为 None，主动聊天永不触发
    if inner.proactive_target.lock().as_deref().map(|t| t.is_empty()).unwrap_or(true) {
        if let Some(peer) = last_history_peer(inner) {
            eprintln!("[wechat] slot{} 主动聊天目标从历史恢复: {}", inner.slot, peer);
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
pub(crate) fn append_history(
    inner: &Arc<WechatInner>,
    dir: &str,
    to_user: &str,
    content: &str,
    msg_type: &str,
    from_bot: bool,
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
    });
    let line = format!("{}\n", rec.to_string());
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 读取该微信槽位的人设（system prompt，None 表示未设置）
pub fn persona_of(inner: &Arc<WechatInner>) -> Option<String> {
    inner.persona.lock().clone()
}

/// 读取该微信全部聊天记录（供前端展示 / 导出）
pub(crate) fn read_history(inner: &Arc<WechatInner>) -> Vec<serde_json::Value> {
    let Some(path) = history_path_of(inner) else { return vec![] };
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .collect()
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
    let acc = AccountFile { token, bot_id, base_url, user_id };
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

/// 发送"正在输入"状态（可选体验优化）
async fn send_typing(inner: &Arc<WechatInner>, to: &str) {
    let Some(token) = inner.token.lock().clone() else { return };
    let Some(ticket) = inner.typing_ticket.lock().clone() else { return };
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
            "status": 1,
        }))
        .send()
        .await;
}

/// 登录成功后获取 typing_ticket 并缓存
async fn refresh_typing_ticket(inner: &Arc<WechatInner>) {
    let Some(token) = inner.token.lock().clone() else { return };
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());
    let client = http_client();
    if let Ok(resp) = client
        .post(format!("{base_url}/ilink/bot/getconfig"))
        .headers(build_headers(Some(&token)))
        .json(&serde_json::json!({}))
        .send()
        .await
    {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(t) = v["typing_ticket"].as_str() {
                if !t.is_empty() {
                    *inner.typing_ticket.lock() = Some(t.to_string());
                }
            }
        }
    }
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
            eprintln!(
                "[wechat] notifyStart status={status} body={}",
                trunc_chars(&text, 200)
            );
            if !status.is_success() {
                return false;
            }
            // 检查业务码
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                let ret = v["ret"].as_i64().unwrap_or(0);
                if ret != 0 {
                    eprintln!(
                        "[wechat] notifyStart ret={ret} errmsg={}",
                        v["errmsg"].as_str().unwrap_or("")
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
    eprintln!(
        "[wechat] sendmessage to={} ctx={}",
        to,
        if context_token.unwrap_or("").is_empty() { "NONE" } else { "YES" }
    );
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
    eprintln!(
        "[wechat] sendmessage status={status} body={}",
        trunc_chars(&text_resp, 300)
    );
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
/// pub(crate)：定时任务（scheduler.rs）推送结果时也会调用。
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
    let plaintext = std::fs::read(image_path).map_err(|e| format!("读取图片失败: {}", e))?;
    if plaintext.is_empty() {
        return Err("图片文件为空".into());
    }
    let rawsize = plaintext.len();
    let rawfilemd5 = md5_hex(&plaintext);
    let aeskey: Vec<u8> = (0..16).map(|_| (now_millis() as u8).wrapping_add(rand_byte())).collect();
    let ciphertext = aes_ecb_encrypt(&plaintext, &aeskey).ok_or("AES 加密失败")?;
    let filesize = ciphertext.len();
    let filekey = format!("{:x}", now_millis());

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

/// 伪随机单字节（无需强随机，用于 aeskey 混淆）
fn rand_byte() -> u8 {
    (now_millis() as u8).wrapping_mul(31).wrapping_add(7)
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
    eprintln!("[wechat] sendimage to={} filekey={} ctx={}", to, filekey,
        if context_token.unwrap_or("").is_empty() { "NONE" } else { "YES" });
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

// ─── getUpdates 长轮询后台循环 ───

async fn start_getupdates_loop(inner: &Arc<WechatInner>, app: AppHandle) {
    // 已有循环则不重复启动
    if inner.shutdown.lock().is_some() {
        return;
    }
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    *inner.shutdown.lock() = Some(tx);

    let token = inner.token.lock().clone().unwrap_or_default();
    let base_url = inner
        .base_url
        .lock()
        .clone()
        .unwrap_or_else(|| ILINK_BASE_URL.to_string());
    let mut buf = inner.get_updates_buf.lock().clone();
    let inner2 = inner.clone();
    let app2 = app.clone();
    inner.running.store(true, Ordering::SeqCst);
    inner.connected.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        let client = http_client_long();
        // 外层：断连自动重连（除明确 stop 外永不退出 —— 修复「静默死循环」：
        // 旧版网络错误/异常退出后 getupdates 永久停止 → 腾讯服务器无法推送
        // → 手机微信显示"暂无法连接 OpenClaw"。现在自动 notify_start 重连。）
        'outer: loop {
            if rx.try_recv().is_ok() {
                break 'outer;
            }
            inner2.running.store(true, Ordering::SeqCst);
            inner2.connected.store(true, Ordering::SeqCst);
            // 内层：长轮询请求循环
            loop {
                if rx.try_recv().is_ok() {
                    break 'outer;
                }
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
                if let Some(b) = resp["get_updates_buf"].as_str() {
                    buf = b.to_string();
                    *inner2.get_updates_buf.lock() = buf.clone();
                }
                *inner2.last_poll.lock() = now_millis();

                // 业务错误
                let ret = resp["ret"].as_i64().unwrap_or(0);
                let errcode = resp["errcode"].as_i64().unwrap_or(0);
                if ret != 0 || errcode != 0 {
                    // ret/errcode = -14 表示 session 超时（token 失效），需重新扫码
                    if ret == -14 || errcode == -14 {
                        eprintln!(
                            "[wechat] slot{} session 超时(ret={ret} errcode={errcode})，自动重连恢复…",
                            inner2.slot
                        );
                        inner2.connected.store(false, Ordering::SeqCst);
                        let _ = app2.emit(
                            "wechat-bot-status",
                            serde_json::json!({ "type": "session_expired" }),
                        );
                        break; // 跳出内层 → 外层自动 notify_start 重连
                    }
                    eprintln!(
                        "[wechat] slot{} getupdates 业务错误 ret={ret} errcode={errcode}，1 秒后重试",
                        inner2.slot
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }

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
                let context_token = m["context_token"].as_str().unwrap_or_default().to_string();

                // 提取文本 + 媒体（图片/文件/语音/视频，下载解密到附件目录）
                let mut text = String::new();
                let mut images: Vec<String> = Vec::new();
                let mut attachments: Vec<String> = Vec::new();
                if let Some(items) = m["item_list"].as_array() {
                    for item in items {
                        match item["type"].as_i64() {
                            Some(1) => {
                                if let Some(t) = item["text_item"]["text"].as_str() {
                                    text = t.to_string();
                                }
                            }
                            Some(2) | Some(3) | Some(4) | Some(5) => {
                                if let Ok(dir) =
                                    crate::executors::builtin::attachment::attach_dir()
                                {
                                    let inbound = dir.join("inbound");
                                    let _ = std::fs::create_dir_all(&inbound);
                                    if let Some((path, kind)) =
                                        process_media_item(&client, item, &inbound).await
                                    {
                                        match kind {
                                            WechatMediaKind::Image => images.push(path),
                                            _ => attachments.push(path),
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                eprintln!(
                    "[wechat] received msg from={} ctx={} text={} media={}",
                    from,
                    if context_token.is_empty() { "NONE" } else { "YES" },
                    trunc_chars(&text, 50),
                    images.len() + attachments.len()
                );
                if text.trim().is_empty() && images.is_empty() && attachments.is_empty() {
                    continue;
                }

                // 缓存 context_token 用于回复
                if !context_token.is_empty() {
                    inner2
                        .context_map
                        .lock()
                        .insert(from.clone(), context_token.clone());
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

                let msg_id = m["seq"]
                    .as_i64()
                    .map(|s| s.to_string())
                    .or_else(|| m["message_id"].as_i64().map(|s| s.to_string()))
                    .unwrap_or_else(uuid);
                let msg_type_str = if !images.is_empty() {
                    "image".to_string()
                } else if !attachments.is_empty() {
                    "file".to_string()
                } else {
                    "text".to_string()
                };
                let msg = WechatMessage {
                    msg_id,
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
                };
                // ★ 聊天记录：收到的用户消息写入 D 盘 history.jsonl（dir=from_user 为对方）
                append_history(&inner2, &from, &from, &text, &msg_type_str, false);
                let _ = app2.emit("wechat-message", &msg);
                *inner2.msg_count.lock() += 1;
            }
        }
        // ── 内层循环退出（-14 session 超时 / 意外）→ 外层自动重连 ──
        if rx.try_recv().is_ok() {
            break 'outer;
        }
        eprintln!(
            "[wechat] slot{} getupdates 循环退出，3 秒后 notify_start 自动重连…",
            inner2.slot
        );
        tokio::time::sleep(Duration::from_secs(3)).await;
        notify_start(&inner2).await;
        // 回到外层顶部重新长轮询（connected 已置 true）
    }
    // 循环退出清理（仅 stop 时到达）
    inner2.running.store(false, Ordering::SeqCst);
    *inner2.shutdown.lock() = None;
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

/// 真随机：基于系统时间纳秒 + 原子计数混合的伪随机数（足够打散主动聊天时机，无需加密级随机）
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

/// [min, max] 闭区间内取随机整数
fn rand_between(min: u64, max: u64) -> u64 {
    if max <= min {
        return min;
    }
    min + pseudo_rand() % (max - min + 1)
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

/// 主动聊天：Bot 每隔「随机 min~max 分钟」主动找最近聊过的用户发一条消息。
/// ★ 真随机：每次发送后随机取一次下次间隔（min~max 分钟），避免固定间隔的机械感。
/// ★ 时间约束：只在 08:00 ~ 23:00 之间主动（晚上11点后不打扰）；用户主动发消息不在此限制内。
async fn proactive_loop(inner: Arc<WechatInner>, app: AppHandle) {
    // 初始等待：真随机 min~max 分钟（避免每次启动后固定同一时间触发）
    let mut wait_secs = {
        let min = *inner.proactive_interval_min.lock();
        let max = *inner.proactive_interval_max.lock();
        rand_between(min, max) * 60
    };
    loop {
        tokio::time::sleep(Duration::from_secs(wait_secs)).await;
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
            let min = *inner.proactive_interval_min.lock();
            let max = *inner.proactive_interval_max.lock();
            wait_secs = rand_between(min, max) * 60;
            continue;
        }
        let target = inner.proactive_target.lock().clone();
        let Some(target) = target else {
            // ★ 兜底：目标为空时从聊天记录恢复最近聊过的人（重启后无需等新消息）
            if last_history_peer(&inner).is_some() {
                continue; // 已恢复目标，下一轮循环立即使用
            }
            let min = *inner.proactive_interval_min.lock();
            let max = *inner.proactive_interval_max.lock();
            wait_secs = rand_between(min, max) * 60;
            continue;
        };
        if target.is_empty() {
            let min = *inner.proactive_interval_min.lock();
            let max = *inner.proactive_interval_max.lock();
            wait_secs = rand_between(min, max) * 60;
            continue;
        }
        // 时间约束：08:00~23:00 之外不主动找（等到次日白天再随机触发）
        use chrono::Timelike;
        let hour = chrono::Local::now().hour();
        if hour < 8 || hour >= 23 {
            eprintln!("[wechat] slot{} 当前 {} 点，深夜时段不主动聊天", inner.slot, hour);
            crate::llm::logging::debug(
                "wechat",
                &format!("slot{} 当前 {} 点，深夜时段不主动聊天", inner.slot, hour),
            );
            wait_secs = rand_between(20, 60) * 60; // 深夜隔 20~60 分钟再看一次
            continue;
        }
        // ★ 拟人静默（修复"说了等你/晚安还一直叭叭"）：上次消息是 AI 发的
        //   （fromBot=true）→ 用户还没回复 → 本轮保持安静，等用户先开口。
        {
            let recs = read_history(&inner);
            if let Some(last) = recs.last() {
                let from_bot = last.get("fromBot").and_then(|v| v.as_bool()).unwrap_or(false);
                if from_bot {
                    crate::llm::logging::debug(
                        "wechat",
                        &format!(
                            "slot{} 用户未回复上次消息，保持安静（等到用户先开口）",
                            inner.slot
                        ),
                    );
                    let min = *inner.proactive_interval_min.lock();
                    let max = *inner.proactive_interval_max.lock();
                    wait_secs = rand_between(min, max) * 60;
                    continue;
                }
            }
        }
        // ★ 存在感惩罚 + 空闲退避（MaiBot 移植）：最近 10 条消息中 AI 发言
        //   ≥6 条 → AI 太抢戏，本轮保持安静；连续空闲轮数越多，间隔指数翻倍
        //   （base×2ⁿ，封顶 24h），避免机器人式轰炸；用户回复后计数自动归零。
        {
            let recs = read_history(&inner);
            let recent: Vec<&serde_json::Value> = recs.iter().rev().take(10).collect();
            let bot_count = recent
                .iter()
                .filter(|r| r.get("fromBot").and_then(|v| v.as_bool()).unwrap_or(false))
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
                        eprintln!(
                            "[wechat] slot{} 引擎配置从设置兜底恢复: model={}",
                            inner.slot, c.model
                        );
                        c
                    }
                    None => {
                        eprintln!(
                            "[wechat] slot{} 引擎未配置（设置中也无 Key），跳过主动聊天",
                            inner.slot
                        );
                        let min = *inner.proactive_interval_min.lock();
                        let max = *inner.proactive_interval_max.lock();
                        wait_secs = rand_between(min, max) * 60;
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
                let min = *inner.proactive_interval_min.lock();
                let max = *inner.proactive_interval_max.lock();
                wait_secs = rand_between(min, max) * 60;
                continue;
            }
        };
        let params = crate::harness::engine::param::ModelParams {
            model: cfg.model.clone(),
            reasoning_effort: cfg.effort,
            ..Default::default()
        };
        let system = format!(
            "你是一个微信聊天机器人。{}\n{}\n现在请你决定要不要主动给用户发一条微信消息：\n- 语气自然亲切，像老朋友主动关心\n- 结合当前时间说话（如早上问早、晚上问候、深夜关怀）\n- 符合你的人设和语言风格\n- 内容 5~40 字，短句口语化，像真人发微信（绝对不要 AI 腔：不要\"首先/其次/总之\"，不要\"！\"堆砌，不要\"哦～\"\"呢～\"等做作语气，最多 1 个 emoji）\n- 只有值得发时才发（有话题、有理由、用户可能需要关心）；如果没什么可说的，就选择不发\n- 必须只输出 JSON：{{\"shouldSend\": true或false, \"content\": \"消息内容\"}}",
            if persona.is_empty() { "（未设置人设）".to_string() } else { persona },
            time_desc
        );
        let system = if context_note.is_empty() {
            system
        } else {
            format!(
                "{system}\n\n{context_note}\n\n【聊天纪律（必须严格遵守）】\n- 参考上面最近的对话：延续话题或开启自然的新话题，但绝不重复自己已经说过的话\n- 如果上次你已经道别（晚安/等你/先忙/回头聊/不打扰等），或对话氛围已收尾，这轮就 shouldSend=false\n- 时间线要连贯：注意「你现在」的时间和上次发消息的时间差，不要重复问候同一时段\n- 像真实的朋友一样自然，不要机械式问候\n- 犹豫不决时优先 shouldSend=false（宁可不发，不要打扰）"
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
                    let min = *inner.proactive_interval_min.lock();
                    let max = *inner.proactive_interval_max.lock();
                    wait_secs = rand_between(min, max) * 60;
                    continue;
                }
                let text = content;
                if text.is_empty() {
                    let min = *inner.proactive_interval_min.lock();
                    let max = *inner.proactive_interval_max.lock();
                    wait_secs = rand_between(min, max) * 60;
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
                        append_history(&inner, &target, &target, &text, "text", true);
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
                        // ★ 真随机：发送成功后重新随机取下次间隔（min~max 分钟）
                        let min = *inner.proactive_interval_min.lock();
                        let max = *inner.proactive_interval_max.lock();
                        wait_secs = rand_between(min, max) * 60;
                        eprintln!(
                            "[wechat] slot{} 主动聊天已发送 to={} text={} 下次等待 {} 分钟（随机 {}~{}）",
                            inner.slot,
                            target,
                            trunc_chars(&text, 60),
                            wait_secs / 60,
                            min,
                            max
                        );
                        let _ = app.emit(
                            "wechat-bot-status",
                            serde_json::json!({ "type": "proactive_sent", "slot": inner.slot }),
                        );
                    }
                    Err(e) => {
                        eprintln!("[wechat] slot{} 主动聊天发送失败: {e}", inner.slot);
                        let min = *inner.proactive_interval_min.lock();
                        let max = *inner.proactive_interval_max.lock();
                        wait_secs = rand_between(min, max) * 60;
                    }
                }
            }
            Err(e) => {
                eprintln!("[wechat] slot{} 主动聊天生成失败: {e}", inner.slot);
                let min = *inner.proactive_interval_min.lock();
                let max = *inner.proactive_interval_max.lock();
                wait_secs = rand_between(min, max) * 60;
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
        // 每个已登录微信都启动主动聊天循环（8:00~23:00 才会真正发送）
        {
            let inner_p = inner.clone();
            let app_p = app.clone();
            tauri::async_runtime::spawn(async move {
                proactive_loop(inner_p, app_p).await;
            });
        }
        eprintln!("[wechat] slot{} 自动续连已恢复（已保存的登录凭据）", inner.slot);
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
    inner.running.store(false, Ordering::SeqCst);
    inner.connected.store(false, Ordering::SeqCst);
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
    // ★ 判断用户是否明确要求字数/长文：查该用户最近一条消息内容。
    //   用户要求了 → 不截断；普通聊天 → 强制真人短句风格。
    let user_asked_long = {
        let recs = read_history(&inner);
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
    // 发送"正在输入"提示
    send_typing(&inner, &to_user).await;
    send_message(&inner, &to_user, &content, context_token.as_deref()).await?;
    // ★ 聊天记录：AI 回复也写入 D 盘 history.jsonl（fromBot=true：AI 发送）
    append_history(&inner, &to_user, &to_user, &content, "text", true);
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
    append_history(&inner, &to_user, &to_user, &content, "text", true);
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
    append_history(&inner, &to_user, &to_user, &image_path, "image", true);
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
            let history_count = read_history(inner).len();
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
            })
        })
        .collect();
    Ok(serde_json::json!({ "bots": list, "total": list.len() }))
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
