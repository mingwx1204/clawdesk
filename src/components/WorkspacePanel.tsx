/**
 * 工作区侧边栏 — Todo 任务 + 文件变更追踪 + 接受/撤销
 * 从对话中提取任务项，追踪工具操作的文件变更
 */
import { useEffect, useState, useMemo } from 'react';
import { useChatStore } from '@/store/useChatStore';
import { CheckSquare, Square, FileEdit, RotateCcw, Check, X, PanelRightClose, ListTodo } from 'lucide-react';

interface TodoItem {
  id: string;
  text: string;
  done: boolean;
}

interface FileChange {
  path: string;
  action: 'wrote' | 'deleted' | 'renamed';
  timestamp: number;
}

export function WorkspacePanel({ onClose }: { onClose: () => void }) {
  const { messages } = useChatStore();
  const [todos, setTodos] = useState<TodoItem[]>([]);

  // 从最近消息中提取待办事项
  useEffect(() => {
    const items: TodoItem[] = [];
    let id = 0;
    for (const msg of [...messages].reverse()) {
      if (msg.role !== 'user') continue;
      // 匹配：数字列表、任务标记
      const lines = msg.content.split('\n');
      for (const line of lines) {
        const todoMatch = line.match(/^[-*]\s*\[([ x])\]\s+(.+)/) || // - [ ] task
          line.match(/^(\d+)[.、)]\s*(.+)/) || // 1. task
          line.match(/^[-*]\s+(TODO|待做|任务)[:：]\s*(.+)/i); // - TODO: task
        if (todoMatch) {
          items.push({ id: `todo-${id++}`, text: (todoMatch[2] || todoMatch[1] || '').trim().slice(0, 100), done: false });
        }
      }
      if (items.length > 0) break;
    }
    setTodos(items);
  }, [messages]);

  // 从工具执行中提取文件变更
  const fileChanges = useMemo(() => {
    const changes: FileChange[] = [];
    for (const msg of messages) {
      if (msg.role !== 'assistant') continue;
      // 匹配写入文件
      const wroteMatch = msg.content.match(/已写入[：:]\s*(.+)/);
      if (wroteMatch) {
        changes.push({ path: wroteMatch[1].trim(), action: 'wrote', timestamp: msg.createdAt });
      }
      // 匹配删除文件
      const delMatch = msg.content.match(/已删除[：:]\s*(.+)/);
      if (delMatch) {
        changes.push({ path: delMatch[1].trim(), action: 'deleted', timestamp: msg.createdAt });
      }
    }
    return changes.slice(-10).reverse();
  }, [messages]);

  // 对话统计
  const stats = useMemo(() => {
    const userMsgs = messages.filter(m => m.role === 'user').length;
    const aiMsgs = messages.filter(m => m.role === 'assistant').length;
    const totalChars = messages.reduce((sum, m) => sum + m.content.length, 0);
    return { userMsgs, aiMsgs, totalChars };
  }, [messages]);

  return (
    <div className="flex h-full w-64 shrink-0 flex-col border-l border-border/40 bg-card/50">
      {/* 标题栏 */}
      <div className="flex items-center justify-between border-b border-border/40 px-3 py-2">
        <span className="flex items-center gap-1.5 text-xs font-medium">
          <ListTodo className="h-3.5 w-3.5" />
          工作区
        </span>
        <button onClick={onClose} className="rounded p-0.5 hover:bg-muted">
          <PanelRightClose className="h-3.5 w-3.5 text-muted-foreground" />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-3 py-2 space-y-4">
        {/* 对话统计 */}
        <div className="grid grid-cols-3 gap-1 text-center text-[10px]">
          <div className="rounded bg-muted p-1">
            <div className="font-medium text-foreground">{stats.userMsgs}</div>
            <div className="text-muted-foreground">用户消息</div>
          </div>
          <div className="rounded bg-muted p-1">
            <div className="font-medium text-foreground">{stats.aiMsgs}</div>
            <div className="text-muted-foreground">AI 回复</div>
          </div>
          <div className="rounded bg-muted p-1">
            <div className="font-medium text-foreground">{(stats.totalChars / 1000).toFixed(0)}k</div>
            <div className="text-muted-foreground">总字符</div>
          </div>
        </div>

        {/* 待办事项 */}
        <div>
          <h3 className="mb-1.5 flex items-center gap-1 text-[11px] font-medium text-muted-foreground">
            <CheckSquare className="h-3 w-3" />
            待办事项
            <span className="ml-auto text-[10px]">{todos.filter(t => t.done).length}/{todos.length}</span>
          </h3>
          {todos.length === 0 ? (
            <p className="text-[10px] text-muted-foreground">在对话中使用数字列表或 - [ ] 格式创建待办</p>
          ) : (
            <div className="space-y-0.5">
              {todos.map(todo => (
                <button
                  key={todo.id}
                  onClick={() => setTodos(prev => prev.map(t => t.id === todo.id ? { ...t, done: !t.done } : t))}
                  className="flex w-full items-start gap-1.5 rounded px-1 py-0.5 text-left text-[11px] hover:bg-muted/50"
                >
                  {todo.done ? (
                    <Check className="mt-0.5 h-3 w-3 shrink-0 text-green-400" />
                  ) : (
                    <Square className="mt-0.5 h-3 w-3 shrink-0 text-muted-foreground" />
                  )}
                  <span className={todo.done ? 'line-through text-muted-foreground' : ''}>{todo.text}</span>
                </button>
              ))}
            </div>
          )}
        </div>

        {/* 文件变更 */}
        <div>
          <h3 className="mb-1.5 flex items-center gap-1 text-[11px] font-medium text-muted-foreground">
            <FileEdit className="h-3 w-3" />
            文件变更
            <span className="ml-auto text-[10px]">{fileChanges.length}</span>
          </h3>
          {fileChanges.length === 0 ? (
            <p className="text-[10px] text-muted-foreground">暂无文件操作记录</p>
          ) : (
            <div className="space-y-0.5">
              {fileChanges.map((fc, i) => (
                <div key={i} className="flex items-center gap-1.5 rounded px-1 py-0.5 text-[10px]">
                  <span className={`shrink-0 ${fc.action === 'wrote' ? 'text-green-400' : 'text-red-400'}`}>
                    {fc.action === 'wrote' ? '+' : fc.action === 'deleted' ? '×' : '~'}
                  </span>
                  <span className="truncate text-muted-foreground">{fc.path.replace(/^.*[/\\]/, '')}</span>
                  <span className="ml-auto shrink-0 text-muted-foreground">
                    {new Date(fc.timestamp).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
