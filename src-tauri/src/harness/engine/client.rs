//! LLM HTTP 客户端 —— 从 CodeWhale tui/src/client.rs + client/stream_entry.rs 剥离重构。
//!
//! 内置 BUG 修复：
//!   1. reqwest 强制 `http1_only()` 关闭 HTTP/2 —— 根治 `stream connection dropped`；
//!   2. `tcp_keepalive(30s)` + `pool_idle_timeout(90s)` + `tcp_nodelay(true)`；
//!   3. 连接超时 30s / 读超时 300s / 响应头等待 45s 独立配置。

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

use super::param::ModelParams;
use super::stream::SseStream;

/// 强制 IPv4 的 DNS 解析器。
///
/// 背景（2026-08 确诊）：`opencode.ai`（Cloudflare）DNS 同时返回 A 与 AAAA，
/// 且 **IPv6 排前**（`2606:4700:78::…`）。部分网络（运营商无 IPv6 路由）下这些
/// IPv6 地址是"黑洞"：SYN 被丢弃、不 RST 不响应。tokio `TcpStream::connect`
/// 对每个地址**串行尝试且无单地址超时** → 卡在 4 个 IPv6 上 30s+，触发
/// `connect_timeout(30s)` → reqwest 报
/// `error sending request for url (...): client error (Connect): TimedOut`。
/// （curl 有 happy-eyeballs 快速切换，所以 curl 正常、应用失败。）
///
/// 解决：解析后过滤掉 IPv6，只保留 IPv4 地址（该场景下 IPv4 一定可达）。
/// 对 LLM 端点、工具请求等所有经此 client 的域名生效。
#[derive(Clone, Default)]
pub struct Ipv4OnlyResolver;

