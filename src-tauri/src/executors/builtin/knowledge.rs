//! `builtin:knowledge` —— 本地知识库 RAG（对标大厂 Agent 的知识管理能力）。
//!
//! 把文档（TXT/MD/代码等）索引到 **SQLite 持久化** 知识库，重启不丢。
//! 技术：文本分块 → TF-IDF 关键词提取 → LIKE 全表扫描检索（10 万块内够用）。

use std::collections::HashMap;

use rusqlite::Connection;
use serde_json::json;
use text_splitter::TextSplitter;

use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};
use crate::core::tool::error::ToolError;
use crate::core::tool::registry::{ToolHandler, ToolRegistry};
use crate::core::tool::result::ToolResult;

fn kb_db_path() -> std::path::PathBuf {
    crate::llm::settings::clawdesk_dir().join("knowledge.db")
}

fn open_db() -> Result<Connection, String> {
    let path = kb_db_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建知识库目录失败: {e}"))?;
    }
    let conn = Connection::open(&path).map_err(|e| format!("打开知识库失败: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file TEXT NOT NULL,
            chunk_idx INTEGER NOT NULL DEFAULT 0,
            text TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chunks_file ON chunks(file);"
    ).map_err(|e| format!("建表失败: {e}"))?;
    Ok(conn)
}

pub fn register(registry: &ToolRegistry) -> Result<(), ToolError> {
    // ── knowledge_index ──
    let def_idx = UnifiedToolDef::new(
        "builtin", "knowledge_index",
        "索引文件到本地知识库（SQLite 持久化，重启不丢）。支持 TXT/MD/PY/JS 等 30+ 格式。",
        vec![ToolParamDef {
            name: "path".into(), param_type: "string".into(),
            description: "文件或目录路径".into(), required: true, enum_values: None, default: None,
        }],
    )?;
    let handler_idx: ToolHandler = std::sync::Arc::new(|args, _ctx| {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if path.is_empty() { return Box::pin(async { Ok(ToolResult::err("path 不能为空")) }); }
        Box::pin(async move {
            match do_index(&path) { Ok(v) => Ok(ToolResult::ok(v)), Err(e) => Ok(ToolResult::err(format!("索引失败: {e}"))) }
        })
    });
    registry.register(def_idx, handler_idx)?;

    // ── knowledge_search ──
    let def_search = UnifiedToolDef::new(
        "builtin", "knowledge_search",
        "搜索本地知识库，返回最相关文档片段（TF-IDF，Top-8）。",
        vec![
            ToolParamDef { name: "query".into(), param_type: "string".into(), description: "搜索关键词/问题".into(), required: true, enum_values: None, default: None },
            ToolParamDef { name: "top".into(), param_type: "number".into(), description: "返回条数（默认8）".into(), required: false, enum_values: None, default: Some(json!(8)) },
        ],
    )?;
    let handler_search: ToolHandler = std::sync::Arc::new(|args, _ctx| {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(8).min(20) as usize;
        if query.is_empty() { return Box::pin(async { Ok(ToolResult::err("query 不能为空")) }); }
        Box::pin(async move {
            match do_search(&query, top) { Ok(v) => Ok(ToolResult::ok(v)), Err(e) => Ok(ToolResult::err(format!("搜索失败: {e}"))) }
        })
    });
    registry.register(def_search, handler_search)?;

    // ── knowledge_stats ──
    let def_stats = UnifiedToolDef::new("builtin", "knowledge_stats", "查看知识库统计", vec![])?;
    registry.register(def_stats, std::sync::Arc::new(|_, _| Box::pin(async { Ok(ToolResult::ok(do_stats())) })))?;

    // ── knowledge_clear ──
    let def_clear = UnifiedToolDef::new("builtin", "knowledge_clear", "清空知识库（不可逆）", vec![])?;
    registry.register(def_clear, std::sync::Arc::new(|_, _| Box::pin(async move {
        match do_clear() { Ok(v) => Ok(ToolResult::ok(v)), Err(e) => Ok(ToolResult::err(format!("清空失败: {e}"))) }
    })))?;

    eprintln!("[KNOWLEDGE] 知识库已启用 (SQLite: {})", kb_db_path().display());
    Ok(())
}

