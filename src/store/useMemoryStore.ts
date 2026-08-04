/**
 * 永久记忆状态管理 v2
 * 集成：重要性评分 · 记忆图谱 · 记忆合并 · HyDE检索
 */
import { create } from 'zustand';
import type { Memory, ChatMessage } from '@/types';
import { upsertMemory, listMemories, deleteMemory, searchMemories } from '@/lib/db';
import {
  extractMemories, extractKeywordsLocal, findRelevantMemories, injectMemories,
  findRelevantMemoriesHyde, scoreImportance, getTopMemories,
  buildMemoryGraph, findConsolidationCandidates,
  generateDailySummary, getStaleMemories,
  type MemoryEdge, type ConsolidationCandidate,
} from '@/lib/memory';
import { uid } from '@/components/settings/BalanceChecker';

interface MemoryState {
  memories: Memory[];
  loaded: boolean;

  load: () => Promise<void>;
  extract: (messages: ChatMessage[], convId: string, llmCall: (p: string) => Promise<string>) => Promise<void>;
  remove: (id: string) => Promise<void>;
  clearAll: () => Promise<void>;

  /** 获取与当前对话相关的记忆注入文本（基础版） */
  getContext: (userMessage: string, baseSystemPrompt: string, currentConvId?: string) => string;
  /** HyDE增强检索——生成假设答案轮廓再匹配记忆 */
  getContextHyde: (userMessage: string, baseSystemPrompt: string, currentConvId?: string) => string;
  /** 记录某条记忆被使用 */
  touch: (id: string) => Promise<void>;

  /* ─── v2 新增 ─── */
  /** 按重要性排序的记忆 */
  getTopMemories: (topK?: number) => Memory[];
  /** 构建记忆关联图谱 */
  getGraph: () => { nodes: { id: string; label: string; importance: number }[]; edges: MemoryEdge[] };
  /** 查找可合并的记忆对 */
  getConsolidationCandidates: () => ConsolidationCandidate[];
  /** 执行记忆合并（删除旧记忆，创建新记忆） */
  consolidate: (candidate: ConsolidationCandidate) => Promise<void>;
  /** 每日摘要 */
  getDailySummary: () => ReturnType<typeof generateDailySummary>;
  /** 获取建议清理的过期记忆 */
  getStale: (staleDays?: number) => Memory[];
  /** 按重要性清理过期记忆 */
  cleanStale: (staleDays?: number) => Promise<number>;
}

