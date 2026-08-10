//! `SensitiveFileGuardMiddleware` —— 敏感文件保护中间件。
//!
//! 契约：
//! - 工具参数中的路径字段（path / file_path / image_path / output_dir 等，
//!   递归查找）若指向**密钥/凭据类文件**（`.env`、`*.pem`、`*.key`、
//!   `id_rsa`、`credentials`、`secrets`、`*token*` 等），默认拦截；
//! - 与 `SandboxMiddleware`（白名单目录）互补：沙箱管"能去哪"，
//!   本层管"哪些文件不能碰"；与 `HighRiskGuardMiddleware`（系统目录黑名单）
//!   互补：本层针对的是**文件名**而非目录；
//! - 可运行时开关（`set_enabled` / IPC 命令），开关状态仅存内存，
//!   出厂默认开启；
//! - 读与写均拦截（读 `.env` 同样可能泄露密钥），错误消息引导用户
//!   确需访问时在设置中关闭本保护。

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use crate::core::tool::def::UnifiedToolDef;
use crate::core::tool::dispatcher::{BoxFuture, Middleware, ToolCall};
use crate::core::tool::error::ToolError;

/// 会被检查的路径字段名（与 HighRiskGuard / Sandbox 保持一致，递归匹配）。
const PATH_KEYS: &[&str] = &[
    "path",
    "file_path",
    "image_path",
    "output_dir",
    "output_path",
    "dir",
    "directory",
];

/// 敏感文件名模式（小写，匹配文件 basename；`*` 为通配符）。
///
/// 覆盖四类：环境变量 / 私钥证书 / 凭据密钥 / 口令文件。
const SENSITIVE_PATTERNS: &[&str] = &[
    // 环境变量与包管理器凭据
    ".env*",
    ".npmrc",
    ".pypirc",
    ".netrc",
    ".pgpass",
    ".wgetrc",
    ".dockerconfigjson",
    // 私钥 / 证书 / 密钥库
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.keystore",
    "*.kdbx",
    "*.der",
    "*.p7b",
    "*.ovpn",
    "id_rsa*",
    "id_dsa*",
    "id_ecdsa*",
    "id_ed25519*",
    // 凭据 / 密钥 / Token
    "credentials*",
    "secrets*",
    "secret*",
    "*token*",
    "*apikey*",
    "*api-key*",
    // 口令文件
    "passwd",
    "shadow",
    "*passwd*",
    "*shadow*",
];

/// 敏感文件守卫中间件：默认开启，可运行时关闭。
pub struct SensitiveFileGuardMiddleware {
    enabled: AtomicBool,
}

impl SensitiveFileGuardMiddleware {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(true),
        }
    }

    /// 运行时开关（IPC / 设置页调用）。
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, AtomicOrdering::SeqCst);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(AtomicOrdering::SeqCst)
    }
}

impl Default for SensitiveFileGuardMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for SensitiveFileGuardMiddleware {
    fn name(&self) -> &'static str {
        "sensitive_file_guard"
    }

    fn before<'a>(
        &'a self,
        _def: &'a UnifiedToolDef,
        call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            if !self.enabled.load(AtomicOrdering::SeqCst) {
                return Ok(());
            }
            if let Some(bad) = find_sensitive_path_arg(&call.arguments) {
                return Err(ToolError::middleware_rejected(
                    "sensitive_file_guard",
                    format!(
                        "敏感文件保护拦截: 参数指向密钥/凭据类文件 `{}`。\
                         如确需访问，请在设置中关闭“敏感文件保护”后再试。",
                        bad
                    ),
                ));
            }
            Ok(())
        })
    }
}

/// 递归查找参数中指向敏感文件的路径字段值。
fn find_sensitive_path_arg(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if PATH_KEYS.contains(&key.as_str()) {
                    if let Some(s) = val.as_str() {
                        if is_sensitive_file(s) {
                            return Some(s.to_string());
                        }
                    }
                }
                if let Some(found) = find_sensitive_path_arg(val) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(find_sensitive_path_arg),
        _ => None,
    }
}

/// 判断路径的 basename 是否命中敏感文件模式。
fn is_sensitive_file(p: &str) -> bool {
    let name = std::path::Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.is_empty() {
        return false;
    }
    SENSITIVE_PATTERNS
        .iter()
        .any(|pat| match_pattern(pat, &name))
}

/// 通配符匹配：`*abc`（后缀）/ `abc*`（前缀）/ `*abc*`（包含）/ 精确。
fn match_pattern(pat: &str, name: &str) -> bool {
    let starts_wild = pat.starts_with('*');
    let ends_wild = pat.ends_with('*');
    let core = pat.trim_matches('*');
    if starts_wild && ends_wild {
        name.contains(core)
    } else if starts_wild {
        name.ends_with(core)
    } else if ends_wild {
        name.starts_with(core)
    } else {
        name == pat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_expected() {
        assert!(match_pattern(".env*", ".env"));
        assert!(match_pattern(".env*", ".env.local"));
        assert!(match_pattern("*.pem", "server.pem"));
        assert!(match_pattern("*token*", "github_token.json"));
        assert!(match_pattern("id_rsa", "id_rsa"));
        assert!(match_pattern("credentials*", "credentials.json"));
        assert!(!match_pattern("*.pem", "readme.md"));
        assert!(!match_pattern(".env*", "environment.txt"));
    }

    #[test]
    fn sensitive_file_detection() {
        assert!(is_sensitive_file("C:\\proj\\.env"));
        assert!(is_sensitive_file("/home/u/.ssh/id_rsa"));
        assert!(is_sensitive_file("C:\\keys\\server.key"));
        assert!(is_sensitive_file("./config/secrets.json"));
        assert!(!is_sensitive_file("C:\\proj\\src\\main.rs"));
        assert!(!is_sensitive_file("C:\\proj\\package.json"));
    }
}