// ═══════════════════════════════════════════
// 索引（SQLite 持久化）
// ═══════════════════════════════════════════

fn do_index(path_str: &str) -> Result<serde_json::Value, String> {
    let p = std::path::Path::new(path_str);
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut truncated = false;
    if p.is_dir() {
        collect_text_files(p, &mut files, 0)?;
        // ★ 2026-08-12：数量上限截断（防超大目录一次性索引失控）
        if files.len() > MAX_INDEX_FILES {
            files.truncate(MAX_INDEX_FILES);
            truncated = true;
        }
    } else {
        files.push(p.to_path_buf());
    }

    let conn = open_db()?;
    let splitter = TextSplitter::new(600);
    let mut added = 0u64;
    let mut skipped = 0u64;

    for f in &files {
        let fname = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        conn.execute("DELETE FROM chunks WHERE file = ?1", rusqlite::params![fname])
            .map_err(|e| format!("清除旧索引失败: {e}"))?;
    }

    for f in &files {
        let text = match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(_) => { skipped += 1; continue; }
        };
        let fname = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        let chunks: Vec<String> = splitter.chunks(&text).map(|c| c.to_string()).collect();

        for (i, chunk) in chunks.into_iter().enumerate() {
            if chunk.trim().is_empty() { continue; }
            conn.execute(
                "INSERT INTO chunks (file, chunk_idx, text) VALUES (?1, ?2, ?3)",
                rusqlite::params![fname, i as i64, chunk],
            ).map_err(|e| format!("写入失败: {e}"))?;
            added += 1;
        }
    }

    let total: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)).unwrap_or(0);
    let mut resp = json!({
        "indexedFiles": files.len() as u64 - skipped,
        "skippedFiles": skipped,
        "addedChunks": added,
        "totalChunks": total,
        "persisted": true,
    });
    if truncated {
        resp["note"] = json!(format!(
            "目录文件数超过上限 {}，仅索引前 {} 个（结果可能不完整）",
            MAX_INDEX_FILES, MAX_INDEX_FILES
        ));
    }
    Ok(resp)
}

// ═══════════════════════════════════════════
// 搜索（LIKE 全表扫描 + TF-IDF 重排，10 万块内够用）
// ═══════════════════════════════════════════

fn do_search(query: &str, top: usize) -> Result<serde_json::Value, String> {
    let conn = open_db()?;
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)).unwrap_or(0);
    if total == 0 {
        return Ok(json!({"results":[],"hint":"知识库为空，请先用 knowledge_index 索引文件","totalChunks":0}));
    }
    let query_words = tokenize_words(query);
    if query_words.is_empty() {
        return Ok(json!({"results":[],"hint":"未提取到有效关键词"}));
    }
    let like_clauses: Vec<String> = query_words.iter()
        .map(|w| format!("text LIKE '%{}%'", w.replace('\'', "''").replace('%', r"\%")))
        .collect();
    let sql = format!("SELECT id, file, chunk_idx, text FROM chunks WHERE {} LIMIT 500", like_clauses.join(" OR "));
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("查询失败: {e}"))?;
    let rows: Vec<(i64, String, i64, String)> = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .map_err(|e| format!("读取结果失败: {e}"))?.filter_map(|r| r.ok()).collect();

    let mut df_map: HashMap<String, usize> = HashMap::new();
    for (_id, _file, _idx, text) in &rows {
        for w in std::collections::HashSet::<String>::from_iter(tokenize_words(text)) {
            *df_map.entry(w).or_insert(0) += 1;
        }
    }
    let q_len = query_words.len().max(1) as f64;
    let mut query_tf: HashMap<String, f64> = HashMap::new();
    for w in &query_words { *query_tf.entry(w.clone()).or_insert(0.0) += 1.0; }
    for v in query_tf.values_mut() { *v /= q_len; }

    let mut scores: Vec<(i64, String, i64, String, f64)> = Vec::new();
    for (id, file, idx, text) in &rows {
        let words = tokenize_words(text);
        let w_len = words.len().max(1) as f64;
        let mut tf: HashMap<String, f64> = HashMap::new();
        for w in words { *tf.entry(w).or_insert(0.0) += 1.0; }
        for v in tf.values_mut() { *v /= w_len; }
        let mut score = 0.0;
        for w in &query_words {
            let t = tf.get(w).copied().unwrap_or(0.0);
            let d = df_map.get(w).copied().unwrap_or(0).max(1) as f64;
            let idf = (total as f64 / d).ln() + 1.0;
            let q_tf = query_tf.get(w).copied().unwrap_or(0.0);
            score += t * idf * q_tf;
        }
        if score > 0.0 { scores.push((*id, file.clone(), *idx, text.clone(), score)); }
    }
    scores.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    scores.truncate(top);
    let results: Vec<serde_json::Value> = scores.into_iter().map(|(id, file, _idx, text, s)| {
        json!({"id":id,"file":file,"score":format!("{:.4}",s),"text":text.chars().take(800).collect::<String>()})
    }).collect();
    Ok(json!({"results":results,"query":query,"totalChunks":total}))
}