export const useMemoryStore = create<MemoryState>((set, get) => ({
  memories: [],
  loaded: false,

  load: async () => {
    const memories = await listMemories();
    set({ memories, loaded: true });
  },

  extract: async (messages, convId, llmCall) => {
    try {
      const items = await extractMemories(messages, llmCall);
      if (items.length === 0) return;

      const now = Date.now();
      const existing = get().memories;

      for (const item of items) {
        const similar = existing.find((m) =>
          m.content.slice(0, 30) === item.content.slice(0, 30)
        );
        if (similar) {
          const updated: Memory = {
            ...similar,
            content: item.content,
            keywords: [...new Set([...similar.keywords, ...item.keywords, ...extractKeywordsLocal(item.content)])],
            useCount: similar.useCount + 1,
            updatedAt: now,
          };
          await upsertMemory(updated);
        } else {
          const memory: Memory = {
            id: uid(),
            ...item,
            keywords: [...item.keywords, ...extractKeywordsLocal(item.content)],
            sourceConvId: convId,
            useCount: 1,
            createdAt: now,
            updatedAt: now,
          };
          await upsertMemory(memory);
        }
      }

      await get().load();
    } catch { /* 静默失败 */ }
  },

  remove: async (id) => {
    await deleteMemory(id);
    set((s) => ({ memories: s.memories.filter((m) => m.id !== id) }));
  },

  clearAll: async () => {
    const all = get().memories;
    for (const m of all) await deleteMemory(m.id);
    set({ memories: [] });
  },

  getContext: (userMessage, baseSystemPrompt, currentConvId) => {
    const memories = get().memories;
    if (memories.length === 0 || !userMessage) return baseSystemPrompt;
    const relevant = findRelevantMemories(memories, userMessage, 5, currentConvId);
    if (relevant.length === 0) return baseSystemPrompt;
    for (const m of relevant) void get().touch(m.id);
    return injectMemories(baseSystemPrompt, relevant, currentConvId);
  },

  /** HyDE增强检索：先构建假设答案轮廓，再匹配记忆 */
  getContextHyde: (userMessage, baseSystemPrompt, currentConvId) => {
    const memories = get().memories;
    if (memories.length === 0 || !userMessage) return baseSystemPrompt;
    
    const { memories: relevant, hydeInfo } = findRelevantMemoriesHyde(memories, userMessage, 8, currentConvId);
    if (relevant.length === 0) return baseSystemPrompt;
    
    for (const m of relevant) void get().touch(m.id);

    const currentMsgs = relevant.filter(m => m.sourceConvId === currentConvId);
    const historicalMsgs = relevant.filter(m => m.sourceConvId !== currentConvId);

    const memoryLines: string[] = [];
    if (currentMsgs.length > 0) {
      memoryLines.push('### 当前对话相关');
      for (const m of currentMsgs) {
        const imp = scoreImportance(m);
        const stars = imp > 5 ? '★★★' : imp > 3 ? '★★' : '★';
        memoryLines.push(`- ${stars} [${m.category}] ${m.content}`);
      }
    }
    if (historicalMsgs.length > 0) {
      memoryLines.push('### 历史长期记忆');
      for (const m of historicalMsgs) {
        const imp = scoreImportance(m);
        const stars = imp > 5 ? '★★★' : imp > 3 ? '★★' : '★';
        memoryLines.push(`- ${stars} [${m.category}] ${m.content}`);
      }
    }

    return `${baseSystemPrompt}

## 用户永久记忆（HyDE增强检索）
${hydeInfo.explanation}
${memoryLines.join('\n')}

注意：
- "当前对话相关"记忆优先级最高，与本次对话直接相关
- "历史长期记忆"是跨会话的知识/偏好，仅供参考
- ⚠️ 不要主动提及历史记忆中的"未完成任务"，除非用户明确询问
- 如果记忆与用户当前说法不一致，以用户当前说法为准。`;
  },

  touch: async (id) => {
    const memory = get().memories.find((m) => m.id === id);
    if (memory) {
      const updated = { ...memory, useCount: memory.useCount + 1, updatedAt: Date.now() };
      await upsertMemory(updated);
      set((s) => ({ memories: s.memories.map((m) => (m.id === id ? updated : m)) }));
    }
  },

  /* ─── v2 重要性 & 图谱 ─── */

  getTopMemories: (topK = 10) => getTopMemories(get().memories, topK),

  getGraph: () => buildMemoryGraph(get().memories),

  getConsolidationCandidates: () => findConsolidationCandidates(get().memories),

  consolidate: async (candidate) => {
    const { memories, mergedContent, mergedKeywords } = candidate;
    const now = Date.now();
    
    // 删除旧记忆
    for (const m of memories) {
      await deleteMemory(m.id);
    }
    
    // 创建合并后的新记忆
    const merged: Memory = {
      id: uid(),
      content: mergedContent,
      keywords: mergedKeywords,
      category: memories[0]?.category || 'knowledge',
      sourceConvId: '',
      useCount: memories.reduce((s, m) => s + m.useCount, 0),
      createdAt: now,
      updatedAt: now,
    };
    await upsertMemory(merged);
    await get().load();
  },

  getDailySummary: () => {
    const today = new Date().toLocaleDateString('zh-CN');
    const todayMs = get().memories.filter(
      (m) => new Date(m.createdAt).toLocaleDateString('zh-CN') === today
    );
    return generateDailySummary(todayMs);
  },

  getStale: (staleDays = 30) => getStaleMemories(get().memories, staleDays),

  cleanStale: async (staleDays = 30) => {
    const stale = getStaleMemories(get().memories, staleDays);
    for (const m of stale) {
      await deleteMemory(m.id);
    }
    set((s) => ({ memories: s.memories.filter(m => !stale.find(st => st.id === m.id)) }));
    return stale.length;
  },
}));
