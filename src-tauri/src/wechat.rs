//! 微信 ClawBot 接入模块 — 腾讯 iLink Bot API（官方扫码登录，纯 HTTP，无外部服务）
//!
//! 架构：微信用户 ↔ 腾讯 iLink Bot（ilinkai.weixin.qq.com）↔ ClawDesk 桌面端
//! 协议参考：@tencent-weixin/openclaw-weixin@2.4.6（MIT 开源，腾讯官方出品）
//!
//! 登录：get_bot_qrcode 获取二维码 -> 手机微信扫码 -> get_qrcode_status 长轮询确认
//! 消息：getupdates 长轮询接收用户消息 -> sendmessage 发送 AI 回复
//! 持久化：bot_token 保存到 app_data_dir/wechat_ilink.json，重启后自动续连

use crate::error::{AppError, AppResult};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

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

// ─── 数据结构 ───

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WechatMessage {
    pub msg_id: String,
    pub from_user: String,
    pub content: String,
    pub msg_type: String,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
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

/// Bot 内部状态（跨线程共享）
pub(crate) struct WechatInner {
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
    pub shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

pub struct WechatBotState(pub Arc<WechatInner>);

impl Default for WechatBotState {
    fn default() -> Self {
        Self(Arc::new(WechatInner {
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
            shutdown: Mutex::new(None),
        }))
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
        .build()
        .expect("HTTP client")
}

fn http_client_long() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(40))
        .build()
        .expect("HTTP client")
}

// ─── 持久化 ───

fn account_file(inner: &Arc<WechatInner>) -> Option<PathBuf> {
    let dir = inner.data_dir.lock().clone()?;
    Some(dir.join("wechat_ilink.json"))
}

async fn save_account(inner: &Arc<WechatInner>) {
    let Some(path) = account_file(inner) else { return };
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
    if let Ok(json) = serde_json::to_string_pretty(&acc) {
        let _ = tokio::fs::write(path, json).await;
    }
}

async fn load_account(inner: &Arc<WechatInner>) {
    let Some(path) = account_file(inner) else { return };
    if let Ok(text) = tokio::fs::read_to_string(&path).await {
        if let Ok(acc) = serde_json::from_str::<AccountFile>(&text) {
            if !acc.token.is_empty() {
                *inner.token.lock() = Some(acc.token);
                *inner.bot_id.lock() = Some(acc.bot_id);
                *inner.base_url.lock() = Some(acc.base_url);
                *inner.user_id.lock() = Some(acc.user_id);
            }
        }
    }
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
        .map_err(|e| AppError::Other(format!("获取微信二维码失败: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Other(format!(
            "获取微信二维码失败 HTTP {status}: {}",
            &text[..text.len().min(200)]
        )));
    }
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AppError::Other(format!("解析二维码响应失败: {e}")))?;
    let qrcode = v["qrcode"].as_str().unwrap_or_default().to_string();
    let qrcode_url = v["qrcode_img_content"].as_str().unwrap_or_default().to_string();
    if qrcode.is_empty() || qrcode_url.is_empty() {
        return Err(AppError::Other(format!(
            "二维码响应异常: {}",
            &text[..text.len().min(200)]
        )));
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
                &text[..text.len().min(200)]
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
        &text_resp[..text_resp.len().min(300)]
    );
    if !status.is_success() {
        return Err((
            format!(
                "发送微信消息失败 HTTP {status}: {}",
                &text_resp[..text_resp.len().min(200)]
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
async fn send_message(
    inner: &Arc<WechatInner>,
    to: &str,
    text: &str,
    context_token: Option<&str>,
) -> AppResult<()> {
    let token = inner
        .token
        .lock()
        .clone()
        .ok_or_else(|| AppError::Other("微信未登录，请先扫码登录".into()))?;
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
                    .map_err(|(m, _)| AppError::Other(m))
            } else {
                Err(AppError::Other(msg))
            }
        }
        Err((msg, _)) => Err(AppError::Other(msg)),
    }
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
        loop {
            if rx.try_recv().is_ok() {
                break;
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
                    Err(_) => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                },
                // 网络错误（超时是正常的长轮询退出）
                Err(_) => {
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
                    inner2.connected.store(false, Ordering::SeqCst);
                    let _ = app2.emit(
                        "wechat-bot-status",
                        serde_json::json!({ "type": "session_expired" }),
                    );
                    break;
                }
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

                // 提取文本内容
                let mut text = String::new();
                if let Some(items) = m["item_list"].as_array() {
                    for item in items {
                        if item["type"].as_i64() == Some(ITEM_TYPE_TEXT) {
                            if let Some(t) = item["text_item"]["text"].as_str() {
                                text = t.to_string();
                            }
                        }
                    }
                }
                eprintln!(
                    "[wechat] received msg from={} ctx={} text={}",
                    from,
                    if context_token.is_empty() { "NONE" } else { "YES" },
                    &text[..text.len().min(50)]
                );
                if text.trim().is_empty() {
                    continue;
                }

                // 缓存 context_token 用于回复
                if !context_token.is_empty() {
                    inner2
                        .context_map
                        .lock()
                        .insert(from.clone(), context_token.clone());
                }

                let msg_id = m["seq"]
                    .as_i64()
                    .map(|s| s.to_string())
                    .or_else(|| m["message_id"].as_i64().map(|s| s.to_string()))
                    .unwrap_or_else(uuid);
                let msg = WechatMessage {
                    msg_id,
                    from_user: from.clone(),
                    content: text,
                    msg_type: "text".into(),
                    timestamp: now_millis(),
                    context_token: if context_token.is_empty() {
                        None
                    } else {
                        Some(context_token)
                    },
                };
                let _ = app2.emit("wechat-message", &msg);
                *inner2.msg_count.lock() += 1;
            }
        }
        // 循环退出清理
        inner2.running.store(false, Ordering::SeqCst);
        *inner2.shutdown.lock() = None;
    });
}

