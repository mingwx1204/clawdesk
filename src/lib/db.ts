/**
 * 数据访问层：Tauri 环境用 SQLite（tauri-plugin-sql），浏览器预览降级 localStorage。
 * 索引设计：messages(conv_id, created_at)、conversations(updated_at)，
 * 保证万级记录下分页查询 < 50ms。
 */

import { isTauri } from './backend';
import type { ChatMessage, Conversation, Experience, EvolutionEvent, Memory, Persona, Skill } from '@/types';

const DB_PATH = 'sqlite:clawdesk.db';

type Row = Record<string, unknown>;

/* ================= SQLite 实现 ================= */

async function sqlExec(query: string, values: unknown[] = []): Promise<void> {
  const { default: Database } = await import('@tauri-apps/plugin-sql');
  const db = await Database.load(DB_PATH);
  await db.execute(query, values);
}

async function sqlSelect<T extends Row>(query: string, values: unknown[] = []): Promise<T[]> {
  const { default: Database } = await import('@tauri-apps/plugin-sql');
  const db = await Database.load(DB_PATH);
  return db.select<T[]>(query, values);
}

async function initSqlite(): Promise<void> {
  await sqlExec(`
    CREATE TABLE IF NOT EXISTS personas (
      id TEXT PRIMARY KEY, name TEXT NOT NULL, system_prompt TEXT DEFAULT '',
      model_id TEXT DEFAULT '', mode TEXT DEFAULT 'standard', workdir TEXT DEFAULT '',
      created_at INTEGER NOT NULL
    )`);
  await sqlExec(`
    CREATE TABLE IF NOT EXISTS conversations (
      id TEXT PRIMARY KEY, persona_id TEXT NOT NULL, title TEXT NOT NULL,
      pinned INTEGER DEFAULT 0, workdir TEXT DEFAULT '', model_id TEXT DEFAULT '',
      created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
    )`);
  await sqlExec(`
    CREATE TABLE IF NOT EXISTS messages (
      id TEXT PRIMARY KEY, conv_id TEXT NOT NULL, role TEXT NOT NULL,
      content TEXT NOT NULL, attachments TEXT DEFAULT '', created_at INTEGER NOT NULL
    )`);
  await sqlExec(`CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conv_id, created_at DESC)`);
  await sqlExec(`CREATE INDEX IF NOT EXISTS idx_conv_updated ON conversations(updated_at DESC)`);
  await sqlExec(`CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)`);
  // 增量迁移：思考链字段（已存在时忽略错误）
  await sqlExec(`ALTER TABLE messages ADD COLUMN reasoning TEXT DEFAULT ''`).catch(() => {});
  // 进化引擎表
  await sqlExec(`
    CREATE TABLE IF NOT EXISTS experiences (
      id TEXT PRIMARY KEY, category TEXT NOT NULL, triggers TEXT DEFAULT '',
      content TEXT NOT NULL, code_snippet TEXT DEFAULT '',
      use_count INTEGER DEFAULT 0, success_rate REAL DEFAULT 1.0,
      source_conv_id TEXT DEFAULT '',
      created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
    )`);
  await sqlExec(`
    CREATE TABLE IF NOT EXISTS skills (
      id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT DEFAULT '',
      type TEXT DEFAULT 'workflow', definition TEXT NOT NULL,
      params_schema TEXT DEFAULT '{}', use_count INTEGER DEFAULT 0,
      success_rate REAL DEFAULT 1.0, auto_activate INTEGER DEFAULT 0,
      source TEXT DEFAULT 'generated', version INTEGER DEFAULT 1,
      created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
    )`);
  await sqlExec(`
    CREATE TABLE IF NOT EXISTS evolution_events (
      id TEXT PRIMARY KEY, type TEXT NOT NULL, summary TEXT DEFAULT '',
      related_id TEXT DEFAULT '', metrics TEXT DEFAULT '{}',
      timestamp INTEGER NOT NULL
    )`);
  // 永久记忆表
  await sqlExec(`
    CREATE TABLE IF NOT EXISTS memories (
      id TEXT PRIMARY KEY, content TEXT NOT NULL,
      keywords TEXT DEFAULT '[]', category TEXT DEFAULT 'knowledge',
      source_conv_id TEXT DEFAULT '', use_count INTEGER DEFAULT 0,
      created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
    )`);
}

