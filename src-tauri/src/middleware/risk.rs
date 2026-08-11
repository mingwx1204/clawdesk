//! 风险分级模块 —— 普通风险 / 高危风险 两级判定。
//!
//! 设计说明：
//! - 文档要求"区分普通风险、高危风险分级弹窗提示"（优化提示词三②）；
//! - `RiskLevel` 随 `ConfirmRequired` 事件推送前端，弹窗展示风险等级；
//! - 高危工具（`def.is_high_risk`）→ High；参数指向系统敏感目录 / 含
//!   危险命令片段 → High（前端红色警告 + 二次确认）；其余 → Normal；
//! - 绝对红线（系统目录、格式化等）仍由 HighRiskGuard 直接拦截，
//!   此处分级仅用于"需用户确认的操作"的弹窗展示，不替代拦截。

use serde::{Deserialize, Serialize};

use crate::core::tool::def::UnifiedToolDef;

/// 风险等级（前端弹窗展示：🟡 普通 / 🔴 高危）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// 普通风险：常规工具调用。
    Normal,
    /// 高危风险：写文件 / 终端 / 关闭窗口 / 参数含敏感路径等。
    High,
}

impl RiskLevel {
    pub fn is_high(&self) -> bool {
        matches!(self, Self::High)
    }
}

/// 危险命令片段（命中即高危，与 terminal 执行器双保险一致）。
/// 注意：比较时参数被 `to_lowercase`，因此标记必须全小写。
const DANGEROUS_CMD_MARKERS: &[&str] = &[
    "format ",
    "del /s",
    "rm -rf /",
    "remove-item -recurse -force",
    "diskpart",
    "shutdown /s",
    "reg delete",
];

/// 判定一次工具调用的风险等级。
///
/// 规则（按优先级）：
/// 1. 工具声明 `is_high_risk` → High（file_write / terminal / window_close 等）；
/// 2. 参数中含系统敏感路径 → High（读取或写入系统目录均有风险）；
/// 3. 参数中含危险命令片段 → High；
/// 4. 其余 → Normal。
pub fn risk_of(def: &UnifiedToolDef, arguments: &serde_json::Value) -> RiskLevel {
    if def.is_high_risk {
        return RiskLevel::High;
    }
    if contains_sensitive_path(arguments) {
        return RiskLevel::High;
    }
    if contains_dangerous_command(arguments) {
        return RiskLevel::High;
    }
    RiskLevel::Normal
}

/// 递归检查参数中是否存在指向系统敏感目录的路径值。
fn contains_sensitive_path(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(map) => map.values().any(contains_sensitive_path),
        serde_json::Value::Array(arr) => arr.iter().any(contains_sensitive_path),
        serde_json::Value::String(s) => is_sensitive_path(s),
        _ => false,
    }
}

/// 系统敏感路径检查（与 HighRiskGuard / analyze_image 双保险一致）。
///
/// ★ 防 junction/symlink 绕过（2026-08-12 修复）：先解析真实路径
///   （存在的祖先 canonicalize），再对标记做子串匹配 —— 否则通过目录链接
///   指向 `C:\Windows` 的路径会被字符串匹配漏掉。
///   注意：同时检查**原始字符串** —— Windows 下 POSIX 风格标记（/etc/ 等）
///   只存在于原始输入中，规范化后会被转成盘符路径而失配。
pub fn is_sensitive_path(p: &str) -> bool {
    let lower = p.to_lowercase();
    let resolved = super::sandbox::resolve_real_path(std::path::Path::new(p));
    let lower_resolved = resolved.to_string_lossy().to_lowercase();
    const MARKERS: &[&str] = &[
        "c:\\windows",
        "c:\\program files",
        "c:\\programdata",
        "/etc/",
        "/usr/",
        "/bin/",
        "/boot/",
        "/sys/",
        "/proc/",
        "/dev/",
        "\\.ssh",
        "/.ssh/",
    ];
    MARKERS.iter().any(|m| lower.contains(m) || lower_resolved.contains(m))
}

/// 递归检查参数中是否含危险命令片段（terminal 类工具）。
fn contains_dangerous_command(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(map) => map.values().any(contains_dangerous_command),
        serde_json::Value::Array(arr) => arr.iter().any(contains_dangerous_command),
        serde_json::Value::String(s) => {
            let lower = s.to_lowercase();
            DANGEROUS_CMD_MARKERS.iter().any(|m| lower.contains(m))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> UnifiedToolDef {
        UnifiedToolDef::new("builtin", "x", "x", vec![]).unwrap()
    }

    #[test]
    fn normal_tool_is_normal_risk() {
        let d = def();
        assert_eq!(risk_of(&d, &serde_json::json!({ "expression": "1+2" })), RiskLevel::Normal);
    }

    #[test]
    fn high_risk_flag_is_high() {
        let d = def().high_risk();
        assert_eq!(risk_of(&d, &serde_json::json!({ "path": "D:\\work\\a.txt" })), RiskLevel::High);
    }

    #[test]
    fn sensitive_path_is_high() {
        let d = def();
        assert_eq!(
            risk_of(&d, &serde_json::json!({ "path": "C:\\Windows\\System32\\config" })),
            RiskLevel::High
        );
        // 嵌套参数同样命中
        assert_eq!(
            risk_of(&d, &serde_json::json!({ "config": { "file_path": "/etc/shadow" } })),
            RiskLevel::High
        );
    }

    #[test]
    fn dangerous_command_is_high() {
        let d = def();
        assert_eq!(
            risk_of(&d, &serde_json::json!({ "command": "Remove-Item -Recurse -Force C:\\x" })),
            RiskLevel::High
        );
    }

    #[test]
    fn normal_path_is_normal() {
        let d = def();
        assert_eq!(
            risk_of(&d, &serde_json::json!({ "path": "D:\\workspace\\ClawDesk" })),
            RiskLevel::Normal
        );
    }

    #[test]
    fn sensitive_path_detector() {
        assert!(is_sensitive_path("C:\\Windows\\System32\\drivers\\etc"));
        assert!(is_sensitive_path("/home/u/.ssh/id_rsa"));
        assert!(!is_sensitive_path("D:\\workspace\\dev\\main.rs"));
    }
}
