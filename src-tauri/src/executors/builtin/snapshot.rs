//! `builtin:snapshot_*` —— 快照索引与回滚工具集（项目 6，文档 §七.1 / §十一.3）。
//!
//! 设计说明：
//! - 文件修改前由 `file_write` 自动备份原文件到 `%APPDATA%/clawdesk/snapshots/`
//!   （`.bak`），本模块维护 **JSON 索引**（原路径 / 快照文件 / 时间戳 / 大小），
//!   使快照可查询、可回滚；
//! - 容量限制：`enforce_capacity` 按创建时间清理最旧快照（默认上限 100MB，
//!   超过阈值自动删除最早的 .bak 与索引项），清理记录回传模型避免读取失效路径；
//! - 工具：
//!   - `snapshot_list`：列出全部快照（含原路径 / 时间 / 大小）；
//!   - `snapshot_restore`：一键回滚单个文件（高危：覆盖当前文件，需确认）；
//!   - `snapshot_diff`：对比快照与当前文件的行级差异（模型自主读取对比变更）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

/// 默认快照容量上限（字节）：100MB。
pub const DEFAULT_CAPACITY_BYTES: u64 = 100 * 1024 * 1024;

/// 快照索引条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapEntry {
    pub id: String,
    pub original: String,
    pub snapshot: String,
    pub created_at: String,
    pub size: u64,
}

// ────────────────────────────────
// 索引存储
// ────────────────────────────────

/// 快照根目录覆盖层：生产默认 None（读 APPDATA）；测试设置唯一临时目录，
/// **避免 set_var 修改全局环境变量导致并行测试互相污染**（项目 6 修复）。
static ROOT_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// ★ 索引全局互斥锁（2026-08-12）：保护 load_index→修改→save_index 的读-改-写原子性，
///   防止并行工具调用（多个 file_write 并发备份）互相覆盖索引丢记录。
static INDEX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 快照根目录：`<数据目录>/snapshots/`（或测试覆盖值）。
pub fn snapshot_dir() -> PathBuf {
    if let Some(dir) = ROOT_OVERRIDE.read().unwrap().as_ref() {
        return dir.clone();
    }
    crate::llm::settings::clawdesk_dir().join("snapshots")
}

/// 设置快照根目录覆盖（仅测试使用；None 恢复默认）。
#[cfg(test)]
pub(crate) fn set_root_override(dir: Option<PathBuf>) {
    *ROOT_OVERRIDE.write().unwrap() = dir;
}

/// 快照相关测试串行锁：防止并行测试同时覆盖 ROOT_OVERRIDE 互相污染。
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap()
}

/// 索引文件路径。
fn index_path() -> PathBuf {
    snapshot_dir().join("index.json")
}