/* ================= localStorage 降级实现 ================= */

const LS_KEY = 'clawdesk-mock-db';

interface MockDb {
  personas: Persona[];
  conversations: Conversation[];
  messages: ChatMessage[];
  settings: Record<string, string>;
  experiences: Experience[];
  skills: Skill[];
  evolutionEvents: EvolutionEvent[];
  memories: Memory[];
}

function loadMock(): MockDb {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<MockDb>;
      // 兼容旧数据：缺少新字段时用默认值
      return {
        personas: parsed.personas || [],
        conversations: parsed.conversations || [],
        messages: parsed.messages || [],
        settings: parsed.settings || {},
        experiences: parsed.experiences || [],
        skills: parsed.skills || [],
        evolutionEvents: parsed.evolutionEvents || [],
        memories: parsed.memories || [],
      };
    }
  } catch { /* 损坏则重建 */ }
  return { personas: [], conversations: [], messages: [], settings: {}, experiences: [], skills: [], evolutionEvents: [], memories: [] };
}

function saveMock(db: MockDb): void {
  localStorage.setItem(LS_KEY, JSON.stringify(db));
}

/* ================= 统一 API ================= */

let initialized = false;

export async function initDb(): Promise<void> {
  if (initialized) return;
  if (isTauri()) await initSqlite();
  initialized = true;
}

/* ---------- 分身 ---------- */

function rowToPersona(r: Row): Persona {
  return {
    id: r.id as string, name: r.name as string,
    systemPrompt: (r.system_prompt as string) ?? '',
    modelId: (r.model_id as string) ?? '',
    mode: ((r.mode as string) ?? 'standard') as Persona['mode'],
    workdir: (r.workdir as string) ?? '',
    createdAt: r.created_at as number,
  };
}

export async function listPersonas(): Promise<Persona[]> {
  if (!isTauri()) return loadMock().personas;
  const rows = await sqlSelect('SELECT * FROM personas ORDER BY created_at ASC');
  return rows.map(rowToPersona);
}

export async function upsertPersona(p: Persona): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    const i = db.personas.findIndex((x) => x.id === p.id);
    i >= 0 ? (db.personas[i] = p) : db.personas.push(p);
    saveMock(db);
    return;
  }
  await sqlExec(
    `INSERT INTO personas (id,name,system_prompt,model_id,mode,workdir,created_at)
     VALUES ($1,$2,$3,$4,$5,$6,$7)
     ON CONFLICT(id) DO UPDATE SET name=$2,system_prompt=$3,model_id=$4,mode=$5,workdir=$6`,
    [p.id, p.name, p.systemPrompt, p.modelId, p.mode, p.workdir, p.createdAt],
  );
}

export async function deletePersona(id: string): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.personas = db.personas.filter((x) => x.id !== id);
    const convIds = db.conversations.filter((c) => c.personaId === id).map((c) => c.id);
    db.conversations = db.conversations.filter((c) => c.personaId !== id);
    db.messages = db.messages.filter((m) => !convIds.includes(m.convId));
    saveMock(db);
    return;
  }
  await sqlExec('DELETE FROM personas WHERE id=$1', [id]);
  await sqlExec('DELETE FROM messages WHERE conv_id IN (SELECT id FROM conversations WHERE persona_id=$1)', [id]);
  await sqlExec('DELETE FROM conversations WHERE persona_id=$1', [id]);
}

/* ---------- 对话 ---------- */

function rowToConv(r: Row): Conversation {
  return {
    id: r.id as string, personaId: r.persona_id as string, title: r.title as string,
    pinned: Boolean(r.pinned), workdir: (r.workdir as string) ?? '',
    modelId: (r.model_id as string) ?? '',
    createdAt: r.created_at as number, updatedAt: r.updated_at as number,
  };
}

