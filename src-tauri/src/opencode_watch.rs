//! opencode 网关持续检测 + 自动回切（2026-08-09 新增）。
//!
//! 背景：用户主/视觉模型配置在 opencode.ai/zen/go 网关（服务端曾宕机返回 500），
//! 已切到 DeepSeek 官方备用。此模块后台定期检测 opencode 端点是否恢复，
//! 恢复后自动把主/视觉端点与 Key 切回 opencode，并通知前端。
//!
//! 开关与配置见 AppSettings：`opencodeWatchEnabled` / `opencodeWatchEndpoint`
//! / `opencodeWatchApiKey` / `opencodeWatchIntervalSecs`。

use std::sync::Arc;
use std::time::Duration;

use tauri::Emitter;

use crate::llm::settings::{ApiKeys, SettingsStore};

/// 后台任务轮询的最小间隔（秒），防止配置了 0/过小值导致高频请求。
const MIN_INTERVAL_SECS: u64 = 30;

/// 启动后台检测任务（lib.rs setup 调用）。
pub fn spawn(app: tauri::AppHandle, settings: Arc<SettingsStore>) {
    tauri::async_runtime::spawn(async move {
        eprintln!("[OPCODE-WATCH] 后台检测任务已启动");
        loop {
            let s = settings.get();
            if !s.opencode_watch_enabled {
                // 未开启：低频休眠，避免空转
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let interval = s.opencode_watch_interval_secs.max(MIN_INTERVAL_SECS);
            tokio::time::sleep(Duration::from_secs(interval)).await;

            let s2 = settings.get();
            let endpoint = s2.opencode_watch_endpoint.clone();
            // ★ Key 走加密存储（ApiKeys.opencode_watch，DPAPI）
            let key = settings.keys().opencode_watch.clone();
            if endpoint.trim().is_empty() || key.trim().is_empty() {
                continue;
            }
            match check_endpoint(&endpoint, &key).await {
                Ok(true) => {
                    eprintln!("[OPCODE-WATCH] ✅ opencode 网关已恢复（{}）", endpoint);
                    if do_switch(&app, &settings) {
                        let _ = app.emit(
                            "opencode-watch-event",
                            serde_json::json!({
                                "switched": true,
                                "endpoint": endpoint,
                                "at": chrono_now_ms(),
                            }),
                        );
                    }
                }
                Ok(false) => {
                    eprintln!("[OPCODE-WATCH] 尚未恢复（仍非 200）");
                }
                Err(e) => {
                    eprintln!("[OPCODE-WATCH] 检测失败: {e}");
                }
            }
        }
    });
}

/// 探测端点：POST 一个最小 chat 请求，**HTTP 2xx 且响应体含有效 choices** 才视为恢复。
/// 只测状态码曾误判（opencode 偶发返回 200 但内容为错误 JSON），
/// 校验 choices 确保真实可用才回切。
async fn check_endpoint(endpoint: &str, api_key: &str) -> Result<bool, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        // ★ IPv6 黑洞修复（2026-08-10）：opencode.ai DNS IPv6 优先且本机 IPv6 不可达，
        //   tokio 串行连接卡 IPv6 → 15s 连接超时。强制 IPv4 解析。
        .dns_resolver(crate::harness::engine::client::Ipv4OnlyResolver::default())
        .build()
        .map_err(|e| format!("构建检测客户端失败: {e}"))?;
    let body = serde_json::json!({
        "model": "deepseek-v4-flash",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 5,
    });
    let resp = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    // 打印状态码 + 响应体前缀供诊断（500=宕机；401=key 问题；2xx=待校验）
    eprintln!(
        "[OPCODE-WATCH] 检测状态码: {} 响应: {}",
        status.as_u16(),
        text.chars().take(120).collect::<String>()
    );
    if !status.is_success() {
        return Ok(false);
    }
    // 校验响应体含有效 choices（防 200 但错误 JSON 的假恢复）
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) if v.get("choices").and_then(|c| c.as_array()).map(|a| !a.is_empty()).unwrap_or(false) => {
            Ok(true)
        }
        _ => {
            eprintln!("[OPCODE-WATCH] 200 但响应体无有效 choices，判定未恢复");
            Ok(false)
        }
    }
}

/// 执行回切：主/视觉端点 → opencode；主/视觉 Key → opencode key。
/// 成功后自动关闭开关（一次性任务），并同步内存 / keys.enc / settings.json。
fn do_switch(app: &tauri::AppHandle, settings: &Arc<SettingsStore>) -> bool {
    let s = settings.get();
    let endpoint = s.opencode_watch_endpoint.trim().to_string();
    let key = settings.keys().opencode_watch.trim().to_string();
    if endpoint.is_empty() || key.is_empty() {
        eprintln!("[OPCODE-WATCH] 回切失败：端点或 Key 为空");
        return false;
    }

    // 1. 更新 AppSettings（端点 + 视觉模型名；主模型名保持用户当前选择）
    //    visionModel 必须一并切为 opencode 网关的视觉模型（mimo-v2.5，实测支持图片输入），
    //    否则端点 opencode + 模型 glm-4v-flash（智谱）不匹配，识图会失败。
    if let Err(e) = settings.apply(serde_json::json!({
        "modelEndpoint": endpoint,
        "visionEndpoint": endpoint,
        "visionModel": "mimo-v2.5",
    })) {
        eprintln!("[OPCODE-WATCH] 更新设置失败: {e}");
        return false;
    }

    // 2. 同步 settings.json 的 apiKeys 字段（预置优先级高于 keys.enc，
    //    必须同时更新，否则重启后仍用旧 key）
    update_settings_file_api_keys(&key);

    // 3. 更新内存 + keys.enc
    let cur = settings.keys();
    settings.set_keys(ApiKeys {
        main: key.clone(),
        vision: key.clone(),
        image: cur.image.clone(),
        opencode_watch: cur.opencode_watch.clone(),
    });

    // 4. 自动关闭开关（避免反复写盘；用户可再次开启）
    let _ = settings.apply(serde_json::json!({ "opencodeWatchEnabled": false }));

    let _ = app.emit(
        "opencode-watch-event",
        serde_json::json!({
            "switched": true,
            "endpoint": endpoint,
            "at": chrono_now_ms(),
        }),
    );
    eprintln!(
        "[OPCODE-WATCH] ✅ 已自动回切 opencode（端点 {}，Key {}…）",
        endpoint,
        key.chars().take(8).collect::<String>()
    );
    true
}

/// 直接改写 settings.json 的 apiKeys.main / apiKeys.vision（保留 image）。
fn update_settings_file_api_keys(new_key: &str) {
    let path = crate::llm::settings::settings_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("[OPCODE-WATCH] 读取 settings.json 失败，跳过 apiKeys 同步");
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if let Some(keys) = v.get_mut("apiKeys").and_then(|k| k.as_object_mut()) {
        keys.insert("main".into(), serde_json::Value::String(new_key.to_string()));
        keys.insert("vision".into(), serde_json::Value::String(new_key.to_string()));
        if let Ok(out) = serde_json::to_string_pretty(&v) {
            let _ = std::fs::write(&path, out);
            eprintln!("[OPCODE-WATCH] settings.json apiKeys 已同步");
        }
    }
}

/// 当前 Unix 毫秒时间戳（事件载荷用）。
fn chrono_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
