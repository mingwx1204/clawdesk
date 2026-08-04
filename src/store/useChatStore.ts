/**
 * 聊天状态管理（Zustand Store）。
 * 核心职责：分身 CRUD、对话 CRUD、消息收发、LLM 流式调用、工具调用循环。
 * 数据持久化到 IndexedDB（通过 @/lib/db）。
 */

import { create } from 'zustand';
import type { Attachment, ChatMessage, Conversation, Persona } from '@/types';
import * as db from '@/lib/db';
import { buildContext } from '@/lib/llm';
import { llmStream, isTauri, type LlmStreamHandle } from '@/lib/backend';
import { useSettingsStore } from './useSettingsStore';
import { notify } from '@/lib/backend';
import { speak, getIsSpeaking, isTtsAvailable } from '@/lib/tts';
import { parseToolCalls, executeToolCalls, getToolsSystemPrompt, guessWorkdirFromMessage, preFetchDirContext } from '@/lib/tools';
// SEED_DATA removed for production — no test data leakage

function uid(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 9)}`;
}

export interface ImportConversation {
  title: string;
  createdAt: number;
  messages: { role: 'user' | 'assistant'; content: string; createdAt: number }[];
}

export interface ImportPayload {
  personaName?: string;
  conversations: ImportConversation[];
}

interface ChatState {
  personas: Persona[];
  currentPersonaId: string;
  conversations: Conversation[];
  currentConvId: string;
  messages: ChatMessage[];
  totalMessages: number;
  generating: boolean;
  searchQuery: string;
  searchResults: ChatMessage[] | null;

  init: () => Promise<void>;
  selectPersona: (id: string) => Promise<void>;
  savePersona: (p: Persona) => Promise<void>;
  removePersona: (id: string) => Promise<void>;

  newConversation: (title?: string) => Promise<void>;
  selectConversation: (id: string) => Promise<void>;
  renameConversation: (id: string, title: string) => Promise<void>;
  setConversationModel: (id: string, modelId: string) => Promise<void>;
  togglePin: (id: string) => Promise<void>;
  removeConversation: (id: string) => Promise<void>;
  exportConversation: (id: string, format: 'md' | 'json') => Promise<string>;
  importConversations: (data: ImportPayload) => Promise<void>;

  loadOlderMessages: () => Promise<void>;
  send: (content: string, attachments?: Attachment[]) => Promise<void>;
  sendWithWechatReply: (content: string, wechatMsgId: string, wechatFromUser: string, wechatContextToken?: string) => Promise<void>;
  regenerate: (messageId: string) => Promise<void>;
  removeMessage: (id: string) => Promise<void>;
  stopGenerating: () => void;
  clearMessages: () => Promise<void>;
  search: (q: string) => Promise<void>;
  clearSearch: () => void;
}

let abortController: AbortController | null = null;
let currentStream: LlmStreamHandle | null = null;

export const useChatStore = create<ChatState>((set, get) => ({
  personas: [],
  currentPersonaId: '',
  conversations: [],
  currentConvId: '',
  messages: [],
  totalMessages: 0,
  generating: false,
  searchQuery: '',
  searchResults: null,

  init: async () => {
    await db.initDb();
    let personas = await db.listPersonas();
    if (personas.length === 0) {
      // 首次运行：创建默认分身
      const def: Persona = {
        id: uid(), name: '开发助手',
        systemPrompt: `你是 ClawDesk 的开发助手，运行在用户的本地桌面环境中，拥有对本机文件系统的完整读写权限。

## 核心能力
- 直接**读取、写入、重命名、删除**用户电脑上的任何文件
- **列出目录结构**、**搜索文件**
- 通过**终端执行命令**（编译、运行、安装依赖等）