/// 读取索引（文件不存在返回空列表）。
pub fn load_index() -> Vec<SnapEntry> {
    match std::fs::read_to_string(index_path()) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// 保存索引。
fn save_index(entries: &[SnapEntry]) -> Result<(), String> {
    let dir = snapshot_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建快照目录失败: {}", e))?;
    let text = serde_json::to_string_pretty(entries).map_err(|e| format!("序列化索引失败: {}", e))?;
    std::fs::write(index_path(), text).map_err(|e| format!("写入索引失败: {}", e))
}

/// 追加一条快照记录（供 file_write 备份后调用）。
/// ★ 持锁执行 load→push→save（防并行调用丢记录）。
pub fn record_snapshot(original: &str, snapshot_file: &str, size: u64) -> Result<(), String> {
    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = load_index();
    entries.push(SnapEntry {
        id: format!("snap_{}", chrono::Local::now().format("%Y%m%d_%H%M%S%3f")),
        original: original.to_string(),
        snapshot: snapshot_file.to_string(),
        // 毫秒级精度（RFC3339 秒级会导致同秒内容量清理排序不稳定）
        created_at: chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string(),
        size,
    });
    save_index(&entries)
}

/// 容量清理：超过上限时按创建顺序删除最旧快照（文件 + 索引项）。
///
/// 返回被清理的条目（供调用方回传模型，避免模型读取已失效路径）。
/// ★ 持锁执行 load→修改→save。
pub fn enforce_capacity(max_bytes: u64) -> Vec<SnapEntry> {
    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = load_index();
    let mut removed = Vec::new();
    let total: u64 = entries.iter().map(|e| e.size).sum();

    if total <= max_bytes {
        return removed;
    }

    // 按创建时间升序（最旧在前）清理
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut current = total;
    let mut keep = Vec::new();
    for e in entries {
        if current > max_bytes {
            // 删除快照文件（失败也继续，索引一并移除）
            let _ = std::fs::remove_file(&e.snapshot);
            current = current.saturating_sub(e.size);
            removed.push(e);
        } else {
            keep.push(e);
        }
    }
    let _ = save_index(&keep);
    removed
}

/// 按原路径列出快照（按创建时间倒序，最新在前）。
pub fn list_snapshots() -> Vec<SnapEntry> {
    let mut entries = load_index();
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    entries
}

/// 根据 id 查找快照条目。
pub fn find_entry(id: &str) -> Option<SnapEntry> {
    load_index().into_iter().find(|e| e.id == id)
}

/// 移除索引中的指定条目（回滚后保留快照，便于再次回滚；删除由用户显式进行）。
/// ★ 持锁执行 load→过滤→save。
fn remove_entry(id: &str) {
    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let entries: Vec<SnapEntry> = load_index().into_iter().filter(|e| e.id != id).collect();
    let _ = save_index(&entries);
}

// ────────────────────────────────
// 回滚与对比
// ────────────────────────────────

/// 回滚：将快照文件内容写回原路径（覆盖当前文件）。
/// 返回是否成功 + 恢复后的字节数。
pub fn restore_snapshot(id: &str) -> Result<serde_json::Value, String> {
    let entry = find_entry(id).ok_or_else(|| format!("快照不存在: {}", id))?;
    let snap_path = Path::new(&entry.snapshot);
    if !snap_path.exists() {
        return Err(format!("快照文件已丢失: {}", entry.snapshot));
    }
    let bytes = std::fs::read(snap_path).map_err(|e| format!("读取快照失败: {}", e))?;
    std::fs::write(&entry.original, &bytes).map_err(|e| format!("回滚写入失败: {}", e))?;
    Ok(json!({
        "id": entry.id,
        "original": entry.original,
        "restoredBytes": bytes.len(),
        "createdAt": entry.created_at,
    }))
}

/// 删除一条快照（文件 + 索引项）。
pub fn delete_snapshot(id: &str) -> Result<bool, String> {
    let entry = find_entry(id).ok_or_else(|| format!("快照不存在: {}", id))?;
    let _ = std::fs::remove_file(&entry.snapshot);
    remove_entry(id);
    Ok(true)
}

/// 行级差异：对比快照内容与当前文件内容。
/// 返回完整差异列表（统一 diff 风格）+ 摘要统计；内容超长时截断。
pub fn diff_snapshot(id: &str) -> Result<serde_json::Value, String> {
    let entry = find_entry(id).ok_or_else(|| format!("快照不存在: {}", id))?;
    let snap_content = std::fs::read_to_string(&entry.snapshot)
        .map_err(|e| format!("读取快照失败: {}", e))?;
    let current_content = match std::fs::read_to_string(&entry.original) {
        Ok(c) => c,
        Err(_) => String::new(),
    };

    let snap_lines: Vec<&str> = snap_content.lines().collect();
    let curr_lines: Vec<&str> = current_content.lines().collect();

    // 简化行级 diff（逐行对比，标记 +/-）
    let mut diff: Vec<String> = Vec::new();
    let max_lines = snap_lines.len().max(curr_lines.len());
    for i in 0..max_lines {
        let s = snap_lines.get(i).copied().unwrap_or("");
        let c = curr_lines.get(i).copied().unwrap_or("");
        if s != c {
            if i < snap_lines.len() {
                diff.push(format!("- {}: {}", i + 1, s));
            }
            if i < curr_lines.len() {
                diff.push(format!("+ {}: {}", i + 1, c));
            }
        }
    }

    let total_diff = diff.len();
    // 截断防上下文爆炸（保留前 100 行 + 尾部统计）
    let truncated = diff.len() > 100;
    diff.truncate(100);

    Ok(json!({
        "id": entry.id,
        "original": entry.original,
        "snapshotLines": snap_lines.len(),
        "currentLines": curr_lines.len(),
        "diffCount": total_diff,
        "truncated": truncated,
        "diff": diff,
    }))
}

// ────────────────────────────────
// 工具注册
// ────────────────────────────────

/// 注册 snapshot_* 工具（在 builtin::register_all 中调用）。
pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    register_list(registry)?;
    register_restore(registry)?;
    register_diff(registry)?;
    Ok(())
}

