/**
 * AI Todo 状态 — AI 通过 tool:todo 管理待办事项。
 * 全局共享，多个对话可访问同一份 Todo 列表。
 */
import { create } from 'zustand';

export interface TodoItem {
  id: number;
  text: string;
  done: boolean;
  createdAt: number;
}

interface TodoState {
  items: TodoItem[];
  nextId: number;
  add: (texts: string[]) => void;
  done: (id: number) => void;
  clear: () => void;
  /** AI 工具调用返回格式化文本 */
  handleToolCall: (params: Record<string, string>) => string;
}

export const useTodoStore = create<TodoState>((set, get) => ({
  items: [],
  nextId: 0,

  add: (texts) => {
    const { items, nextId } = get();
    const newItems = texts
      .filter(t => t.trim())
      .map((text, i) => ({
        id: nextId + i,
        text: text.trim().slice(0, 200),
        done: false,
        createdAt: Date.now(),
      }));
    set({ items: [...items, ...newItems], nextId: nextId + newItems.length });
  },

  done: (id) => {
    set(s => ({
      items: s.items.map(item =>
        item.id === id ? { ...item, done: true } : item
      ),
    }));
  },

  clear: () => set({ items: [], nextId: 0 }),

  handleToolCall: (params) => {
    const action = params.action || 'add';
    switch (action) {
      case 'add':
      case 'create': {
        const texts = (params.items || params.text || '')
          .split(/\n|\\n/)
          .map(s => s.replace(/^[-*\d.]+\s*/, '').trim())
          .filter(Boolean);
        if (texts.length === 0) return '❌ 请提供待办事项内容 (items)';
        get().add(texts);
        const pending = get().items.filter(i => !i.done).length;
        return `✅ 已添加 ${texts.length} 项待办（共 ${pending} 项未完成）`;
      }
      case 'done':
      case 'complete': {
        const id = parseInt(params.id || '0', 10);
        const item = get().items.find(i => i.id === id);
        if (!item) return `❌ 未找到待办 #${id}`;
        get().done(id);
        const remaining = get().items.filter(i => !i.done).length;
        const next = get().items.find(i => !i.done);
        const nextHint = next ? `\n📌 下一项：#${next.id} ${next.text}` : '\n🎉 全部完成！';
        return `✅ 已完成 #${id} "${item.text}"（剩余 ${remaining} 项）${remaining > 0 ? nextHint : ''}`;
      }
      case 'list': {
        const all = get().items;
        if (all.length === 0) return '📋 暂无待办事项';
        return all.map(i =>
          `${i.done ? '✅' : '⬜'} #${i.id} ${i.text}`
        ).join('\n');
      }
      case 'clear':
        get().clear();
        return '🗑️ 已清空所有待办';
      default:
        return `❌ 未知操作: ${action}。支持: add, done, list, clear`;
    }
  },
}));