impl reqwest::dns::Resolve for Ipv4OnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let err_map = |e: std::io::Error| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(e)
            };
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(err_map)?;
            let v4: Vec<std::net::SocketAddr> = addrs.filter(|a| a.is_ipv4()).collect();
            if v4.is_empty() {
                // 该域名没有 IPv4 记录（纯 IPv6 站点）→ 返回空列表快速失败，
                // 而不是像之前那样在 IPv6 黑洞上干等 30s。
                eprintln!("[DNS] {host} 无 IPv4 记录，返回空地址列表");
            }
            Ok(Box::new(v4.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// LLM 流式客户端（reqwest，HTTP/1.1 强制）。
#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    api_key: Arc<String>,
    base_url: String,
    /// 响应头等待超时（45s，对应 CodeWhale stream_entry DEFAULT_STREAM_OPEN_TIMEOUT）。
    open_timeout: Duration,
    /// 单块空闲超时（120s，透传给 SseStream）。
    idle_timeout: Duration,
    /// 请求整体超时（300s）。
    request_timeout: Duration,
}

impl LlmClient {
    /// 创建客户端。`base_url` 形如 `https://api.deepseek.com`（不含末尾斜杠与 /v1）。
    pub fn new(api_key: String, base_url: String) -> Result<Self> {
        // 确保 rustls crypto provider 已安装（reqwest 的 rustls-no-provider 特性要求，
        // 否则构建 Client 会 panic：`No rustls crypto provider is configured`）。
        // 这里在每次构建 reqwest 前安装，作为 main.rs 的双保险。
        // ★ 2026-08-12：安装状态只打印一次（原实现每次 new 都打两行日志 → 噪音）。
        static RUSTLS_PRINTED: std::sync::Once = std::sync::Once::new();
        match rustls::crypto::ring::default_provider().install_default() {
            Ok(_) => RUSTLS_PRINTED.call_once(|| {
                eprintln!("[RUSTLS] ring crypto provider installed");
            }),
            Err(e) => RUSTLS_PRINTED.call_once(|| {
                eprintln!("[RUSTLS] provider install skipped/failed: {e:?}");
            }),
        }
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("ClawDesk/0.1.0 (harness ", env!("CARGO_PKG_VERSION"), ")"))
            // 禁用系统代理：用户机器上的加速器/代理（WattAccelerator 等）会拦截 SSE 长连接，
            // 导致流式响应只收到开头数据就卡住。DeepSeek API 直连即可。
            .no_proxy()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .tcp_nodelay(true)
            .http1_only()
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            // ★ 修复 2026-08-10：IPv6 黑洞导致连接 30s 超时（opencode.ai DNS IPv6 优先）。
            //   强制 IPv4-only 解析，避免 tokio 串行连接卡在 IPv6 黑洞地址上。
            .dns_resolver(Ipv4OnlyResolver::default())
            .build()
            .context("构建 reqwest Client 失败")?;

        Ok(Self {
            http,
            api_key: Arc::new(api_key),
            base_url: base_url.trim_end_matches('/').to_string(),
            // ★ 2026-08-16 调大：本地 llama.cpp 模型首次加载/推理慢（25s+），
            //   45s 会超时报"SSE 响应头未返回"。
            open_timeout: Duration::from_secs(150),
            idle_timeout: Duration::from_secs(300),
            request_timeout: Duration::from_secs(300),
        })
    }

    /// 发起流式对话请求，返回 SSE 事件流。
    ///
    /// `messages` 为 OpenAI 兼容消息数组（Value）。
    pub async fn stream_chat(
        &self,
        params: &ModelParams,
        messages: &[serde_json::Value],
        system_prompt: Option<&str>,
        tools: &serde_json::Value,
    ) -> Result<SseStream> {
        let mut body = params.to_body_params();

        let mut msgs: Vec<serde_json::Value> = Vec::new();
        if let Some(sp) = system_prompt {
            msgs.push(serde_json::json!({ "role": "system", "content": sp }));
        }
        msgs.extend_from_slice(messages);
        body["messages"] = serde_json::Value::Array(msgs);
        body["tools"] = tools.clone();

        let url = format!("{}/v1/chat/completions", self.base_url);

        // 诊断摘要（不含请求体内容，仅打印模型与工具数量）
        eprintln!(
            "[SSE] 请求: model={} tools={} 条",
            body["model"],
            body["tools"].as_array().map(|a| a.len()).unwrap_or(0),
        );

        let send_fut = self
            .http
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key.as_str()))
            .json(&body)
            .timeout(self.request_timeout)
            .send();

        let response = match tokio::time::timeout(self.open_timeout, send_fut).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => bail!("发送流式请求失败: {e}"),
            Err(_) => bail!(
                "SSE 响应头 {}s 内未返回（HTTP/1.1）。请检查网络/端点。",
                self.open_timeout.as_secs()
            ),
        };

        let status = response.status();
        eprintln!(
            "[SSE] 响应头: status={}, content-type={:?}, transfer-encoding={:?}, content-length={:?}",
            status,
            response.headers().get("content-type"),
            response.headers().get("transfer-encoding"),
            response.headers().get("content-length"),
        );
        if !status.is_success() {
            let status_text = response.text().await.unwrap_or_default();
            bail!("API 错误: HTTP {} — {}", status.as_u16(), status_text);
        }

        Ok(SseStream::new(response.bytes_stream(), self.idle_timeout))
    }

    /// 非流式请求（上下文压缩/总结等场景）。
    pub async fn chat_once(
        &self,
        params: &ModelParams,
        messages: &[serde_json::Value],
        system_prompt: Option<&str>,
    ) -> Result<String> {
        let mut p = params.clone();
        p.stream = false;

        let mut body = p.to_body_params();
        let mut msgs: Vec<serde_json::Value> = Vec::new();
        if let Some(sp) = system_prompt {
            msgs.push(serde_json::json!({ "role": "system", "content": sp }));
        }
        msgs.extend_from_slice(messages);
        body["messages"] = serde_json::Value::Array(msgs);
        let url = format!("{}/v1/chat/completions", self.base_url);
        let response = self
            .http
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key.as_str()))
            .json(&body)
            .timeout(self.request_timeout)
            .send()
            .await
            .context("发送非流式请求失败")?;

        let status = response.status();
        let text = response.text().await.context("读取响应体失败")?;
        if !status.is_success() {
            bail!("API 错误: HTTP {} — {}", status.as_u16(), text);
        }

        let parsed: serde_json::Value =
            serde_json::from_str(&text).context("解析响应 JSON 失败")?;
        Ok(parsed["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    /// 当前端点摘要（不含 key，供状态展示）。
    pub fn endpoint_summary(&self) -> serde_json::Value {
        serde_json::json!({ "baseUrl": self.base_url, "protocol": "HTTP/1.1" })
    }
}
