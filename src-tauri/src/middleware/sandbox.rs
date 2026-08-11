//! 沙箱隔离模块 —— 工具文件操作白名单约束（项目 3）。
//!
//! 设计说明：
//! - `SandboxManager` 维护允许访问的根目录白名单，默认包含：
//!   当前工作目录、用户主目录（USERPROFILE）、系统临时目录、应用数据目录；
//! - `SandboxMiddleware` 在工具分发前检查路径参数（path / file_path /
//!   image_path / output_dir 等，递归查找），指向白名单外 → 拦截，
//!   返回标准化错误引导模型使用授权目录或请求用户授权；
//! - 系统敏感目录（C:\Windows 等）由 HighRiskGuard 黑名单拦截，
//!   沙箱是**白名单层**：即使非敏感目录，未授权也不得访问；
//! - 用户可通过 IPC（sandbox_add_root / sandbox_remove_root）扩展授权范围。

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::core::tool::def::UnifiedToolDef;
use crate::core::tool::dispatcher::{BoxFuture, Middleware, ToolCall};
use crate::core::tool::error::ToolError;

/// 会被沙箱检查的路径字段名（与 HighRiskGuard 保持一致，递归匹配）。
const PATH_KEYS: &[&str] = &[
    "path",
    "file_path",
    "image_path",
    "output_dir",
    "output_path",
    "dir",
    "directory",
];

/// 沙箱管理器：线程安全的白名单根集合。
#[derive(Debug)]
pub struct SandboxManager {
    roots: RwLock<Vec<PathBuf>>,
}

impl SandboxManager {
    /// 创建沙箱并写入默认授权根。
    pub fn new() -> Self {
        let mgr = Self {
            roots: RwLock::new(Vec::new()),
        };
        for root in default_roots() {
            let _ = mgr.add_root(&root);
        }
        mgr
    }

    /// 添加一个授权根（规范化后去重）。
    pub fn add_root(&self, path: &str) -> bool {
        if let Some(norm) = normalize(path) {
            let mut roots = self.roots.write().unwrap();
            if !roots.iter().any(|r| r == &norm) {
                roots.push(norm);
                return true;
            }
        }
        false
    }

    /// 移除一个授权根（返回是否找到并移除）。
    pub fn remove_root(&self, path: &str) -> bool {
        if let Some(norm) = normalize(path) {
            let mut roots = self.roots.write().unwrap();
            if let Some(idx) = roots.iter().position(|r| r == &norm) {
                roots.remove(idx);
                return true;
            }
        }
        false
    }

    /// 当前全部授权根（字符串形式，供 IPC / 环境快照使用）。
    pub fn roots(&self) -> Vec<String> {
        self.roots
            .read()
            .unwrap()
            .iter()
            .map(|r| r.to_string_lossy().to_string())
            .collect()
    }

    /// 判断路径是否在任一授权根下（含等于根自身）。
    ///
    /// ★ 防 junction/symlink 绕过（2026-08-12 修复）：比较前对双方做真实路径解析
    ///   （canonicalize 存在的祖先），字符串规范化无法识别的目录链接会被还原。
    pub fn is_allowed(&self, path: &str) -> bool {
        let Some(norm) = normalize(path) else {
            return false;
        };
        let real = resolve_real_path(&norm);
        let roots = self.roots.read().unwrap();
        roots
            .iter()
            .any(|root| is_within(&real, &resolve_real_path(root)))
    }
}

impl Default for SandboxManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 沙箱中间件：拦截指向白名单外的路径参数。
pub struct SandboxMiddleware {
    sandbox: Arc<SandboxManager>,
}

impl SandboxMiddleware {
    pub fn new(sandbox: Arc<SandboxManager>) -> Self {
        Self { sandbox }
    }
}

impl Middleware for SandboxMiddleware {
    fn name(&self) -> &'static str {
        "sandbox"
    }

    fn before<'a>(
        &'a self,
        _def: &'a UnifiedToolDef,
        call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            if let Some(bad) = find_out_of_sandbox_path(&self.sandbox, &call.arguments) {
                return Err(ToolError::middleware_rejected(
                    "sandbox",
                    format!(
                        "沙箱隔离拦截: 路径 `{}` 不在授权目录内。当前授权根: {}。请改用授权目录内的路径，或请求用户通过 sandbox_add_root 授权该目录",
                        bad,
                        self.sandbox.roots().join("; ")
                    ),
                ));
            }
            Ok(())
        })
    }
}

