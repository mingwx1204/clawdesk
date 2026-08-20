//! Agent 会话管理 —— 多轮对话记忆 + SQLite 持久化 + 上下文压缩。
//!
//! 设计：
//! - 会话保存完整消息历史，跨 `agent_chat` 请求保留（按会话 ID）；
//! - **SQLite 持久化**：所有会话写入同步落盘，重启软件状态不丢失；
//! - 上下文压缩策略：消息数 / 字符数超过阈值时，调用方（runner 层）
//!   用 LLM 生成历史摘要，再经 `compact_with` 应用；
//! - 纯内存模式（`new()`）供离线测试使用。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::LlmMessage;

/// 单个 Agent 会话。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    /// 用户自定义会话名称（None = 未命名，前端显示 id）。
    #[serde(default)]
    pub name: Option<String>,
    /// 完整消息历史（含 system 摘要消息）。
    pub messages: Vec<LlmMessage>,
    pub created_at: String,
    /// 已发生压缩次数。
    pub compacted_count: usize,
    /// Fork 父会话 ID（None = 主会话；Some = 分支会话，§十二.2）。
    #[serde(default)]
    pub parent_id: Option<String>,
    /// 会话累计输入 token（真实记录，上下文占用面板数据源）。
    #[serde(default)]
    pub total_input_tokens: u64,
    /// 会话累计输出 token。
    #[serde(default)]
    pub total_output_tokens: u64,
    /// 最近一次请求的输入 token 数（≈ 当前上下文窗口占用）。
    #[serde(default)]
    pub last_input_tokens: u64,
}

/// 任务断点（§十二.1）：ReAct 任务中断时保存，用于从断点续跑。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCheckpoint {
    pub session_id: String,
    /// 已迭代轮数（续跑时从该轮继续）。
    pub round: usize,
    /// 待执行子任务（规划步骤）。
    pub plan_steps: Vec<String>,
    /// 已执行工具摘要（供续跑模型知晓进度）。
    pub tool_history: Vec<String>,
    /// 当前上下文摘要。
    pub summary: String,
    pub created_at: String,
}

/// 会话管理器（应用级共享状态，线程安全）。
///
/// 存储层级：内存 HashMap（读快）+ SQLite（持久化）。
/// 写路径：内存与磁盘同步更新，保证重启恢复一致。
pub struct SessionManager {
    sessions: RwLock<HashMap<String, AgentSession>>,
    /// SQLite 连接（None = 纯内存模式，测试用）。
    db: Mutex<Option<Connection>>,
    /// 触发压缩的消息数阈值。
    #[allow(dead_code)]
    max_messages: usize,
    /// 触发压缩的总字符数阈值。
    #[allow(dead_code)]
    max_chars: usize,
    /// 压缩时保留的最近消息条数。
    #[allow(dead_code)]
    keep_last: usize,
}

/// 移除"悬空"的 tool 消息（双向配对清理，LlmMessage 版）。
///
/// 压缩/截断后可能破坏 tool 配对组，DeepSeek 会报 HTTP 400，两种形态：
/// 1. 悬空 tool：前置 assistant(tool_calls) 被截掉 / 段首就是 tool 消息 → 丢弃该 tool；
/// 2. assistant 带 tool_calls 但其部分/全部 tool 响应被截掉 → 整组删除。
/// （当前 compact_with 在 lib 构建中未被接线，随其一起豁免 dead_code；测试仍覆盖。）
#[allow(dead_code)]
fn prune_dangling_tool_messages(messages: &mut Vec<LlmMessage>) {
    use super::Role;
    let old = std::mem::take(messages);
    let mut out: Vec<LlmMessage> = Vec::with_capacity(old.len());
    let mut i = 0usize;
    while i < old.len() {
        // 悬空 tool：无前置 assistant(tool_calls) 支撑 → 丢弃
        if matches!(old[i].role, Role::Tool) {
            i += 1;
            continue;
        }
        // assistant 带 tool_calls：检查其 tool 响应是否完整
        if matches!(old[i].role, Role::Assistant) && old[i].tool_calls.is_some() {
            let declared: Vec<String> = old[i]
                .tool_calls
                .as_ref()
                .map(|tcs| tcs.iter().map(|tc| tc.id.clone()).collect())
                .unwrap_or_default();
            // 向后收集紧接着的 tool 响应（直到下一条非 tool 消息）
            let mut j = i + 1;
            let mut responded: Vec<String> = Vec::new();
            while j < old.len() && matches!(old[j].role, Role::Tool) {
                if let Some(id) = &old[j].tool_call_id {
                    responded.push(id.clone());
                }
                j += 1;
            }
            // 每个声明的 tool_call_id 都必须有响应，否则整组删除 assistant（含残留响应）
            let complete = declared.iter().all(|id| responded.contains(id));
            if complete {
                out.push(old[i].clone());
                out.extend_from_slice(&old[i + 1..j]);
            }
            i = j;
            continue;
        }
        out.push(old[i].clone());
        i += 1;
    }
    *messages = out;
}