## 重要规则
1. 当用户要求操作文件时，直接使用工具执行，绝对不能说"无法访问"或"没有权限"
2. 修改文件前先用 read_file 工具读取当前内容
3. 写入文件时提供完整内容
4. 回答简洁专业`,
        modelId: useSettingsStore.getState().settings.defaultModelId,
        mode: 'standard', workdir: '', createdAt: Date.now(),
      };
      await db.upsertPersona(def);
      personas = [def];
    }
    const currentPersonaId = personas[0].id;
    const conversations = await db.listConversations(currentPersonaId);
    // Production: no seed data — clean start
    set({ personas, currentPersonaId, conversations });
    // 确保始终有一个对话
    if (conversations.length === 0) {
      await get().newConversation('永久记忆');
    } else {
      await get().selectConversation(conversations[0].id);
    }
    // 加载永久记忆
    try {
      const { useMemoryStore } = await import('./useMemoryStore');
      await useMemoryStore.getState().load();
    } catch { /* 静默 */ }
  },

  selectPersona: async (id) => {
    const conversations = await db.listConversations(id);
    set({ currentPersonaId: id, conversations, currentConvId: '', messages: [], totalMessages: 0 });
    if (conversations.length > 0) await get().selectConversation(conversations[0].id);
  },

  savePersona: async (p) => {
    await db.upsertPersona(p);
    const personas = await db.listPersonas();
    set({ personas, currentPersonaId: p.id });
    // 切到该分身的最近对话
    await get().selectPersona(p.id);
  },

  removePersona: async (id) => {
    await db.deletePersona(id);
    const personas = await db.listPersonas();
    set({ personas });
    if (get().currentPersonaId === id && personas.length > 0) {
      await get().selectPersona(personas[0].id);
    }
  },

  newConversation: async (title?: string) => {
    const { currentPersonaId, personas } = get();
    const persona = personas.find((p) => p.id === currentPersonaId);
    const conv: Conversation = {
      id: uid(), personaId: currentPersonaId, title: title || '新对话',
      pinned: false, workdir: persona?.workdir ?? '', modelId: persona?.modelId ?? '',
      createdAt: Date.now(), updatedAt: Date.now(),
    };
    await db.upsertConversation(conv);
    const conversations = await db.listConversations(currentPersonaId);
    set({ conversations, currentConvId: conv.id, messages: [], totalMessages: 0, searchResults: null });
  },

  selectConversation: async (id) => {
    const [messages, total] = await Promise.all([
      db.listMessages(id, 0, 50),
      db.countMessages(id),
    ]);
    set({ currentConvId: id, messages, totalMessages: total, searchResults: null });
  },

  renameConversation: async (id, title) => {
    const conv = get().conversations.find((c) => c.id === id);
    if (!conv) return;
    await db.upsertConversation({ ...conv, title });
    set({ conversations: get().conversations.map((c) => (c.id === id ? { ...c, title } : c)) });
  },

  setConversationModel: async (id, modelId) => {
    const conv = get().conversations.find((c) => c.id === id);
    if (!conv || conv.modelId === modelId) return;
    await db.upsertConversation({ ...conv, modelId });
    set({ conversations: get().conversations.map((c) => (c.id === id ? { ...c, modelId } : c)) });
  },

  togglePin: async (id) => {
    const conv = get().conversations.find((c) => c.id === id);
    if (!conv) return;
    await db.upsertConversation({ ...conv, pinned: !conv.pinned });
    const conversations = await db.listConversations(get().currentPersonaId);
    set({ conversations });
  },

  removeConversation: async (id) => {
    await db.deleteConversation(id);
    const conversations = await db.listConversations(get().currentPersonaId);
    set({ conversations });
    if (get().currentConvId === id) {
      if (conversations.length > 0) await get().selectConversation(conversations[0].id);
      else set({ currentConvId: '', messages: [], totalMessages: 0 });
    }
  },

  exportConversation: async (id, format) => {
    const conv = get().conversations.find((c) => c.id === id);
    const msgs = await db.listMessages(id, 0, 100000);
    if (format === 'json') {
      return JSON.stringify({ conversation: conv, messages: msgs }, null, 2);
    }
    const lines = [`# ${conv?.title ?? '对话导出'}`, ''];
    for (const m of msgs) {
      lines.push(`## ${m.role === 'user' ? '用户' : 'AI'} · ${new Date(m.createdAt).toLocaleString('zh-CN')}`, '', m.content, '');
    }
    return lines.join('\n');
  },

  /** 批量导入对话（用于迁移/恢复） */
  importConversations: async (data) => {
    const { personas, currentPersonaId } = get();
    // 确保有默认分身
    let personaId = currentPersonaId;
    if (!personaId && personas.length === 0) {
      const p: Persona = {
        id: uid(), name: data.personaName ?? '历史记录',
        systemPrompt: '', modelId: '', mode: 'standard', workdir: '', createdAt: Date.now(),
      };
      await db.upsertPersona(p);
      personaId = p.id;
      set({ personas: [p], currentPersonaId: p.id });
    } else if (!personaId && personas.length > 0) {
      personaId = personas[0].id;
    }

    for (const conv of data.conversations) {
      const convId = uid();
      const now = conv.createdAt || Date.now();
      const conversation: Conversation = {
        id: convId, personaId, title: conv.title,
        pinned: false, workdir: '', modelId: '',
        createdAt: now, updatedAt: now,
      };
      await db.upsertConversation(conversation);
      for (const msg of conv.messages) {
        const chatMsg: ChatMessage = {
          id: uid(), convId, role: msg.role,
          content: msg.content, attachments: [], streaming: false,
          createdAt: msg.createdAt || now,
        };
        await db.upsertMessage(chatMsg);
      }
    }

    // 刷新列表
    const conversations = await db.listConversations(personaId);
    set({ conversations });
    if (conversations.length > 0) {
      const msgs = await db.listMessages(conversations[0].id, 0, 50);
      const total = await db.countMessages(conversations[0].id);
      set({ currentConvId: conversations[0].id, messages: msgs, totalMessages: total });
    }
  },

  loadOlderMessages: async () => {
    const { currentConvId, messages, totalMessages } = get();
    if (!currentConvId || messages.length >= totalMessages) return;
    const older = await db.listMessages(currentConvId, messages.length, 50);
    set({ messages: [...older, ...messages] });
  },

  send: async (content, attachments) => {
    let { currentConvId, generating } = get();
    if (generating || (!content.trim() && !attachments?.length)) return;

    // 发送新消息时立即停止上一轮的朗读 (Bug fix)
    const { stopSpeaking } = await import('@/lib/tts');
    stopSpeaking();

    // Auto-create conversation if none exists
    if (!currentConvId) {
      await get().newConversation();
      currentConvId = get().currentConvId;
      if (!currentConvId) return;
    }

    const now = Date.now();
    const userMsg: ChatMessage = { id: uid(), convId: currentConvId, role: 'user', content, attachments, createdAt: now };
    await db.upsertMessage(userMsg);
    set((s) => ({
      messages: [...s.messages, userMsg],
      totalMessages: s.totalMessages + 1,
      generating: true,
    }));

    // 首条消息自动生成标题
    const conv = get().conversations.find((c) => c.id === currentConvId);
    if (conv && conv.title === '新对话') {
      const title = content.trim().split('\n')[0].slice(0, 30) || '新对话';
      await get().renameConversation(currentConvId, title);
    }

    // 启动工具调用循环（最多 5 轮），异常兜底
    try {
      // 注意：不传 conv 快照，runToolLoop 内部每次从 store 实时获取，
      // 避免标题重命名等状态被过期对象覆盖 (Bug fix)
      await runToolLoop(currentConvId);
    } catch (e) {
      console.error('runToolLoop error:', e);
      // 显示错误消息给用户
      const errMsg: ChatMessage = {
        id: uid(), convId: currentConvId, role: 'assistant',
        content: `> ⚠️ 发生错误：${(e as Error).message || '未知错误'}\n\n请重试或检查 API 密钥配置。`,
        createdAt: Date.now(),
      };
      await db.upsertMessage(errMsg);
      set((s) => ({ messages: [...s.messages, errMsg], totalMessages: s.totalMessages + 1, generating: false }));
    }
  },

  /** 处理微信 Bot 消息：发送到 AI 后自动回复 */
  sendWithWechatReply: async (content, wechatMsgId, wechatFromUser, wechatContextToken) => {
    const { currentConvId } = get();
    if (!currentConvId || !content.trim()) return;

    // 复用 send 的流程
    await get().send(`[微信] ${content}`);

    // 等待 AI 回复完成后发送到微信
    const checkInterval = setInterval(async () => {
      if (!useChatStore.getState().generating) {
        clearInterval(checkInterval);
        // 获取最后一条 AI 消息作为回复
        const msgs = useChatStore.getState().messages;
        const lastAi = [...msgs].reverse().find((m) => m.role === 'assistant');
        if (lastAi?.content && !lastAi.content.includes('⚠️')) {
          void import('@/store/useWechatBotStore').then((m) =>
            m.useWechatBotStore.getState().reply(wechatMsgId, wechatFromUser, lastAi.content, wechatContextToken)
          );
        }
      }
    }, 1000);
  },

  regenerate: async (messageId) => {
    const { messages, generating } = get();
    if (generating) return;
    const idx = messages.findIndex((m) => m.id === messageId);
    if (idx < 0 || messages[idx].role !== 'assistant') return;
    // 删除该条 AI 消息，以之前的最后一条用户消息重新生成
    const target = messages[idx];
    await db.deleteMessage(messageId);
    set((s) => ({ messages: s.messages.filter((m) => m.id !== messageId), totalMessages: s.totalMessages - 1 }));
    const lastUser = [...messages.slice(0, idx)].reverse().find((m) => m.role === 'user');
    if (lastUser) {
      // 直接复用 send 的流式管线，但不再追加用户消息
      const { currentConvId } = get();
      if (!currentConvId) return;
      const aiMsg: ChatMessage = { id: uid(), convId: currentConvId, role: 'assistant', content: '', createdAt: Date.now(), streaming: true };
      set((s) => ({ messages: [...s.messages, aiMsg], totalMessages: s.totalMessages + 1, generating: true }));
      const settings = useSettingsStore.getState().settings;
      const persona = get().personas.find((p) => p.id === get().currentPersonaId);
      const conv = get().conversations.find((c) => c.id === currentConvId);
      const model = useSettingsStore.getState().resolveModel(conv?.modelId || persona?.modelId || settings.defaultModelId);
      abortController = new AbortController();
      let acc = '';
      let reasoningAcc = '';
      const callbacks = {
        onDelta: (t: string) => {
          acc += t;
          set((s) => ({ messages: s.messages.map((m) => (m.id === aiMsg.id ? { ...m, content: acc } : m)) }));
        },
        onReasoning: (t: string) => {
          reasoningAcc += t;
          set((s) => ({ messages: s.messages.map((m) => (m.id === aiMsg.id ? { ...m, reasoning: reasoningAcc } : m)) }));
        },
        onDone: async () => {
          const final = { ...target, id: aiMsg.id, content: acc, reasoning: reasoningAcc || undefined, streaming: false, createdAt: aiMsg.createdAt };
          await db.upsertMessage(final);
          set((s) => ({ generating: false, messages: s.messages.map((m) => (m.id === aiMsg.id ? final : m)) }));
          // TTS 朗读 AI 回复
          const ttsOn = useSettingsStore.getState().settings.ttsEnabled;
          if (ttsOn && isTtsAvailable() && acc.trim()) {
            // 过滤 markdown 符号，只读纯文本
            const plainText = acc.replace(/```[\s\S]*?```/g, '代码块已省略。').replace(/[#*_~`>\[\]|\\]/g, '').trim();
            if (plainText) void speak(plainText);
          }
        },
        onError: async (err: string) => {
          const final = { ...aiMsg, content: acc + `\n\n> ⚠️ ${err}`, streaming: false };
          await db.upsertMessage(final);
          set((s) => ({ generating: false, messages: s.messages.map((m) => (m.id === aiMsg.id ? final : m)) }));
        },
      };
      try {
        currentStream = await llmStream(
          {
            apiBase: model?.apiBase ?? '',
            apiKey: model ? (model.builtin ? (settings.apiKeys[model.id] ?? '') : model.apiKey) : '',
            model: model?.model ?? '',
            messages: buildContext(persona?.systemPrompt ?? '', get().messages.filter((m) => m.id !== aiMsg.id)),
            params: settings.modelParams,
            mode: persona?.mode ?? settings.defaultMode,
            signal: abortController.signal,
          },
          callbacks,
        );
      } catch (e) {
        await callbacks.onError(String(e));
      }
    }
  },

  removeMessage: async (id) => {
    await db.deleteMessage(id);
    set((s) => ({ messages: s.messages.filter((m) => m.id !== id), totalMessages: s.totalMessages - 1 }));
  },

  stopGenerating: () => {
    abortController?.abort();
    abortController = null;
    currentStream?.cancel();
    currentStream = null;
  },

  clearMessages: async () => {
    const { currentConvId, messages } = get();
    if (!currentConvId) return;
    for (const m of messages) await db.deleteMessage(m.id);
    set({ messages: [], totalMessages: 0 });
  },

  search: async (q) => {
    set({ searchQuery: q });
    if (!q.trim()) { set({ searchResults: null }); return; }
    // 直接调用 DB LIKE 查询关键词
    const results = await db.searchMessages(q.trim());
    set({ searchResults: results });
  },

  clearSearch: () => set({ searchQuery: '', searchResults: null }),
}));