/// 递归查找参数中指向沙箱外的路径字段值。
fn find_out_of_sandbox_path(sandbox: &SandboxManager, v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                if PATH_KEYS.contains(&key.as_str()) {
                    if let Some(s) = val.as_str() {
                        if !s.is_empty() && !sandbox.is_allowed(s) {
                            return Some(s.to_string());
                        }
                    }
                }
                if let Some(found) = find_out_of_sandbox_path(sandbox, val) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(|v| find_out_of_sandbox_path(sandbox, v)),
        _ => None,
    }
}

/// 默认授权根：工作目录 + 用户主目录 + 临时目录 + 应用数据目录。
fn default_roots() -> Vec<String> {
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.to_string_lossy().to_string());
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        roots.push(home);
    }
    if let Ok(home) = std::env::var("HOME") {
        roots.push(home);
    }
    roots.push(std::env::temp_dir().to_string_lossy().to_string());
    if let Ok(appdata) = std::env::var("APPDATA") {
        roots.push(std::path::Path::new(&appdata).join("clawdesk").to_string_lossy().to_string());
    }
    roots
}

/// 规范化路径：统一分隔符、大小写（Windows）、解析 `..`，返回绝对路径。
fn normalize(p: &str) -> Option<PathBuf> {
    let p = p.trim().trim_matches('"');
    if p.is_empty() {
        return None;
    }
    let path = Path::new(p);
    // 相对路径基于当前工作目录展开
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };

    // 解析 `.` / `..` 与重复分隔符
    let mut normalized = PathBuf::new();
    for comp in absolute.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }

    // Windows 下统一大小写（磁盘路径不区分大小写）
    #[cfg(windows)]
    {
        let s = normalized.to_string_lossy().to_lowercase();
        return Some(PathBuf::from(s));
    }
    #[cfg(not(windows))]
    Some(normalized)
}

/// 解析真实路径（防 junction/symlink 绕过）—— 对「最长存在的祖先」做 canonicalize，
/// 不存在的尾部路径原样拼接后统一规范化；全部失败时回退 `normalize` 结果。
///
/// 注意：被检查路径（如工具参数中的目标文件）可能不存在，直接 canonicalize 会失败，
/// 因此逐级向上找存在的祖先再拼接（`C:\Windows\new.txt` → canonicalize(`C:\Windows`) + `new.txt`）。
pub fn resolve_real_path(p: &Path) -> PathBuf {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = p.to_path_buf();
    loop {
        match std::fs::canonicalize(&cur) {
            Ok(real) => {
                let mut out = real;
                for s in suffix.iter().rev() {
                    out.push(s);
                }
                // 统一大小写/分隔符，保证与授权根（normalize 结果）比较一致
                return normalize(&out.to_string_lossy()).unwrap_or(out);
            }
            Err(_) => {
                let Some(name) = cur.file_name() else { break };
                suffix.push(name.to_os_string());
                if !cur.pop() {
                    break;
                }
            }
        }
    }
    p.to_path_buf()
}