// ═══════════════════════════════════════════
// 统计 · 清空
// ═══════════════════════════════════════════

fn do_stats() -> serde_json::Value {
    let conn = match open_db() { Ok(c) => c, Err(e) => return json!({"error": e}) };
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)).unwrap_or(0);
    let files_count: i64 = conn.query_row("SELECT COUNT(DISTINCT file) FROM chunks", [], |r| r.get(0)).unwrap_or(0);
    let mut stmt = match conn.prepare("SELECT file, COUNT(*) as cnt FROM chunks GROUP BY file ORDER BY cnt DESC LIMIT 30") {
        Ok(s) => s,
        Err(_) => return json!({"totalChunks": total, "totalFiles": files_count, "persisted": true, "dbPath": kb_db_path().to_string_lossy().to_string()}),
    };
    let files: Vec<serde_json::Value> = stmt.query_map([], |row| {
        let f: String = row.get(0)?; let n: i64 = row.get(1)?;
        Ok(json!({"file": f, "chunks": n}))
    }).unwrap().filter_map(|r| r.ok()).collect();
    json!({"totalChunks":total,"totalFiles":files_count,"persisted":true,"dbPath":kb_db_path().to_string_lossy().to_string(),"files":files})
}

fn do_clear() -> Result<serde_json::Value, String> {
    let conn = open_db()?;
    let before: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)).unwrap_or(0);
    conn.execute("DELETE FROM chunks", []).map_err(|e| format!("清空失败: {e}"))?;
    Ok(json!({"ok":true,"deletedChunks":before,"message":format!("已清空 {} 个文档块", before)}))
}

// ═══════════════════════════════════════════
// 辅助
// ═══════════════════════════════════════════

fn tokenize_words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 2).map(|w| w.to_string()).collect()
}

/// ★ 2026-08-12：单次索引文件数 / 递归深度上限（防超大目录遍历失控）。
const MAX_INDEX_FILES: usize = 2000;
const MAX_INDEX_DEPTH: u32 = 12;

fn collect_text_files(
    dir: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
    depth: u32,
) -> Result<(), String> {
    if depth > MAX_INDEX_DEPTH || files.len() >= MAX_INDEX_FILES {
        return Ok(());
    }
    let text_exts = ["txt", "md", "py", "js", "ts", "jsx", "tsx", "json", "csv",
                      "html", "css", "xml", "yaml", "yml", "toml", "rs", "go", "java",
                      "c", "cpp", "h", "hpp", "vue", "svelte", "sql", "sh", "bat", "ps1"];
    for entry in std::fs::read_dir(dir).map_err(|e| format!("读取目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("读取条目失败: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            let _ = collect_text_files(&path, files, depth + 1);
        } else if let Some(ext) = path.extension() {
            if text_exts.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
                if path.metadata().map(|m| m.len() < 5_000_000).unwrap_or(false) {
                    files.push(path);
                }
            }
        }
        if files.len() >= MAX_INDEX_FILES {
            break;
        }
    }
    Ok(())
}
