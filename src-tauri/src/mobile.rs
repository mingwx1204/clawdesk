//! 手机端桥接：本机局域网 HTTP 服务。
//! 微信/QQ 扫一扫真实 URL 二维码 -> 打开手机网页 -> 与桌面端双向同步对话。
//!
//! 说明：微信公众号/QQ 官方 Bot 接入需要企业主体与服务端凭证，
//! 本方案是无需任何外部服务的真实可用替代：同一局域网内扫码即聊。

use crate::error::{AppError, AppResult};
use axum::{
    extract::{Query, State},
    response::Html,
    routing::{get, post},
    Json, Router,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

const MAX_BUFFERED: usize = 300;

#[derive(Debug, Clone, Serialize)]
pub struct BridgeMsg {
    pub id: u64,
    pub role: String, // user / assistant
    pub content: String,
    pub ts: u64,
}

pub(crate) struct BridgeInner {
    msgs: Mutex<VecDeque<BridgeMsg>>,
    seq: AtomicU64,
    shutdown: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    port: AtomicU64,
    client_connected: std::sync::atomic::AtomicBool,
}

pub struct MobileBridgeState(pub Arc<BridgeInner>);

impl Default for MobileBridgeState {
    fn default() -> Self {
        Self(Arc::new(BridgeInner {
            msgs: Mutex::new(VecDeque::new()),
            seq: AtomicU64::new(0),
            shutdown: Mutex::new(None),
            port: AtomicU64::new(0),
            client_connected: std::sync::atomic::AtomicBool::new(false),
        }))
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn push_msg(inner: &BridgeInner, role: &str, content: &str) -> u64 {
    let id = inner.seq.fetch_add(1, Ordering::SeqCst) + 1;
    let mut msgs = inner.msgs.lock();
    msgs.push_back(BridgeMsg {
        id,
        role: role.to_string(),
        content: content.to_string(),
        ts: now_secs(),
    });
    while msgs.len() > MAX_BUFFERED {
        msgs.pop_front();
    }
    id
}

/// 获取本机局域网 IP（UDP 路由探测，不产生真实流量）
fn lan_ip() -> AppResult<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.connect("223.5.5.5:80")?;
    Ok(sock.local_addr()?.ip().to_string())
}

#[derive(Serialize)]
pub struct BridgeInfo {
    pub url: String,
    pub lan_ip: String,
    pub port: u16,
}

#[derive(Deserialize)]
struct SinceQuery {
    since: Option<u64>,
}

#[derive(Serialize)]
struct MessagesResp {
    msgs: Vec<BridgeMsg>,
    latest: u64,
}

#[derive(Deserialize)]
struct SendBody {
    content: String,
}

/// 合并的路由状态
#[derive(Clone)]
struct AppCtx {
    inner: Arc<BridgeInner>,
    app: AppHandle,
}

async fn index_page(State(ctx): State<AppCtx>) -> Html<&'static str> {
    if !ctx.inner.client_connected.swap(true, Ordering::SeqCst) {
        let _ = ctx.app.emit("mobile-client-connected", ());
    }
    Html(MOBILE_PAGE)
}

async fn list_messages(State(ctx): State<AppCtx>, Query(q): Query<SinceQuery>) -> Json<MessagesResp> {
    let since = q.since.unwrap_or(0);
    let msgs = ctx.inner.msgs.lock();
    let filtered: Vec<BridgeMsg> = msgs.iter().filter(|m| m.id > since).cloned().collect();
    let latest = msgs.back().map(|m| m.id).unwrap_or(0);
    Json(MessagesResp { msgs: filtered, latest })
}

async fn send_message(State(ctx): State<AppCtx>, Json(body): Json<SendBody>) -> Json<serde_json::Value> {
    let content = body.content.trim().to_string();
    if content.is_empty() {
        return Json(serde_json::json!({ "ok": false }));
    }
    push_msg(&ctx.inner, "user", &content);
    // 通知桌面端：手机发来了一条用户消息
    let _ = ctx.app.emit("mobile-user-msg", content);
    Json(serde_json::json!({ "ok": true }))
}

/// 启动桥接服务（幂等：已运行则直接返回地址）
#[tauri::command]
pub async fn mobile_bridge_start(
    app: AppHandle,
    state: tauri::State<'_, MobileBridgeState>,
    port: Option<u16>,
) -> AppResult<BridgeInfo> {
    let inner = state.0.clone();
    let port = port.unwrap_or(17895);
    if inner.port.load(Ordering::SeqCst) != 0 {
        let ip = lan_ip()?;
        return Ok(BridgeInfo {
            url: format!("http://{}:{}", ip, inner.port.load(Ordering::SeqCst)),
            lan_ip: ip,
            port: inner.port.load(Ordering::SeqCst) as u16,
        });
    }

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| AppError::Other(format!("端口 {port} 绑定失败（可能被占用）: {e}")))?;

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    *inner.shutdown.lock() = Some(tx);
    inner.port.store(port as u64, Ordering::SeqCst);

    let router = Router::new()
        .route("/", get(index_page))
        .route("/api/messages", get(list_messages))
        .route("/api/send", post(send_message))
        .with_state(AppCtx {
            inner: inner.clone(),
            app: app.clone(),
        });

    tauri::async_runtime::spawn(async move {
        let server = axum::serve(listener, router).with_graceful_shutdown(async {
            let _ = rx.await;
        });
        let _ = server.await;
    });

    let ip = lan_ip()?;
    Ok(BridgeInfo {
        url: format!("http://{ip}:{port}"),
        lan_ip: ip,
        port,
    })
}