/* ---------- 工具调用循环 ---------- */

/** 最大工具调用轮数，防止无限循环 */
const MAX_TOOL_ROUNDS = 5;

/**
 * 内部函数：发起一次 AI 流式调用并等待完整回复。
 * 使用 Promise 封装回调式 llmStream，返回完整内容和推理链。
 * 自动将工具系统提示词注入到 system prompt 中。
 */
async function callAIOnce(convId: string, aiMsgId: string): Promise<{ content: string; reasoning: string }> {
  const store = useChatStore.getState();
  const settings = useSettingsStore.getState().settings;
  const persona = store.personas.find((p) => p.id === store.currentPersonaId);
  const conv = store.conversations.find((c) => c.id === convId);
  const modelId = conv?.modelId || persona?.modelId || settings.defaultModelId;
  const mode = persona?.mode ?? settings.defaultMode;
  const workdir = persona?.workdir ?? conv?.workdir ?? '';

  // 从当前用户消息中自动提取路径（优先于已保存的 workdir）
  const lastUserMsg = [...store.messages].reverse().find((m) => m.role === 'user');
  const guessedDir = lastUserMsg ? guessWorkdirFromMessage(lastUserMsg.content) : '';
  const effectiveWorkdir = guessedDir || workdir;

  // 智能路由：__auto__ → 根据消息内容自动选 Flash/Pro
  const resolveModel = useSettingsStore.getState().resolveModel;
  const routeModel = useSettingsStore.getState().routeModel;
  const model = (modelId === '__auto__' && lastUserMsg)
    ? routeModel(lastUserMsg.content)
    : (resolveModel(modelId) ?? resolveModel('deepseek-v4-pro'));

  // 构建上下文：系统提示词 + 工具说明 + 历史消息
  let systemPrompt = (persona?.systemPrompt ?? '') + '\n\n' + getToolsSystemPrompt(effectiveWorkdir);
  // 🧠 HyDE增强记忆注入：当前对话记忆优先，旧对话任务记忆降权
  try {
    const { useMemoryStore } = await import('./useMemoryStore');
    const memoryStore = useMemoryStore.getState();
    if (memoryStore.memories.length > 0) {
      const enhanced = memoryStore.getContextHyde(lastUserMsg?.content ?? '', systemPrompt, convId);
      if (enhanced !== systemPrompt) systemPrompt = enhanced;
    }
  } catch { /* 静默 */ }
  // 🔄 注入进化引擎提取的历史经验（Bug fix：此前经验从未被使用）
  try {
    const { useEvolutionStore } = await import('./useEvolutionStore');
    const evoStore = useEvolutionStore.getState();
    if (!evoStore.loaded) await evoStore.load();
    const expContext = evoStore.getRelevantContext(lastUserMsg?.content ?? '');
    if (expContext) {
      systemPrompt += `\n\n## 历史经验（进化引擎提取）\n${expContext}\n\n请参考以上经验来更好地回答当前问题。`;
    }
  } catch { /* 静默 */ }
  const history = store.messages.filter((m) => m.id !== aiMsgId);

  // 预执行：如果用户提到路径，提前列出目录内容
  let preFetchContent = '';
  if (lastUserMsg && guessedDir) {
    preFetchContent = await preFetchDirContext(lastUserMsg.content);
  }

  // 将工具提示 + 预加载内容注入到最后一条用户消息中
  const pathHint = effectiveWorkdir ? `当前工作目录: ${effectiveWorkdir}` : '';
  const toolReminder = `\n\n[系统指令] 你是 ClawDesk 系统工具，拥有本机文件完整读写权。用户要求操作文件时直接用 \`\`\`tool:xxx\`\`\` 格式执行，禁止说无法访问。${pathHint}${preFetchContent}`;
  const contextMessages = buildContext(systemPrompt, history);
  for (let i = contextMessages.length - 1; i >= 0; i--) {
    if (contextMessages[i].role === 'user') {
      contextMessages[i] = {
        ...contextMessages[i],
        content: contextMessages[i].content + toolReminder,
      };
      break;
    }
  }

  abortController = new AbortController();
  let acc = '';
  let reasoningAcc = '';
  let settled = false;

  return new Promise((resolve, reject) => {
    const settle = (fn: () => void) => {
      if (!settled) { settled = true; fn(); }
    };
    const callbacks = {
      onDelta: (t: string) => {
        acc += t;
        useChatStore.setState((s) => ({
          messages: s.messages.map((m) => (m.id === aiMsgId ? { ...m, content: acc } : m)),
        }));
      },
      onReasoning: (t: string) => {
        reasoningAcc += t;
        useChatStore.setState((s) => ({
          messages: s.messages.map((m) => (m.id === aiMsgId ? { ...m, reasoning: reasoningAcc } : m)),
        }));
      },
      onDone: () => settle(() => resolve({ content: acc, reasoning: reasoningAcc })),
      onError: (err: string) => settle(() => reject(new Error(err))),
    };

    void llmStream(
      {
        apiBase: model?.apiBase ?? '',
        apiKey: model ? (model.builtin ? (settings.apiKeys[model.id] ?? '') : model.apiKey) : '',
        model: model?.model ?? modelId,
        messages: contextMessages,
        params: settings.modelParams,
        mode,
        signal: abortController!.signal,
      },
      callbacks,
    ).then((handle) => {
      // 保存流句柄，确保「停止生成」按钮在桌面端能取消当前请求
      currentStream = handle;
    }).catch((e) => settle(() => reject(e)));
  });
}