impl SessionManager {
    /// 纯内存模式（离线测试 / 无持久化需求）。
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            db: Mutex::new(None),
            max_messages: 30,
            max_chars: 12000,
            keep_last: 10,
        }
    }

    /// 带 SQLite 持久化：打开（或创建）数据库并加载已有会话。
    ///
    /// 说明：`path` 通常为 `app.path().app_data_dir()/sessions.db`。
    #[allow(dead_code)] // 测试专用（生产用 attach_db 附加持久化）
    pub fn new_with_db(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("打开会话数据库失败: {}", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS checkpoints (
                session_id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                created_at TEXT NOT NULL
            );",
        )
        .map_err(|e| format!("初始化会话表失败: {}", e))?;

        // 加载已有会话到内存
        let mut sessions = HashMap::new();
        let mut stmt = conn
            .prepare("SELECT data FROM sessions")
            .map_err(|e| format!("读取会话失败: {}", e))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询会话失败: {}", e))?;
        for row in rows {
            if let Ok(json) = row {
                if let Ok(session) = serde_json::from_str::<AgentSession>(&json) {
                    sessions.insert(session.id.clone(), session);
                }
            }
        }
        drop(stmt);

        Ok(Self {
            sessions: RwLock::new(sessions),
            db: Mutex::new(Some(conn)),
            max_messages: 30,
            max_chars: 12000,
            keep_last: 10,
        })
    }

    /// 创建新会话（内存 + 磁盘）。
        pub fn create(&self, id: String) -> AgentSession {
        self.create_with_parent(id, None)
    }

    pub fn create_with_parent(&self, id: String, parent_id: Option<String>) -> AgentSession {
        let session = AgentSession {
            id: id.clone(),
            name: None,
            messages: Vec::new(),
            created_at: chrono::Local::now().to_rfc3339(),
            compacted_count: 0,
            parent_id,
            total_input_tokens: 0,
            total_output_tokens: 0,
            last_input_tokens: 0,
        };
        self.sessions
            .write()
            .unwrap()
            .insert(id.clone(), session.clone());
        self.persist(&session);
        session
    }

    /// 按 ID 获取会话（不存在则创建）。
    pub fn get_or_create(&self, id: &str) -> AgentSession {
        // 注意：先读并立即释放读锁（let 语句），再决定是否 create（写锁），
        // 避免同一线程 read guard 存活期间获取 write 导致 RwLock 死锁。
        let existing = self.sessions.read().unwrap().get(id).cloned();
        match existing {
            Some(s) => s,
            None => self.create(id.to_string()),
        }
    }

    /// 动态附加 SQLite 持久化（应用 setup 阶段调用）。
    ///
    /// 说明：AppState 先以内存模式创建，应用启动拿到数据目录后再
    /// 附加数据库 —— 避免重建 Arc<SessionManager> 导致引用失效。
    #[allow(dead_code)] // 由 lib.rs setup 调用，避免未使用告警（AppState 用法）
    pub fn attach_db(&self, path: &Path) -> Result<(), String> {
        let conn = Connection::open(path).map_err(|e| format!("打开会话数据库失败: {}", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS checkpoints (
                session_id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                created_at TEXT NOT NULL
            );",
        )
        .map_err(|e| format!("初始化会话表失败: {}", e))?;

        let mut stmt = conn
            .prepare("SELECT data FROM sessions")
            .map_err(|e| format!("读取会话失败: {}", e))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("查询会话失败: {}", e))?;
        {
            let mut sessions = self.sessions.write().unwrap();
            for row in rows {
                if let Ok(json) = row {
                    if let Ok(session) = serde_json::from_str::<AgentSession>(&json) {
                        sessions.entry(session.id.clone()).or_insert(session);
                    }
                }
            }
        }
        drop(stmt);

        *self.db.lock().unwrap() = Some(conn);
        Ok(())
    }

    /// 更新会话（内存 + 磁盘）。
    pub fn update(&self, session: AgentSession) {
        self.sessions
            .write()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        self.persist(&session);
    }

    /// 清空会话上下文：保留会话本身，删除全部消息并把窗口占用归零。
    /// 累计 token 统计保留（右侧面板的「累计用量」是历史事实，不清零）。
    pub fn clear_context(&self, id: &str) -> bool {
        let mut session = self.get_or_create(id);
        session.messages.clear();
        session.last_input_tokens = 0;
        self.update(session);
        true
    }

    /// 删除会话（内存 + 磁盘）。
    pub fn delete(&self, id: &str) -> Option<AgentSession> {
        let removed = self.sessions.write().unwrap().remove(id);
        if let Some(db) = self.db.lock().unwrap().as_ref() {
            let _ = db.execute("DELETE FROM sessions WHERE id = ?1", [id]);
        }
        removed
    }

    /// 列出全部会话 ID。
    pub fn list(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.sessions.read().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// 判断是否触发压缩（消息数或字符数超阈值）。
    #[allow(dead_code)]
    pub fn needs_compaction(&self, session: &AgentSession) -> bool {
        let total_chars: usize = session
            .messages
            .iter()
            .map(|m| m.content.len())
            .sum();
        session.messages.len() > self.max_messages || total_chars > self.max_chars
    }

    /// 应用压缩：用摘要替换历史（保留最近 `keep_last` 条原始消息）。
    ///
    /// ★ 2026-08-12 修复：截断后清理首尾不配对的 tool / assistant(tool_calls) 消息
    ///   （保留窗口可能以 tool 消息开头、或前置 assistant(tool_calls) 被截掉 →
    ///   DeepSeek HTTP 400 "role 'tool' must be a response to ... 'tool_calls'"）。
    ///   实现思路移植自 harness::engine::context::prune_dangling_tools（Value 版）。
    #[allow(dead_code)]
    pub fn compact_with(&self, session: &mut AgentSession, summary: String) {
        let keep = self.keep_last.min(session.messages.len());
        let kept = session.messages.split_off(session.messages.len() - keep);

        let mut messages = vec![LlmMessage {
            role: super::Role::System,
            content: format!("【历史对话摘要】{}", summary),
            tool_calls: None,
            tool_call_id: None,
        }];
        messages.extend(kept);
        prune_dangling_tool_messages(&mut messages);
        session.messages = messages;
        session.compacted_count += 1;
    }

    /// 当前会话数量。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.sessions.read().unwrap().len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 跨会话关键词检索历史消息（供 memory_search 工具使用）。
    ///
    /// 返回最多 20 条命中：sessionId / role / content 摘要。
    pub fn search(&self, keyword: &str) -> Vec<serde_json::Value> {
        let kw = keyword.to_lowercase();
        let mut hits = Vec::new();
        let sessions = self.sessions.read().unwrap();
        'outer: for s in sessions.values() {
            for m in &s.messages {
                if m.content.to_lowercase().contains(&kw) {
                    let content = crate::llm::truncate(&m.content, 300);
                    hits.push(serde_json::json!({
                        "sessionId": s.id,
                        "role": format!("{:?}", m.role),
                        "content": content,
                    }));
                    if hits.len() >= 20 {
                        break 'outer;
                    }
                }
            }
        }
        hits
    }

    /// 将会话写入 SQLite（UPSERT）。
    #[allow(dead_code)]
    pub fn save_checkpoint(&self, cp: &AgentCheckpoint) {
        if let Some(db) = self.db.lock().unwrap().as_ref() {
            let json = serde_json::to_string(cp).unwrap_or_default();
            let _ = db.execute(
                "INSERT INTO checkpoints (session_id, data, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET data = ?2, created_at = ?3",
                rusqlite::params![cp.session_id, json, chrono::Local::now().to_rfc3339()],
            );
        }
    }

    pub fn load_checkpoint(&self, session_id: &str) -> Option<AgentCheckpoint> {
        let db = self.db.lock().unwrap();
        let conn = db.as_ref()?;
        let mut stmt = conn.prepare("SELECT data FROM checkpoints WHERE session_id = ?1").ok()?;
        let mut rows = stmt.query_map([session_id], |row| row.get::<_, String>(0)).ok()?;
        if let Some(Ok(json)) = rows.next() {
            return serde_json::from_str(&json).ok();
        }
        None
    }

    #[allow(dead_code)]
    pub fn clear_checkpoint(&self, session_id: &str) {
        if let Some(db) = self.db.lock().unwrap().as_ref() {
            let _ = db.execute("DELETE FROM checkpoints WHERE session_id = ?1", [session_id]);
        }
    }


    pub fn fork(&self, source_id: &str, new_id: String) -> Option<AgentSession> {
        let source = self.sessions.read().unwrap().get(source_id).cloned()?;
        let mut branch = source.clone();
        branch.id = new_id.clone();
        branch.created_at = chrono::Local::now().to_rfc3339();
        branch.parent_id = Some(source_id.to_string());
        self.sessions
            .write()
            .unwrap()
            .insert(new_id.clone(), branch.clone());
        self.persist(&branch);
        Some(branch)
    }

    pub fn branches_of(&self, parent_id: &str) -> Vec<String> {
        let sessions = self.sessions.read().unwrap();
        let mut ids: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.parent_id.as_deref() == Some(parent_id))
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    fn persist(&self, session: &AgentSession) {
        if let Some(db) = self.db.lock().unwrap().as_ref() {
            let json = serde_json::to_string(session).unwrap_or_default();
            let updated_at = chrono::Local::now().to_rfc3339();
            let _ = db.execute(
                "INSERT INTO sessions (id, data, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET data = ?2, updated_at = ?3",
                rusqlite::params![session.id, json, updated_at],
            );
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    fn msg(role: Role, content: &str) -> LlmMessage {
        LlmMessage {
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn create_get_update_delete_roundtrip() {
        let mgr = SessionManager::new();
        let s = mgr.create("s1".into());
        assert_eq!(s.id, "s1");
        assert!(mgr.get_or_create("s1").messages.is_empty());
        assert_eq!(mgr.len(), 1);

        let mut s2 = mgr.get_or_create("s1");
        s2.messages.push(msg(Role::User, "你好"));
        mgr.update(s2);
        assert_eq!(mgr.get_or_create("s1").messages.len(), 1);

        assert!(mgr.delete("s1").is_some());
        assert!(mgr.is_empty());
    }

    #[test]
    fn clear_context_keeps_session_and_totals() {
        let mgr = SessionManager::new();
        let mut s = mgr.create("s-clear".into());
        s.messages.push(msg(Role::User, "你好"));
        s.total_input_tokens = 1234;
        s.total_output_tokens = 567;
        s.last_input_tokens = 888;
        mgr.update(s);

        assert!(mgr.clear_context("s-clear"));
        let cleared = mgr.get_or_create("s-clear");
        assert!(cleared.messages.is_empty());
        assert_eq!(cleared.last_input_tokens, 0);
        // 累计用量是历史事实，清空上下文时保留
        assert_eq!(cleared.total_input_tokens, 1234);
        assert_eq!(cleared.total_output_tokens, 567);
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn compaction_thresholds() {
        let mgr = SessionManager::new();
        let mut s = AgentSession {
            id: "s".into(),
            name: None,
            messages: Vec::new(),
            created_at: String::new(),
            compacted_count: 0,
            parent_id: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            last_input_tokens: 0,
        };
        for i in 0..10 {
            s.messages.push(msg(Role::User, &format!("msg{}", i)));
        }
        assert!(!mgr.needs_compaction(&s));
        for i in 10..35 {
            s.messages.push(msg(Role::User, &format!("msg{}", i)));
        }
        assert!(mgr.needs_compaction(&s));
    }

    #[test]
    fn compact_keeps_recent_and_summary() {
        let mgr = SessionManager::new();
        let mut s = AgentSession {
            id: "s".into(),
            name: None,
            messages: Vec::new(),
            created_at: String::new(),
            compacted_count: 0,
            parent_id: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            last_input_tokens: 0,
        };
        for i in 0..20 {
            s.messages.push(msg(Role::User, &format!("msg{}", i)));
        }
        mgr.compact_with(&mut s, "用户问了 20 个问题".into());
        assert_eq!(s.messages.len(), 11);
        assert!(s.messages[0].content.contains("历史对话摘要"));
        assert_eq!(s.messages[1].content, "msg10");
        assert_eq!(s.compacted_count, 1);
    }

    /// SQLite 持久化：写入后重新打开，会话可恢复。
    #[test]
    fn sqlite_persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("clawdesk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("sessions.db");

        // 第一次打开：创建 + 写入
        {
            let mgr = SessionManager::new_with_db(&db_path).unwrap();
            let mut s = mgr.create("persist-1".into());
            s.messages.push(msg(Role::User, "持久化测试"));
            mgr.update(s);
        }

        // 第二次打开（模拟重启）：应恢复
        {
            let mgr = SessionManager::new_with_db(&db_path).unwrap();
            let restored = mgr.get_or_create("persist-1");
            assert_eq!(restored.messages.len(), 1);
            assert_eq!(restored.messages[0].content, "持久化测试");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_is_sorted() {
        let mgr = SessionManager::new();
        mgr.create("b".into());
        mgr.create("a".into());
        mgr.create("c".into());
        assert_eq!(mgr.list(), vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    /// 断点持久化：save_checkpoint -> load_checkpoint 往返（SQLite 模式，项目 10）。
    #[test]
    fn checkpoint_persist_roundtrip() {
        let dir = std::env::temp_dir().join(format!("clawdesk-ckpt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("sessions.db");

        {
            let mgr = SessionManager::new_with_db(&db_path).unwrap();
            let cp = AgentCheckpoint {
                session_id: "s-ck".into(),
                round: 3,
                plan_steps: vec!["步骤A".into(), "步骤B".into()],
                tool_history: vec!["builtin:file_write:success".into()],
                summary: "已完成第 3 轮".into(),
                created_at: chrono::Local::now().to_rfc3339(),
            };
            mgr.save_checkpoint(&cp);
            assert!(mgr.load_checkpoint("s-ck").is_some());
        }

        // 模拟重启：重新打开数据库，断点应可恢复
        {
            let mgr = SessionManager::new_with_db(&db_path).unwrap();
            let loaded = mgr.load_checkpoint("s-ck").expect("断点应恢复");
            assert_eq!(loaded.round, 3);
            assert_eq!(loaded.plan_steps.len(), 2);
            assert_eq!(loaded.tool_history[0], "builtin:file_write:success");
            // 清除后不可再加载
            mgr.clear_checkpoint("s-ck");
            assert!(mgr.load_checkpoint("s-ck").is_none());
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fork 分支隔离（§十二.2）：完整拷贝记忆 + parent_id 标注 + 独立演进。
    #[test]
    fn fork_branch_isolated() {
        let mgr = SessionManager::new();
        // 主会话写入记忆
        let mut main = mgr.create("main-1".into());
        main.messages.push(msg(Role::User, "原始需求"));
        mgr.update(main);

        // Fork 分支：完整拷贝
        let branch = mgr.fork("main-1", "branch-1".into()).expect("fork 应成功");
        assert_eq!(branch.messages.len(), 1);
        assert_eq!(branch.messages[0].content, "原始需求");
        assert_eq!(branch.parent_id.as_deref(), Some("main-1"));

        // 分支独立演进：主会话新增消息，分支不受影响
        let mut main2 = mgr.get_or_create("main-1");
        main2.messages.push(msg(Role::User, "主会话新消息"));
        mgr.update(main2);
        let branch2 = mgr.get_or_create("branch-1");
        assert_eq!(branch2.messages.len(), 1, "分支应保持 1 条消息，不受主会话影响");

        // 分支列表标注从属关系
        let branches = mgr.branches_of("main-1");
        assert_eq!(branches, vec!["branch-1".to_string()]);
    }

    /// Fork 后删除分支不影响主会话。
    #[test]
    fn fork_delete_keeps_parent() {
        let mgr = SessionManager::new();
        mgr.create("main-2".into());
        mgr.fork("main-2", "branch-2".into()).unwrap();
        assert!(mgr.delete("branch-2").is_some());
        assert!(mgr.get_or_create("main-2").id == "main-2");
        assert!(mgr.branches_of("main-2").is_empty());
    }
}
