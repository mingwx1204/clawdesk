//! 余额查询 & 模型列表探测 IPC 命令。
//!
//! 包含：extract_origin / detect_provider（辅助），check_balance / deepseek_balance / list_models（命令）。

use std::time::Duration;

/// 从完整端点 URL 提取 origin（scheme://host）。
/// - `https://api.deepseek.com/chat/completions` → `https://api.deepseek.com`
/// - `https://api.siliconflow.cn/images/generations` → `https://api.siliconflow.cn`
fn extract_origin(endpoint: &str) -> String {
    let e = endpoint.trim();
    if let Some(after_scheme) = e.strip_prefix("https://") {
        if let Some(host_end) = after_scheme.find('/') {
            format!("https://{}", &after_scheme[..host_end])
        } else {
            format!("https://{}", after_scheme)
        }
    } else if let Some(after_scheme) = e.strip_prefix("http://") {
        if let Some(host_end) = after_scheme.find('/') {
            format!("http://{}", &after_scheme[..host_end])
        } else {
            format!("http://{}", after_scheme)
        }
    } else {
        e.to_string()
    }
}

/// 从端点 host 名识别提供商（余额查询用）。
fn detect_provider(endpoint: &str) -> &'static str {
    let e = endpoint.trim().to_lowercase();
    if e.contains("opencode.ai") {
        "opencode-go"
    } else if e.contains("api.deepseek.com") {
        "deepseek"
    } else if e.contains("api.siliconflow.cn") {
        "siliconflow"
    } else if e.contains("api.z.ai") || e.contains("open.bigmodel.cn") {
        "zai"
    } else if e.contains("api.openai.com") {
        "openai"
    } else {
        "unknown"
    }
}