/// 判断 `path` 是否在 `root` 目录下（含等于）。
fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> SandboxManager {
        SandboxManager::new()
    }

    #[test]
    fn default_roots_include_cwd_and_home() {
        let s = sandbox();
        let roots = s.roots();
        assert!(!roots.is_empty());
        let cwd = std::env::current_dir().unwrap().to_string_lossy().to_lowercase();
        assert!(roots.iter().any(|r| r.to_lowercase() == cwd));
    }

    #[test]
    fn add_remove_root_roundtrip() {
        let s = sandbox();
        let tmp = std::env::temp_dir().join("clawdesk-sandbox-test");
        let tmp_s = tmp.to_string_lossy().to_string();
        assert!(s.add_root(&tmp_s));
        // 重复添加返回 false
        assert!(!s.add_root(&tmp_s));
        assert!(s.is_allowed(&tmp_s));
        assert!(s.remove_root(&tmp_s));
        assert!(!s.remove_root(&tmp_s));
    }

    #[test]
    fn child_path_is_allowed() {
        let s = sandbox();
        let tmp = std::env::temp_dir().join("clawdesk-sandbox-test");
        s.add_root(&tmp.to_string_lossy());
        let child = tmp.join("sub").join("file.txt").to_string_lossy().to_string();
        assert!(s.is_allowed(&child));
        // 大小写不敏感（Windows）
        #[cfg(windows)]
        assert!(s.is_allowed(&child.to_uppercase()));
    }

    #[test]
    fn outside_root_rejected() {
        let s = sandbox();
        // 构造一个不可能授权的根，验证其下子路径被拒
        let fake = PathBuf::from("Z:\\definitely\\not\\allowed\\sandbox");
        assert!(!s.is_allowed(&fake.to_string_lossy()));
    }

    /// ★ junction/symlink 绕过回归测试（2026-08-12）：
    /// 授权根内存在指向**根外**目录的 junction 时，经 junction 的路径必须被判定为根外。
    #[test]
    fn junction_bypass_is_blocked() {
        let s = sandbox();
        let base = std::env::temp_dir().join(format!("clawdesk-junc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("root"); // 授权根
        let outside = base.join("outside"); // 根外目录（junction 目标）
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let link = dir.join("link");
        // 创建 junction（Windows 目录链接；失败则跳过测试——非 Windows 无此概念）
        let created = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if created {
            // 清空默认授权根（temp/home 等默认根会覆盖测试目录导致判定失真），只留测试根
            for r in s.roots() {
                s.remove_root(&r);
            }
            assert!(s.add_root(dir.to_str().unwrap()));
            // 经 junction 访问根外目录：真实路径解析后应判定为根外 → 拒绝
            let escaped = link.join("secret.txt").to_string_lossy().to_string();
            assert!(
                !s.is_allowed(&escaped),
                "junction 指向根外目录，路径 `{escaped}` 不应被判定为在授权根内"
            );
            // 对照：根内普通路径仍放行（不被 junction 修复误伤）
            assert!(s.is_allowed(&dir.join("normal.txt").to_string_lossy()));
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dot_dot_normalized() {
        let s = sandbox();
        let tmp = std::env::temp_dir().join("clawdesk-sandbox-test");
        s.add_root(&tmp.to_string_lossy());
        let escaped = tmp
            .join("a")
            .join("..")
            .join("b")
            .to_string_lossy()
            .to_string();
        // a/../b → b，仍在根内
        assert!(s.is_allowed(&escaped));
    }

    #[tokio::test]
    async fn middleware_blocks_outside_path() {
        let s = Arc::new(SandboxManager::new());
        let m = SandboxMiddleware::new(s.clone());
        let def = UnifiedToolDef::new("builtin", "x", "x", vec![]).unwrap();
        let call = ToolCall {
            id: "c".into(),
            tool_id: "builtin:x".into(),
            arguments: serde_json::json!({ "path": "Z:\\out\\of\\sandbox\\file.txt" }),
            round: 1,
        };
        let err = m.before(&def, &call).await.unwrap_err();
        assert!(err.message.contains("沙箱隔离拦截"));
        assert!(err.message.contains("sandbox_add_root"));
    }

    #[tokio::test]
    async fn middleware_allows_inside_path() {
        let s = Arc::new(SandboxManager::new());
        let m = SandboxMiddleware::new(s.clone());
        let def = UnifiedToolDef::new("builtin", "x", "x", vec![]).unwrap();
        // 工作目录内路径
        let cwd_file = std::env::current_dir()
            .unwrap()
            .join("src")
            .join("main.rs")
            .to_string_lossy()
            .to_string();
        let call = ToolCall {
            id: "c".into(),
            tool_id: "builtin:x".into(),
            arguments: serde_json::json!({ "path": cwd_file }),
            round: 1,
        };
        assert!(m.before(&def, &call).await.is_ok());
    }

    #[tokio::test]
    async fn middleware_allows_no_path_args() {
        let s = Arc::new(SandboxManager::new());
        let m = SandboxMiddleware::new(s);
        let def = UnifiedToolDef::new("builtin", "x", "x", vec![]).unwrap();
        let call = ToolCall {
            id: "c".into(),
            tool_id: "builtin:x".into(),
            arguments: serde_json::json!({ "expression": "1+2" }),
            round: 1,
        };
        assert!(m.before(&def, &call).await.is_ok());
    }
}