#[tauri::command]
pub fn mobile_bridge_stop(state: tauri::State<'_, MobileBridgeState>) -> AppResult<()> {
    let inner = &state.0;
    if let Some(tx) = inner.shutdown.lock().take() {
        let _ = tx.send(());
    }
    inner.port.store(0, Ordering::SeqCst);
    inner.client_connected.store(false, Ordering::SeqCst);
    Ok(())
}

/// 桌面端向手机端推送消息（用户消息镜像 / AI 回复）
#[tauri::command]
pub fn mobile_bridge_push(
    state: tauri::State<'_, MobileBridgeState>,
    role: String,
    content: String,
) -> AppResult<()> {
    let inner = &state.0;
    if inner.port.load(Ordering::SeqCst) == 0 {
        return Ok(()); // 服务未启动，静默忽略
    }
    push_msg(inner, &role, &content);
    Ok(())
}

#[tauri::command]
pub fn mobile_bridge_status(state: tauri::State<'_, MobileBridgeState>) -> AppResult<serde_json::Value> {
    let inner = &state.0;
    Ok(serde_json::json!({
        "running": inner.port.load(Ordering::SeqCst) != 0,
        "connected": inner.client_connected.load(Ordering::SeqCst),
    }))
}

/// 生成真实二维码（SVG 字符串），内容为可扫描的 URL
#[tauri::command]
pub fn mobile_qr_svg(text: String) -> AppResult<String> {
    let code = qrcode::QrCode::new(text.as_bytes())
        .map_err(|e| AppError::Other(format!("二维码生成失败: {e}")))?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Ok(svg)
}

/// 手机端聊天页面（内联单文件，深色主题，长轮询同步）
/// 说明：微信/QQ 内置浏览器限制打开 http:// 局域网地址，
/// 请用手机自带相机或 Chrome/Safari 扫描二维码。
const MOBILE_PAGE: &str = r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no">
<title>ClawDesk 手机端</title>
<style>
  *{margin:0;padding:0;box-sizing:border-box;}
  body{background:#0d1117;color:#e6edf3;font-family:-apple-system,"PingFang SC","Microsoft YaHei",sans-serif;height:100dvh;display:flex;flex-direction:column;}
  header{padding:10px 14px;border-bottom:1px solid #21262d;font-size:13px;font-weight:600;display:flex;align-items:center;gap:6px;flex-shrink:0;}
  .dot{width:7px;height:7px;border-radius:50%;background:#3fb950;flex-shrink:0;}
  .dot.off{background:#f85149;}
  #msgs{flex:1;overflow-y:auto;padding:10px;display:flex;flex-direction:column;gap:8px;-webkit-overflow-scrolling:touch;}
  .msg{max-width:84%;padding:9px 12px;border-radius:12px;font-size:13px;line-height:1.55;white-space:pre-wrap;word-break:break-word;}
  .user{align-self:flex-end;background:#1f6feb;border-bottom-right-radius:3px;}
  .assistant{align-self:flex-start;background:#161b22;border:1px solid #21262d;border-bottom-left-radius:3px;}
  footer{padding:8px;border-top:1px solid #21262d;display:flex;gap:6px;flex-shrink:0;}
  #input{flex:1;background:#161b22;border:1px solid #30363d;border-radius:10px;padding:9px 10px;color:#e6edf3;font-size:13px;outline:none;-webkit-appearance:none;}
  #input:focus{border-color:#1f6feb;}
  #send{background:#1f6feb;color:#fff;border:none;border-radius:10px;padding:0 16px;font-size:13px;font-weight:500;white-space:nowrap;}
  #send:active{opacity:.7;}
  #send:disabled{opacity:.35;}
</style>
</head>
<body>
<header><span class="dot" id="dot"></span><span id="stat">ClawDesk · 连接中…</span></header>
<div id="msgs"></div>
<footer>
  <input id="input" placeholder="输入消息…" autocomplete="off" enterkeyhint="send">
  <button id="send">发送</button>
</footer>
<script>
  var latest=0,failCount=0,connected=false;
  var msgsEl=document.getElementById('msgs');
  var inputEl=document.getElementById('input');
  var sendBtn=document.getElementById('send');
  var dotEl=document.getElementById('dot');
  var statEl=document.getElementById('stat');
  /* 渲染消息列表 */
  function render(list){
    var atBottom=msgsEl.scrollHeight-msgsEl.scrollTop-msgsEl.clientHeight<70;
    for(var i=0;i<list.length;i++){
      var m=list[i],div=document.createElement('div');
      div.className='msg '+(m.role==='user'?'user':'assistant');
      div.textContent=m.content;
      msgsEl.appendChild(div);
    }
    if(list.length&&atBottom)msgsEl.scrollTop=msgsEl.scrollHeight;
  }
  /* 轮询拉取新消息 */
  function poll(){
    fetch('/api/messages?since='+latest).then(function(r){return r.json();}).then(function(j){
      latest=j.latest;render(j.msgs);failCount=0;
      if(!connected){connected=true;dotEl.className='dot';statEl.textContent='ClawDesk · 已连接';}
    }).catch(function(){
      failCount++;
      if(failCount>=3){dotEl.className='dot off';statEl.textContent='ClawDesk · 连接断开';}
    });
    setTimeout(poll,1500);
  }
  /* 发送消息 */
  function send(){
    var text=inputEl.value.trim();
    if(!text)return;
    sendBtn.disabled=true;inputEl.value='';
    fetch('/api/send',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({content:text})})
      .catch(function(){alert('发送失败，请检查网络连接');})
      .then(function(){sendBtn.disabled=false;inputEl.focus();});
  }
  sendBtn.onclick=send;
  inputEl.addEventListener('keydown',function(e){if(e.key==='Enter')send();});
  poll();
</script>
</body>
</html>"##;
