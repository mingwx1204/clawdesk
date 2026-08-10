/**
 * Todo 面板 — 与截图一致：圆形单选 + 进度条 + 当前项高亮。
 * 显示在输入框上方，AI 自动管理待办。
 */
import { useState } from 'react';
import { useTodoStore } from '@/store/useTodoStore';
import { Check, ListTodo } from 'lucide-react';

export function TodoPanel() {
  const { items } = useTodoStore();
  const [expanded, setExpanded] = useState(true);

  if (items.length === 0) return null;

  const pending = items.filter(i => !i.done);
  const done = items.filter(i => i.done);
  const current = items.find(i => !i.done);
  const total = items.length;
  const doneCount = done.length;
  const allDone = pending.length === 0;

  return (
    <div className="shrink-0 border-t border-border/40 bg-card/50 px-4 py-2">
      {/* 折叠标题栏 */}
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex w-full items-center gap-2 text-xs"
      >
        <ListTodo className="h-3.5 w-3.5 text-primary" />
        <span className="font-medium">
          {allDone ? '🎉 全部完成' : `待办事项(${doneCount}/${total})`}
        </span>
        <div className="mx-2 h-1.5 flex-1 overflow-hidden rounded-full bg-muted">
          <div
            className="h-full rounded-full bg-primary transition-all duration-500"
            style={{ width: `${total > 0 ? (doneCount / total) * 100 : 0}%` }}
          />
        </div>
        <span className="text-[10px] text-muted-foreground">
          {total > 0 ? Math.round((doneCount / total) * 100) : 0}%
        </span>
      </button>

      {/* 展开列表 */}
      {expanded && (
        <div className="mt-2 space-y-0.5">
          {items.map(item => {
            const isCurrent = item.id === current?.id;
            const isDone = item.done;
            return (
              <div
                key={item.id}
                className={`flex items-start gap-2 rounded-md px-2 py-1.5 text-[12px] transition-colors ${
                  isCurrent
                    ? 'bg-primary/10 ring-1 ring-primary/20'
                    : isDone
                    ? 'text-muted-foreground/50'
                    : 'hover:bg-muted/50'
                }`}
              >
                {/* 圆形单选指示器 */}
                <span className="mt-0.5 shrink-0">
                  {isDone ? (
                    <span className="flex h-4 w-4 items-center justify-center rounded-full bg-green-500/20">
                      <Check className="h-2.5 w-2.5 text-green-400" />
                    </span>
                  ) : isCurrent ? (
                    <span className="flex h-4 w-4 items-center justify-center rounded-full border-2 border-primary">
                      <span className="h-1.5 w-1.5 rounded-full bg-primary" />
                    </span>
                  ) : (
                    <span className="flex h-4 w-4 items-center justify-center rounded-full border-2 border-muted-foreground/30" />
                  )}
                </span>
                <span className={isDone ? 'line-through' : isCurrent ? 'font-medium' : ''}>
                  {item.text}
                </span>
                {isCurrent && (
                  <span className="ml-auto shrink-0 rounded bg-primary/20 px-1.5 py-0.5 text-[10px] text-primary">
                    进行中
                  </span>
                )}
                {isDone && (
                  <span className="ml-auto shrink-0 rounded bg-green-500/10 px-1.5 py-0.5 text-[10px] text-green-400">
                    已完成
                  </span>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