/**
 * 工具调用循环：AI 回复后检测工具调用，执行并继续对话。
 *
 * 工作流程：
 * 1. 创建 AI 消息，调用 callAIOnce 获取流式回复
 * 2. 解析回复中的 ```tool:xxx``` 代码块
 * 3. 无工具调用 → 结束，通知用户
 * 4. 有工具调用 → 执行工具，将结果作为 system 消息注入上下文
 * 5. 回到步骤 1（最多 MAX_TOOL_ROUNDS 轮）
 */
async function runToolLoop(convId: string) {
  const store = useChatStore.getState();
  const persona = store.personas.find((p) => p.id === store.currentPersonaId);
  // 实时获取最新对话（避免过期快照覆盖标题等字段）
  const conv = store.conversations.find((c) => c.id === convId);
  const baseWorkdir = persona?.workdir ?? conv?.workdir ?? '';
  // 从最后一条用户消息提取路径
  const lastUser = [...store.messages].reverse().find((m) => m.role === 'user');
  const guessedDir = lastUser ? guessWorkdirFromMessage(lastUser.content) : '';
  const workdir = guessedDir || baseWorkdir;

  for (let round = 0; round < MAX_TOOL_ROUNDS; round++) {
    const aiMsgId = uid();
    const aiMsg: ChatMessage = {
      id: aiMsgId, convId, role: 'assistant', content: '',
      createdAt: Date.now(), streaming: true,
    };
    await db.upsertMessage(aiMsg);
    useChatStore.setState((s) => ({
      messages: [...s.messages, aiMsg],
      totalMessages: s.totalMessages + 1,
      generating: true,
    }));

    let acc = '';
    let reasoningAcc = '';
    try {
      const result = await callAIOnce(convId, aiMsgId);
      acc = result.content;
      reasoningAcc = result.reasoning;
    } catch (e) {
      // Error in AI call — display error, don't parse tool calls
      // 容错处理：Tauri invoke 的 rejection 可能没有 message 属性
      const errMsg = ((e as Error | undefined)?.message ?? String(e ?? '')) || '未知错误';
      acc = `\n\n> ⚠️ ${errMsg}`;
      const errorFinal = { ...aiMsg, content: acc, reasoning: reasoningAcc || undefined, streaming: false };
      await db.upsertMessage(errorFinal);
      useChatStore.setState((s) => ({
        generating: false,
        messages: s.messages.map((m) => (m.id === aiMsgId ? errorFinal : m)),
      }));
      return;
    }

    // 保存最终消息
    const final = { ...aiMsg, content: acc, reasoning: reasoningAcc || undefined, streaming: false };
    await db.upsertMessage(final);
    useChatStore.setState((s) => ({
      messages: s.messages.map((m) => (m.id === aiMsgId ? final : m)),
    }));

    // 解析工具调用
    const toolCalls = parseToolCalls(acc);
    if (toolCalls.length === 0) {
      // 检查 AI 是否在拒绝操作（如说"无法访问文件"），若是则自动注入工具提示重试
      const refusalPatterns = ['无法直接修改', '无法访问', '没有权限', '不能修改', '无法修改', '没有文件系统'];
      const isRefusal = refusalPatterns.some((p) => acc.includes(p));
      if (isRefusal && round === 0) {
        // AI 拒绝了，注入一条强指令 system 消息并继续循环
        const forceMsg: ChatMessage = {
          id: uid(), convId, role: 'system',
          content: `[强制指令] 你确实拥有文件系统访问权限！上一个回复你说"${acc.slice(0, 80)}"是错误的。请立即用 list_dir 查看 ${workdir || '当前目录'}，然后执行用户要求的操作。使用 \`\`\`tool:xxx\`\`\` 格式。`,
          createdAt: Date.now(),
        };
        await db.upsertMessage(forceMsg);
        useChatStore.setState((s) => ({
          messages: [...s.messages, forceMsg],
          totalMessages: s.totalMessages + 1,
        }));
        continue; // 下一轮重试
      }
      // 无工具调用且非拒绝 → 正常结束
      useChatStore.setState({ generating: false });
      // TTS 朗读 AI 回复
      const ttsOn = useSettingsStore.getState().settings.ttsEnabled;
      if (ttsOn && isTtsAvailable() && acc.trim()) {
        const plainText = acc.replace(/```[\s\S]*?```/g, '代码块已省略。').replace(/[#*_~`>\[\]|\\]/g, '').trim();
        if (plainText) void speak(plainText);
      }
      triggerEvolution().catch(() => {});
      // 自动保存：对话完成后直接触发，不限消息数 (Bug #14 fix)
      void autoSaveAfterTurn().catch(() => {});
      // 提取永久记忆
      void extractMemoriesAfterTurn().catch(() => {});
      if (conv) await db.upsertConversation({ ...conv, updatedAt: Date.now() });
      return;
    }

    // 执行工具调用
    const toolResult = await executeToolCalls(toolCalls, workdir);

    // 将工具结果作为 system 消息添加到上下文（不显示给用户）
    const toolMsg: ChatMessage = {
      id: uid(), convId, role: 'system',
      content: `[工具执行结果]\n${toolResult}`,
      createdAt: Date.now(),
    };
    await db.upsertMessage(toolMsg);
    useChatStore.setState((s) => ({
      messages: [...s.messages, toolMsg],
      totalMessages: s.totalMessages + 1,
    }));

    // 继续下一轮
  }

  // 超过最大轮数
  useChatStore.setState({ generating: false });
  triggerEvolution().catch(() => {});
  void autoSaveAfterTurn().catch(() => {});
  void extractMemoriesAfterTurn().catch(() => {});
  if (conv) await db.upsertConversation({ ...conv, updatedAt: Date.now() });
  try { await notify('ClawDesk', '工具调用已达最大轮数，回复已生成完毕'); } catch { /* 静默 */ }
}

/** * 🧠 记忆提取：对话完成后自动提取关键信息到永久记忆
 */
async function extractMemoriesAfterTurn() {
  setTimeout(async () => {
    try {
      const store = useChatStore.getState();
      const msgs = store.messages;
      if (msgs.length < 4) return; // 至少4条消息才提取
      const { useMemoryStore } = await import('./useMemoryStore');
      // 桌面端必须走 llmStream（Rust 后端绕过 CORS），浏览器端自动降级 streamChat
      const { llmStream } = await import('@/lib/backend');
      const settings = useSettingsStore.getState().settings;
      const modelId = settings.defaultModelId;
      const allModels = useSettingsStore.getState().allModels();
      const flash = allModels.find(m => m.id === 'deepseek-v4-flash') ?? allModels[0];
      if (!flash) return;

      // 用 Flash 做轻量提取，省钱
      const llmCall = async (prompt: string): Promise<string> => {
        return new Promise((resolve) => {
          let result = '';
          void llmStream(
            {
              apiBase: flash.apiBase,
              apiKey: settings.apiKeys[flash.id] ?? (flash as { apiKey?: string }).apiKey ?? '',
              model: flash.model,
              messages: [{ role: 'user', content: prompt }],
              params: { temperature: 0.2, maxTokens: 1024, topP: 0.9 },
              mode: 'fast',
              signal: new AbortController().signal,
            },
            { onDelta: (t: string) => { result += t; }, onDone: () => resolve(result), onError: () => resolve('') }
          );
          setTimeout(() => resolve(result), 15000); // 15s timeout
        });
      };

      await useMemoryStore.getState().extract(msgs, store.currentConvId, llmCall);
    } catch {
      // 静默失败
    }
  }, 3000);
}

/** * � 自动保存：每次对话完成后立即触发，不依赖进化引擎。
 * 不受消息数限制，确保短对话也能保存。
 */
async function autoSaveAfterTurn() {
  setTimeout(async () => {
    try {
      const store = useChatStore.getState();
      const msgs = store.messages;
      if (msgs.length < 2) return;
      const { autoSaveConversation } = await import('@/lib/autoSave');
      const settings = useSettingsStore.getState().settings;
      if (settings.autoSaveChat && settings.savePath) {
        const conv = store.conversations.find(c => c.id === store.currentConvId);
        await autoSaveConversation(conv, msgs, settings.savePath);
      }
    } catch {
      // 静默失败，不影响主流程
    }
  }, 2000);
}

/**
 * �🔄 自我进化触发器：对话正常结束后，提取经验并自我反思。
 * 仅在消息数 >= 3 且有有效 AI 回复时触发，避免空对话浪费 API。
 */
async function triggerEvolution() {
  const store = useChatStore.getState();
  const msgs = store.messages;
  if (msgs.length < 3) return; // 太短的对话不提取经验

  // 自动进化开关：关闭则跳过（节省LLM额度）
  const { useSettingsStore } = await import('./useSettingsStore');
  if (!useSettingsStore.getState().settings.autoEvolve) return;

  // 延迟执行，不阻塞用户交互
  setTimeout(async () => {
    try {
      const { useEvolutionStore } = await import('./useEvolutionStore');
      await useEvolutionStore.getState().load(); // 确保已加载
      await useEvolutionStore.getState().runEvolve(msgs);
    } catch {
      // 静默失败，不影响主流程
    }

    // 🔷 自动保存对话到本地数据库
    try {
      const { autoSaveConversation } = await import('@/lib/autoSave');
      const settings = useSettingsStore.getState().settings;
      if (settings.autoSaveChat && settings.savePath) {
        const conv = store.conversations.find(c => c.id === store.currentConvId);
        const result = await autoSaveConversation(conv, msgs, settings.savePath);
        if (result) {
          try { await notify('💾 已保存', result); } catch { /* 静默 */ }
        }
      }
    } catch {
      // 保存失败不影响主流程
    }
  }, 2000);
}