/// 查询账户余额：根据端点 URL 自动识别提供商（DeepSeek / SiliconFlow / …），调用对应余额 API。
#[tauri::command]
pub async fn check_balance(api_key: String, endpoint: String) -> Result<serde_json::Value, String> {
    let key = api_key.trim().to_string();
    let origin = extract_origin(&endpoint);
    let provider = detect_provider(&endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        // ★ IPv6 黑洞修复（2026-08-10）
        .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;

    match provider {
        "deepseek" => {
            let resp = client
                .get(format!("{}/user/balance", origin))
                .header("Authorization", format!("Bearer {}", key))
                .send()
                .await
                .map_err(|e| format!("查询余额失败（网络）: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(format!(
                    "查询余额失败 HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            serde_json::from_str(&text).map_err(|e| format!("解析余额响应失败: {e}"))
        }
        "siliconflow" => {
            let resp = client
                .get(format!("{}/v1/user/info", origin))
                .header("Authorization", format!("Bearer {}", key))
                .send()
                .await
                .map_err(|e| format!("查询余额失败（网络）: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if !status.is_success() {
                return Err(format!(
                    "查询余额失败 HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("解析余额响应失败: {e}"))?;
            // SiliconFlow 响应格式: { status: bool, data: { balance: string } }
            let balance = v
                .get("data")
                .and_then(|d| d.get("balance"))
                .and_then(|b| b.as_str())
                .unwrap_or("0");
            Ok(serde_json::json!({
                "is_available": true,
                "balance_infos": [{
                    "currency": "¥",
                    "total_balance": balance,
                    "granted_balance": "0",
                    "topped_up_balance": balance
                }]
            }))
        }
        "opencode-go" => {
            // OpenCode Go 无公开余额 API。发送一个最小 chat 请求验证 key 有效性。
            let resp = client
                .post(format!("{}/zen/go/v1/chat/completions", origin))
                .header("Authorization", format!("Bearer {}", key))
                .json(&serde_json::json!({
                    "model": "deepseek-v4-flash",
                    "messages": [{ "role": "user", "content": "ping" }],
                    "max_tokens": 1
                }))
                .send()
                .await
                .map_err(|e| format!("查询余额失败（网络）: {e}"))?;
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                let hint = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/error/message")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| text.chars().take(200).collect::<String>());
                let reset = retry_after
                    .as_deref()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|secs| {
                        if secs >= 3600 {
                            format!("约 {} 小时后重置", secs / 3600)
                        } else {
                            format!("约 {} 分钟后重置", secs / 60 + 1)
                        }
                    })
                    .unwrap_or_else(|| "稍后重置".to_string())
                    .to_string();
                return Err(format!(
                    "已达 OpenCode Go 使用限额（{}）
提示：{}（{reset}）",
                    status, hint
                ));
            }
            if !status.is_success() {
                return Err(format!(
                    "查询余额失败 HTTP {status}: {}",
                    text.chars().take(200).collect::<String>()
                ));
            }
            Ok(serde_json::json!({
                "is_available": true,
                "note": "OpenCode Go 订阅（每月 $10，含约 $60 使用额度）无公开余额 API，已通过最小请求验证 Key 有效且额度可用",
                "balance_infos": [{
                    "currency": "Go",
                    "total_balance": "Key 有效 · 额度可用",
                    "granted_balance": "0",
                    "topped_up_balance": "0"
                }]
            }))
        }
        "zai" | "openai" => Err(format!(
            "{} 官方不支持简单余额查询，请前往官网查看",
            if provider == "zai" { "智谱(Z.ai)" } else { "OpenAI" }
        )),
        other => Err(format!("暂不支持该提供商的余额查询（{}），请前往官网查看", other)),
    }
}

/// 查询 DeepSeek 账户余额（兼容旧接口，内部调用 check_balance）。
#[tauri::command]
pub async fn deepseek_balance(api_key: String) -> Result<serde_json::Value, String> {
    check_balance(api_key, "https://api.deepseek.com/chat/completions".to_string()).await
}

/// 自动检索填入的 Key 支持哪些模型：
/// - OpenAI 兼容端点走 `GET {origin}/models`；
/// - 返回 `{ provider, endpoint, models: [{ id, owned_by? }] }`，按 id 排序去重。
#[tauri::command]
pub async fn list_models(
    api_key: String,
    endpoint: String,
) -> Result<serde_json::Value, String> {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err("API Key 不能为空".into());
    }
    let origin = extract_origin(&endpoint);
    let provider = detect_provider(&endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))?;

    let candidates = vec![format!("{origin}/models"), format!("{origin}/v1/models")];

    let mut last_err = String::new();
    let mut raw: Option<serde_json::Value> = None;
    let mut used_url = String::new();
    for url in &candidates {
        match client
            .get(url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                        raw = Some(v);
                        used_url = url.clone();
                        break;
                    }
                    last_err = format!("解析 {} 响应失败", url);
                } else {
                    last_err = format!("HTTP {status}: {}", text.chars().take(200).collect::<String>());
                }
            }
            Err(e) => last_err = format!("请求 {} 失败: {e}", url),
        }
    }

    let mut models: Vec<serde_json::Value> = Vec::new();
    if let Some(v) = &raw {
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for item in arr {
                let id = item.get("id").and_then(|i| i.as_str());
                let owned = item.get("owned_by").and_then(|o| o.as_str());
                if let Some(id) = id {
                    let mut entry = serde_json::json!({ "id": id });
                    if let Some(owned) = owned {
                        entry["owned_by"] = serde_json::Value::String(owned.to_string());
                    }
                    models.push(entry);
                }
            }
        }
    }

    models.sort_by(|a, b| {
        a.get("id").and_then(|i| i.as_str()).unwrap_or("")
            .cmp(b.get("id").and_then(|i| i.as_str()).unwrap_or(""))
    });
    models.dedup_by(|a, b| {
        a.get("id").and_then(|i| i.as_str())
            == b.get("id").and_then(|i| i.as_str())
    });

    if models.is_empty() {
        return Err(format!(
            "未能从该端点枚举到任何模型（{provider} / {origin}）。
提示：确认端点支持 OpenAI 兼容 /models 接口，或手动填写模型名。
细节：{last_err}"
        ));
    }

    Ok(serde_json::json!({
        "provider": provider,
        "endpoint": origin,
        "models_url": used_url,
        "count": models.len(),
        "models": models,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_origin_strips_path() {
        assert_eq!(
            extract_origin("https://api.deepseek.com/chat/completions"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            extract_origin("https://api.siliconflow.cn/images/generations"),
            "https://api.siliconflow.cn"
        );
        assert_eq!(
            extract_origin("https://open.bigmodel.cn/api/paas/v4/chat/completions"),
            "https://open.bigmodel.cn"
        );
    }

    #[test]
    fn extract_origin_handles_bare_host_and_http() {
        assert_eq!(extract_origin("https://api.deepseek.com"), "https://api.deepseek.com");
        assert_eq!(extract_origin("http://127.0.0.1:8080/v1/chat/completions"), "http://127.0.0.1:8080");
        assert_eq!(extract_origin("   "), ""); // trim 后为空字符串
    }

    #[test]
    fn detect_provider_recognizes_known_hosts() {
        assert_eq!(detect_provider("https://opencode.ai/zen/go/v1/chat/completions"), "opencode-go");
        assert_eq!(detect_provider("https://api.deepseek.com/chat/completions"), "deepseek");
        assert_eq!(detect_provider("https://api.siliconflow.cn/images/generations"), "siliconflow");
        assert_eq!(detect_provider("https://api.z.ai/chat/completions"), "zai");
        assert_eq!(detect_provider("https://open.bigmodel.cn/api/paas/v4/chat/completions"), "zai");
        assert_eq!(detect_provider("https://api.openai.com/v1/chat/completions"), "openai");
        assert_eq!(detect_provider("https://my-custom.example.com/chat/completions"), "unknown");
    }
}