fn register_list(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "snapshot_list",
        "列出全部文件修改快照（原路径/时间/大小），用于查看可回滚的历史版本",
        vec![],
    )?;
    let handler: ToolHandler = Arc::new(|_args, _ctx| {
        Box::pin(async move {
            let entries = list_snapshots();
            Ok(ToolResult::ok(json!({
                "count": entries.len(),
                "snapshots": entries,
            })))
        })
    });
    registry.register(def, handler)
}

fn register_restore(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "snapshot_restore",
        "将指定快照回滚到原文件（覆盖当前内容），用于撤销文件修改",
        vec![ToolParamDef {
            name: "snapshot_id".into(),
            param_type: "string".into(),
            description: "快照 ID（snapshot_list 返回）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?
    .high_risk(); // 回滚覆盖文件属高危：需用户确认

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let id = args.get("snapshot_id").and_then(|v| v.as_str()).unwrap_or_default();
            if id.is_empty() {
                return Ok(ToolResult::err("snapshot_id 不能为空"));
            }
            match restore_snapshot(id) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("回滚失败: {}", e))),
            }
        })
    });
    registry.register(def, handler)
}

fn register_diff(registry: &ToolRegistry) -> Result<(), ToolError> {
    let def = UnifiedToolDef::new(
        "builtin",
        "snapshot_diff",
        "对比快照与当前文件的行级差异，用于审查文件修改内容",
        vec![ToolParamDef {
            name: "snapshot_id".into(),
            param_type: "string".into(),
            description: "快照 ID（snapshot_list 返回）".into(),
            required: true,
            enum_values: None,
            default: None,
        }],
    )?;

    let handler: ToolHandler = Arc::new(|args, _ctx| {
        Box::pin(async move {
            let id = args.get("snapshot_id").and_then(|v| v.as_str()).unwrap_or_default();
            if id.is_empty() {
                return Ok(ToolResult::err("snapshot_id 不能为空"));
            }
            match diff_snapshot(id) {
                Ok(v) => Ok(ToolResult::ok(v)),
                Err(e) => Ok(ToolResult::err(format!("对比失败: {}", e))),
            }
        })
    });
    registry.register(def, handler)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试专用：把快照目录指向唯一临时目录（覆盖层 + 串行锁，避免并行污染）。
    fn with_temp_snapshot_dir<T>(f: impl FnOnce() -> T) -> T {
        let _guard = test_lock();
        let dir = std::env::temp_dir().join(format!("clawdesk-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        set_root_override(Some(dir.clone()));
        let result = f();
        set_root_override(None);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn record_and_list_snapshot() {
        with_temp_snapshot_dir(|| {
            let dir = std::env::temp_dir().join("clawdesk-snap-src");
            std::fs::create_dir_all(&dir).unwrap();
            let orig = dir.join("a.txt");
            std::fs::write(&orig, "v1").unwrap();
            let snap = snapshot_dir().join("snap1.bak");
            std::fs::write(&snap, "v1").unwrap();

            record_snapshot(orig.to_str().unwrap(), snap.to_str().unwrap(), 2).unwrap();
            let list = list_snapshots();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].original, orig.to_str().unwrap());
            assert_eq!(list[0].size, 2);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn restore_overwrites_original() {
        with_temp_snapshot_dir(|| {
            let dir = std::env::temp_dir().join("clawdesk-snap-src2");
            std::fs::create_dir_all(&dir).unwrap();
            let orig = dir.join("b.txt");
            std::fs::write(&orig, "current").unwrap();
            let snap = snapshot_dir().join("snap2.bak");
            std::fs::write(&snap, "old-content").unwrap();

            record_snapshot(orig.to_str().unwrap(), snap.to_str().unwrap(), 11).unwrap();
            let id = list_snapshots()[0].id.clone();
            let out = restore_snapshot(&id).unwrap();
            assert_eq!(out["restoredBytes"], 11);
            assert_eq!(std::fs::read_to_string(&orig).unwrap(), "old-content");
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn diff_reports_changes() {
        with_temp_snapshot_dir(|| {
            let dir = std::env::temp_dir().join("clawdesk-snap-src3");
            std::fs::create_dir_all(&dir).unwrap();
            let orig = dir.join("c.txt");
            std::fs::write(&orig, "line1\nchanged\nline3").unwrap();
            let snap = snapshot_dir().join("snap3.bak");
            std::fs::write(&snap, "line1\nline2\nline3").unwrap();

            record_snapshot(orig.to_str().unwrap(), snap.to_str().unwrap(), 17).unwrap();
            let id = list_snapshots()[0].id.clone();
            let out = diff_snapshot(&id).unwrap();
            assert!(out["diffCount"].as_u64().unwrap() >= 1);
            assert!(out["diff"].as_array().unwrap().iter().any(|d| {
                let s = d.as_str().unwrap();
                s.contains("line2") || s.contains("changed")
            }));
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn enforce_capacity_removes_oldest() {
        with_temp_snapshot_dir(|| {
            let dir = std::env::temp_dir().join("clawdesk-snap-src4");
            std::fs::create_dir_all(&dir).unwrap();
            let orig = dir.join("d.txt");

            // 创建两条快照（各 10 字节），容量上限 15 → 清理 1 条
            let snap1 = snapshot_dir().join("old.bak");
            std::fs::write(&snap1, "0123456789").unwrap();
            record_snapshot(orig.to_str().unwrap(), snap1.to_str().unwrap(), 10).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));

            let snap2 = snapshot_dir().join("new.bak");
            std::fs::write(&snap2, "abcdefghij").unwrap();
            record_snapshot(orig.to_str().unwrap(), snap2.to_str().unwrap(), 10).unwrap();

            let removed = enforce_capacity(15);
            assert_eq!(removed.len(), 1);
            assert_eq!(removed[0].snapshot, snap1.to_str().unwrap());
            assert_eq!(list_snapshots().len(), 1);
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    #[test]
    fn delete_removes_snapshot() {
        with_temp_snapshot_dir(|| {
            let dir = std::env::temp_dir().join("clawdesk-snap-src5");
            std::fs::create_dir_all(&dir).unwrap();
            let orig = dir.join("e.txt");
            let snap = snapshot_dir().join("snap5.bak");
            std::fs::write(&snap, "x").unwrap();
            record_snapshot(orig.to_str().unwrap(), snap.to_str().unwrap(), 1).unwrap();
            let id = list_snapshots()[0].id.clone();
            assert!(delete_snapshot(&id).unwrap());
            assert!(list_snapshots().is_empty());
            assert!(!Path::new(&snap).exists());
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