export async function listConversations(personaId?: string): Promise<Conversation[]> {
  const sort = (a: Conversation, b: Conversation) =>
    Number(b.pinned) - Number(a.pinned) || b.updatedAt - a.updatedAt;
  if (!isTauri()) {
    const db = loadMock();
    return db.conversations.filter((c) => !personaId || c.personaId === personaId).sort(sort);
  }
  const rows = personaId
    ? await sqlSelect('SELECT * FROM conversations WHERE persona_id=$1', [personaId])
    : await sqlSelect('SELECT * FROM conversations');
  return rows.map(rowToConv).sort(sort);
}

export async function upsertConversation(c: Conversation): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    const i = db.conversations.findIndex((x) => x.id === c.id);
    i >= 0 ? (db.conversations[i] = c) : db.conversations.push(c);
    saveMock(db);
    return;
  }
  await sqlExec(
    `INSERT INTO conversations (id,persona_id,title,pinned,workdir,model_id,created_at,updated_at)
     VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
     ON CONFLICT(id) DO UPDATE SET title=$3,pinned=$4,workdir=$5,model_id=$6,updated_at=$8`,
    [c.id, c.personaId, c.title, c.pinned ? 1 : 0, c.workdir, c.modelId, c.createdAt, c.updatedAt],
  );
}

export async function deleteConversation(id: string): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.conversations = db.conversations.filter((c) => c.id !== id);
    db.messages = db.messages.filter((m) => m.convId !== id);
    saveMock(db);
    return;
  }
  await sqlExec('DELETE FROM conversations WHERE id=$1', [id]);
  await sqlExec('DELETE FROM messages WHERE conv_id=$1', [id]);
}

/* ---------- 消息 ---------- */

function rowToMsg(r: Row): ChatMessage {
  let attachments: ChatMessage['attachments'];
  try {
    attachments = r.attachments ? JSON.parse(r.attachments as string) : undefined;
  } catch { attachments = undefined; }
  return {
    id: r.id as string, convId: r.conv_id as string,
    role: r.role as ChatMessage['role'], content: r.content as string,
    reasoning: (r.reasoning as string) || undefined,
    attachments, createdAt: r.created_at as number,
  };
}

/** 分页加载：最新 offset..offset+limit 条，按时间升序返回（便于直接渲染） */
export async function listMessages(convId: string, offset = 0, limit = 50): Promise<ChatMessage[]> {
  if (!isTauri()) {
    const all = loadMock().messages
      .filter((m) => m.convId === convId)
      .sort((a, b) => a.createdAt - b.createdAt);  // Sort by time (Bug #17 fix)
    const start = Math.max(0, all.length - offset - limit);
    return all.slice(start, all.length - offset);
  }
  const rows = await sqlSelect(
    `SELECT * FROM (SELECT * FROM messages WHERE conv_id=$1 ORDER BY created_at DESC LIMIT $2 OFFSET $3)
     ORDER BY created_at ASC`,
    [convId, limit, offset],
  );
  return rows.map(rowToMsg);
}

export async function countMessages(convId: string): Promise<number> {
  if (!isTauri()) return loadMock().messages.filter((m) => m.convId === convId).length;
  const rows = await sqlSelect<{ n: number }>('SELECT COUNT(*) as n FROM messages WHERE conv_id=$1', [convId]);
  return rows[0]?.n ?? 0;
}

export async function upsertMessage(m: ChatMessage): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    const i = db.messages.findIndex((x) => x.id === m.id);
    i >= 0 ? (db.messages[i] = m) : db.messages.push(m);
    saveMock(db);
    return;
  }
  await sqlExec(
    `INSERT INTO messages (id,conv_id,role,content,attachments,created_at,reasoning)
     VALUES ($1,$2,$3,$4,$5,$6,$7)
     ON CONFLICT(id) DO UPDATE SET content=$4,attachments=$5,reasoning=$7`,
    [m.id, m.convId, m.role, m.content, JSON.stringify(m.attachments ?? []), m.createdAt, m.reasoning ?? ''],
  );
}

export async function deleteMessage(id: string): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.messages = db.messages.filter((m) => m.id !== id);
    saveMock(db);
    return;
  }
  await sqlExec('DELETE FROM messages WHERE id=$1', [id]);
}

