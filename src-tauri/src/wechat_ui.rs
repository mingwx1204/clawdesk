//! 微信白名单（可聊天对象）—— 独立微信路线的安全红线。
//!
//! 供虚拟机微信（vm_vnc.rs 的 vm_send / vm_fetch_file）与既有工具强制校验：
//! AI 只能给白名单里的对象发消息，白名单为空时拒绝发送。
//! 持久化到 `<数据目录>/wechat_ui.json`（键名沿用窗口句柄时代的 "vm"）。

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::json;

/// 每个微信的白名单（key → 允许聊天的对象名列表）。
static WHITELIST: OnceLock<parking_lot::Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

fn whitelist_map() -> &'static parking_lot::Mutex<HashMap<String, Vec<String>>> {
    WHITELIST.get_or_init(|| {
        let m = load_whitelist().unwrap_or_default();
        parking_lot::Mutex::new(m)
    })
}

fn whitelist_path() -> std::path::PathBuf {
    crate::llm::settings::clawdesk_dir().join("wechat_ui.json")
}

fn load_whitelist() -> Option<HashMap<String, Vec<String>>> {
    let text = std::fs::read_to_string(whitelist_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut out = HashMap::new();
    for (k, users) in v.get("whitelist")?.as_object()?.iter() {
        let list = users
            .as_array()?
            .iter()
            .filter_map(|u| u.as_str().map(String::from))
            .collect();
        out.insert(k.clone(), list);
    }
    Some(out)
}

fn save_whitelist(map: &HashMap<String, Vec<String>>) -> Result<(), String> {
    let path = whitelist_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let v = json!({ "whitelist": map });
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|e| format!("白名单持久化失败: {e}"))
}

/// 读取指定微信的可聊天对象白名单。
pub fn whitelist_of(key: &str) -> Vec<String> {
    whitelist_map().lock().get(key).cloned().unwrap_or_default()
}

/// 设置白名单（逗号/顿号分隔；空字符串 = 清空）。
pub fn set_whitelist(key: &str, users: &str) -> Result<Vec<String>, String> {
    let list: Vec<String> = users
        .split([',', '，', '、', '\n', '\r', ' '])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut map = whitelist_map().lock();
    if list.is_empty() {
        map.remove(key);
    } else {
        map.insert(key.to_string(), list.clone());
    }
    let snapshot = map.clone();
    drop(map);
    save_whitelist(&snapshot)?;
    Ok(list)
}

/// 校验目标是否在白名单内（大小写不敏感的子串匹配）。
pub fn check_whitelist(key: &str, to: &str) -> Result<(), String> {
    let list = whitelist_of(key);
    if list.is_empty() {
        return Err("该微信未设置可聊天对象（白名单）——AI 不允许发送消息".into());
    }
    let to_lower = to.to_lowercase();
    if !list.iter().any(|u| to_lower.contains(&u.to_lowercase())) {
        return Err(format!(
            "{to} 不在可聊天白名单（{}）中，拒绝发送",
            list.join(" / ")
        ));
    }
    Ok(())
}

/// 等待（虚拟机 UI 操作间的稳定延时）。
pub fn wait_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 白名单持久化 roundtrip（临时数据目录）。
    #[test]
    fn whitelist_save_load_roundtrip() {
        // ★ 共享串行锁：与其他改 CLAWDESK_DATA_DIR 的测试互斥
        let _g = crate::llm::logging::test_env_lock();
        let dir = std::env::temp_dir().join(format!("clawdesk-wxui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = std::env::var("CLAWDESK_DATA_DIR").ok();
        std::env::set_var("CLAWDESK_DATA_DIR", &dir);

        let list = set_whitelist("12345", "小明，小红 老王").unwrap();
        assert_eq!(list, vec!["小明", "小红", "老王"]);
        assert_eq!(whitelist_of("12345"), vec!["小明", "小红", "老王"]);
        assert_eq!(whitelist_of("99999"), Vec::<String>::new());

        let list2 = set_whitelist("12345", "").unwrap();
        assert!(list2.is_empty());
        assert_eq!(whitelist_of("12345"), Vec::<String>::new());

        if let Some(v) = old {
            std::env::set_var("CLAWDESK_DATA_DIR", v);
        } else {
            std::env::remove_var("CLAWDESK_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 白名单校验：未设置 / 不在名单内 → 拒绝。
    #[test]
    fn whitelist_check_blocks_unlisted() {
        // ★ 共享串行锁：与其他改 CLAWDESK_DATA_DIR 的测试互斥
        let _g = crate::llm::logging::test_env_lock();
        // ★ 隔离数据目录（与其他白名单测试互不干扰，也避免写入真实数据目录）
        let dir = std::env::temp_dir().join(format!("clawdesk-wxui2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let old = std::env::var("CLAWDESK_DATA_DIR").ok();
        std::env::set_var("CLAWDESK_DATA_DIR", &dir);

        let err = check_whitelist("777", "陌生人").unwrap_err();
        assert!(err.contains("未设置可聊天对象"), "{err}");

        let _ = set_whitelist("777", "小明");
        let err = check_whitelist("777", "陌生人").unwrap_err();
        assert!(err.contains("不在可聊天白名单"), "{err}");

        check_whitelist("777", "小明-工作号").unwrap();

        if let Some(v) = old {
            std::env::set_var("CLAWDESK_DATA_DIR", v);
        } else {
            std::env::remove_var("CLAWDESK_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