// ─── Tauri 命令 ───

/// 初始化数据目录（应用 setup 时调用）
pub fn init_data_dir(app: &tauri::AppHandle, state: &WechatBotState) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        *state.0.data_dir.lock() = Some(dir);
    }
}

/// 获取登录二维码
#[tauri::command]
pub async fn wechat_get_qr(state: tauri::State<'_, WechatBotState>) -> AppResult<serde_json::Value> {
    let inner = state.0.clone();
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
            }));
        }
    }
    let client = http_client();
    let qr = fetch_qr(&client).await?;
    let qrcode = qr.qrcode.clone();
    let qrcode_url = qr.qrcode_url.clone();
    *inner.qr_session.lock() = Some(qr);
    Ok(serde_json::json!({ "qrcode": qrcode, "qrcodeUrl": qrcode_url }))
}

/// 长轮询扫码状态（单次，前端循环调用）
#[tauri::command]
pub async fn wechat_qr_status(
    state: tauri::State<'_, WechatBotState>,
    app: AppHandle,
) -> AppResult<serde_json::Value> {
    let inner = state.0.clone();
    let session = inner
        .qr_session
        .lock()
        .clone()
        .ok_or_else(|| AppError::Other("请先生成登录二维码".into()))?;
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
                return Err(AppError::Other("登录确认但缺少 bot_token/ilink_bot_id".into()));
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
                serde_json::json!({ "type": "connected", "botId": bot_id, "userId": user_id }),
            );
            Ok(serde_json::json!({ "status": "confirmed", "botId": bot_id, "userId": user_id }))
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

/// 提交手机微信显示的配对码（need_verifycode 状态时）
#[tauri::command]
pub async fn wechat_verify_code(
    state: tauri::State<'_, WechatBotState>,
    code: String,
) -> AppResult<serde_json::Value> {
    let inner = state.0.clone();
    let mut guard = inner.qr_session.lock();
    let session = guard
        .as_mut()
        .ok_or_else(|| AppError::Other("请先生成登录二维码".into()))?;
    session.pending_verify_code = Some(code.trim().to_string());
    drop(guard);
    Ok(serde_json::json!({ "ok": true }))
}