/** 全文搜索（候选集拉给 Web Worker 做高亮匹配；万级 < 500ms） */
export async function searchMessages(keyword: string): Promise<ChatMessage[]> {
  const pattern = `%${keyword}%`;
  if (!isTauri()) {
    return loadMock().messages.filter((m) => m.content.toLowerCase().includes(keyword.toLowerCase()));
  }
  const rows = await sqlSelect('SELECT * FROM messages WHERE content LIKE $1 ORDER BY created_at DESC LIMIT 10000', [pattern]);
  return rows.map(rowToMsg);
}

/* ---------- 设置 ---------- */

export async function getSetting(key: string): Promise<string | null> {
  if (!isTauri()) return loadMock().settings[key] ?? null;
  const rows = await sqlSelect<{ value: string }>('SELECT value FROM settings WHERE key=$1', [key]);
  return rows[0]?.value ?? null;
}

export async function setSetting(key: string, value: string): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.settings[key] = value;
    saveMock(db);
    return;
  }
  await sqlExec('INSERT INTO settings (key,value) VALUES ($1,$2) ON CONFLICT(key) DO UPDATE SET value=$2', [key, value]);
}

/* ---------- 进化引擎 ─── Experience ---------- */

export async function upsertExperience(e: Experience): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    const i = db.experiences.findIndex((x) => x.id === e.id);
    i >= 0 ? (db.experiences[i] = e) : db.experiences.push(e);
    saveMock(db);
    return;
  }
  await sqlExec(
    `INSERT INTO experiences (id,category,triggers,content,code_snippet,use_count,success_rate,source_conv_id,created_at,updated_at)
     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
     ON CONFLICT(id) DO UPDATE SET use_count=$6,success_rate=$7,updated_at=$10`,
    [e.id, e.category, JSON.stringify(e.triggers), e.content, e.codeSnippet ?? '', e.useCount, e.successRate, e.sourceConvId, e.createdAt, e.updatedAt],
  );
}

export async function listExperiences(limit = 50): Promise<Experience[]> {
  if (!isTauri()) {
    return loadMock().experiences.slice(-limit);
  }
  const rows = await sqlSelect<Row & { category: string; triggers: string; content: string; code_snippet: string; use_count: number; success_rate: number; source_conv_id: string }>(
    'SELECT * FROM experiences ORDER BY updated_at DESC LIMIT $1', [limit],
  );
  return rows.map(r => ({
    id: String(r.id), category: r.category as Experience['category'],
    triggers: JSON.parse(r.triggers || '[]'),
    content: r.content, codeSnippet: r.code_snippet || undefined,
    useCount: r.use_count, successRate: r.success_rate,
    sourceConvId: r.source_conv_id,
    createdAt: Number(r.created_at), updatedAt: Number(r.updated_at),
  }));
}

export async function deleteExperience(id: string): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.experiences = db.experiences.filter(e => e.id !== id);
    saveMock(db);
    return;
  }
  await sqlExec('DELETE FROM experiences WHERE id=$1', [id]);
}

/* ---------- 进化引擎 ─── Skill ---------- */

export async function upsertSkill(s: Skill): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    const i = db.skills.findIndex((x) => x.id === s.id);
    i >= 0 ? (db.skills[i] = s) : db.skills.push(s);
    saveMock(db);
    return;
  }
  await sqlExec(
    `INSERT INTO skills (id,name,description,type,definition,params_schema,use_count,success_rate,auto_activate,source,version,created_at,updated_at)
     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
     ON CONFLICT(id) DO UPDATE SET description=$3,definition=$5,use_count=$7,success_rate=$8,version=$11,updated_at=$13`,
    [s.id, s.name, s.description, s.type, s.definition, JSON.stringify(s.paramsSchema ?? {}), s.useCount, s.successRate, s.autoActivate ? 1 : 0, s.source, s.version, s.createdAt, s.updatedAt],
  );
}

export async function listSkills(): Promise<Skill[]> {
  if (!isTauri()) return loadMock().skills;
  const rows = await sqlSelect<Row & { name: string; description: string; type: string; definition: string; params_schema: string; use_count: number; success_rate: number; auto_activate: number; source: string; version: number }>(
    'SELECT * FROM skills ORDER BY updated_at DESC',
  );
  return rows.map(r => ({
    id: String(r.id), name: r.name, description: r.description,
    type: r.type as Skill['type'], definition: r.definition,
    paramsSchema: JSON.parse(r.params_schema || '{}'),
    useCount: r.use_count, successRate: r.success_rate,
    autoActivate: r.auto_activate === 1,
    source: r.source as Skill['source'], version: r.version,
    createdAt: Number(r.created_at), updatedAt: Number(r.updated_at),
  }));
}

export async function deleteSkill(id: string): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.skills = db.skills.filter(s => s.id !== id);
    saveMock(db);
    return;
  }
  await sqlExec('DELETE FROM skills WHERE id=$1', [id]);
}

/* ---------- 进化引擎 ─── EvolutionEvent ---------- */

export async function upsertEvolution(ev: EvolutionEvent): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.evolutionEvents.push(ev);
    saveMock(db);
    return;
  }
  await sqlExec(
    `INSERT INTO evolution_events (id,type,summary,related_id,metrics,timestamp)
     VALUES ($1,$2,$3,$4,$5,$6)`,
    [ev.id, ev.type, ev.summary, ev.relatedId ?? '', JSON.stringify(ev.metrics ?? {}), ev.timestamp],
  );
}

export async function listEvolutionEvents(limit = 50): Promise<EvolutionEvent[]> {
  if (!isTauri()) return loadMock().evolutionEvents.slice(-limit);
  const rows = await sqlSelect<Row & { type: string; summary: string; related_id: string; metrics: string; timestamp: number }>(
    'SELECT * FROM evolution_events ORDER BY timestamp DESC LIMIT $1', [limit],
  );
  return rows.map(r => ({
    id: String(r.id), type: r.type as EvolutionEvent['type'], summary: r.summary,
    relatedId: r.related_id || undefined,
    metrics: JSON.parse(r.metrics || '{}'),
    timestamp: r.timestamp,
  }));
}

/* ---------- 永久记忆 ─── Memory ---------- */

export async function upsertMemory(m: Memory): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    const i = db.memories.findIndex((x) => x.id === m.id);
    i >= 0 ? (db.memories[i] = m) : db.memories.push(m);
    saveMock(db);
    return;
  }
  await sqlExec(
    `INSERT INTO memories (id,content,keywords,category,source_conv_id,use_count,created_at,updated_at)
     VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
     ON CONFLICT(id) DO UPDATE SET content=$2,keywords=$3,category=$4,use_count=$6,updated_at=$8`,
    [m.id, m.content, JSON.stringify(m.keywords), m.category, m.sourceConvId, m.useCount, m.createdAt, m.updatedAt],
  );
}

export async function listMemories(): Promise<Memory[]> {
  if (!isTauri()) return loadMock().memories.sort((a, b) => b.updatedAt - a.updatedAt);
  const rows = await sqlSelect<Row & { content: string; keywords: string; category: string; source_conv_id: string; use_count: number }>(
    'SELECT * FROM memories ORDER BY updated_at DESC',
  );
  return rows.map(r => ({
    id: String(r.id),
    content: r.content,
    keywords: JSON.parse(r.keywords || '[]'),
    category: r.category as Memory['category'],
    sourceConvId: r.source_conv_id,
    useCount: r.use_count,
    createdAt: Number(r.created_at),
    updatedAt: Number(r.updated_at),
  }));
}

export async function deleteMemory(id: string): Promise<void> {
  if (!isTauri()) {
    const db = loadMock();
    db.memories = db.memories.filter(m => m.id !== id);
    saveMock(db);
    return;
  }
  await sqlExec('DELETE FROM memories WHERE id=$1', [id]);
}

export async function searchMemories(keyword: string): Promise<Memory[]> {
  const all = await listMemories();
  const kw = keyword.toLowerCase();
  return all.filter(m =>
    m.content.toLowerCase().includes(kw) ||
    m.keywords.some(k => k.toLowerCase().includes(kw))
  );
}