/// 刷新二维码（过期 / 配对码多次错误后）
#[tauri::command]
pub async fn wechat_refresh_qr(state: tauri::State<'_, WechatBotState>) -> AppResult<serde_json::Value> {
    let inner = state.0.clone();
    let client = http_client();
    let qr = fetch_qr(&client).await?;
    let qrcode = qr.qrcode.clone();
    let qrcode_url = qr.qrcode_url.clone();
    *inner.qr_session.lock() = Some(qr);
    Ok(serde_json::json!({ "qrcode": qrcode, "qrcodeUrl": qrcode_url }))
}

/// 登出微信（清除本地 token）
#[tauri::command]
pub async fn wechat_logout(state: tauri::State<'_, WechatBotState>) -> AppResult<()> {
    let inner = state.0.clone();
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

/// 启动微信 Bot（兼容旧接口：自动加载已保存账号并续连）
#[tauri::command]
pub async fn wechat_bot_start(
    app: AppHandle,
    state: tauri::State<'_, WechatBotState>,
    _config: serde_json::Value,
) -> AppResult<()> {
    let inner = state.0.clone();
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
            serde_json::json!({ "type": "connected", "resumed": true }),
        );
        return Ok(());
    }
    Err(AppError::Other("微信未登录，请先在微信面板扫码登录".into()))
}

/// 停止微信 Bot
#[tauri::command]
pub fn wechat_bot_stop(state: tauri::State<'_, WechatBotState>) -> AppResult<()> {
    wechat_bot_stop_inner(&state.0);
    Ok(())
}

fn wechat_bot_stop_inner(inner: &Arc<WechatInner>) {
    if let Some(tx) = inner.shutdown.lock().take() {
        let _ = tx.send(());
    }
    inner.running.store(false, Ordering::SeqCst);
    inner.connected.store(false, Ordering::SeqCst);
}

/// 通过 Bot 回复微信用户消息（AI 回复）
#[tauri::command]
pub async fn wechat_bot_reply(
    state: tauri::State<'_, WechatBotState>,
    _msg_id: String,
    to_user: String,
    content: String,
) -> AppResult<()> {
    let inner = state.0.clone();
    // 从 context_map 取该用户的 context_token
    let context_token = inner.context_map.lock().get(&to_user).cloned();
    // 发送"正在输入"提示
    send_typing(&inner, &to_user).await;
    send_message(&inner, &to_user, &content, context_token.as_deref()).await?;
    Ok(())
}

/// 发送消息到微信用户（AI 回复的另一种入口，支持指定 context）
#[tauri::command]
pub async fn wechat_send_message(
    state: tauri::State<'_, WechatBotState>,
    to_user: String,
    content: String,
    context_token: Option<String>,
) -> AppResult<()> {
    let inner = state.0.clone();
    let ctx = context_token.or_else(|| inner.context_map.lock().get(&to_user).cloned());
    send_message(&inner, &to_user, &content, ctx.as_deref()).await
}

/// 获取 Bot 运行状态
#[tauri::command]
pub fn wechat_bot_status(state: tauri::State<'_, WechatBotState>) -> AppResult<serde_json::Value> {
    let inner = &state.0;
    let bot_id = inner.bot_id.lock().clone().unwrap_or_default();
    Ok(serde_json::json!({
        "running": inner.running.load(Ordering::SeqCst),
        "connected": inner.connected.load(Ordering::SeqCst),
        "botName": if bot_id.is_empty() { "ClawBot".to_string() } else { bot_id.clone() },
        "lastPoll": *inner.last_poll.lock(),
        "messageCount": *inner.msg_count.lock(),
        "loggedIn": inner.token.lock().as_ref().map(|t| !t.is_empty()).unwrap_or(false),
        "botId": bot_id,
    }))
}
