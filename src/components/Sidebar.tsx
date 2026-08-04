import { memo, useEffect, useMemo, useState } from 'react';
import { List, type RowComponentProps } from 'react-window';
import { Plus, Search, Pin, PanelLeftClose, PanelLeftOpen, Bot, Pencil, Trash2, Download, PinOff } from 'lucide-react';
import { useChatStore } from '@/store/useChatStore';
import { useWorkspaceStore } from '@/store/useWorkspaceStore';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import {
  ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger,
} from '@/components/ui/context-menu';
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import type { Conversation } from '@/types';
import { cn } from '@/lib/utils';

function formatTime(ts: number): string {
  const d = new Date(ts);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  if (sameDay) return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  return d.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' });
}

/** 防抖搜索输入 */
function useDebounced<T>(value: T, delay = 250): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), delay);
    return () => clearTimeout(t);
  }, [value, delay]);
  return v;
}

const ConvItem = memo(function ConvItem({
  conv, active, onSelect,
}: {
  conv: Conversation; active: boolean; onSelect: () => void;
}) {
  const { renameConversation, togglePin, removeConversation, exportConversation } = useChatStore();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(conv.title);

  const commitRename = () => {
    setEditing(false);
    if (draft.trim() && draft !== conv.title) void renameConversation(conv.id, draft.trim());
  };

  const doExport = async (format: 'md' | 'json') => {
    const content = await exportConversation(conv.id, format);
    const blob = new Blob([content], { type: 'text/plain;charset=utf-8' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `${conv.title}.${format}`;
    a.click();
    URL.revokeObjectURL(a.href);
  };

  return (
    <ContextMenu>
      <ContextMenuTrigger>
        <div
          className={cn(
            'group mx-2 mb-0.5 cursor-pointer rounded-lg px-3 py-2 transition-colors animate-slide-right-in',
            active ? 'bg-accent' : 'hover:bg-accent/50',
          )}
          onClick={onSelect}
          onDoubleClick={() => { setDraft(conv.title); setEditing(true); }}
        >
          <div className="flex items-center gap-1.5">
            {conv.pinned && <Pin className="h-3 w-3 shrink-0 text-primary" />}
            {editing ? (
              <input
                className="w-full rounded bg-background px-1 text-sm outline-none ring-1 ring-primary"
                value={draft}
                autoFocus
                onChange={(e) => setDraft(e.target.value)}
                onBlur={commitRename}
                onKeyDown={(e) => { if (e.key === 'Enter') commitRename(); if (e.key === 'Escape') setEditing(false); }}
                onClick={(e) => e.stopPropagation()}
              />
            ) : (
              <span className="flex-1 truncate text-sm">{conv.title}</span>
            )}
          </div>
          <div className="mt-0.5 flex items-center justify-between">
            <span className="text-[11px] text-muted-foreground">{formatTime(conv.updatedAt)}</span>
            {conv.modelId && (
              <span className="rounded bg-secondary px-1.5 py-0.5 text-[10px] text-secondary-foreground">{conv.modelId}</span>
            )}
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onClick={() => { setDraft(conv.title); setEditing(true); }}>
          <Pencil className="mr-2 h-3.5 w-3.5" /> 重命名
        </ContextMenuItem>
        <ContextMenuItem onClick={() => void togglePin(conv.id)}>
          {conv.pinned ? <PinOff className="mr-2 h-3.5 w-3.5" /> : <Pin className="mr-2 h-3.5 w-3.5" />}
          {conv.pinned ? '取消置顶' : '置顶'}
        </ContextMenuItem>
        <ContextMenuItem onClick={() => void doExport('md')}>
          <Download className="mr-2 h-3.5 w-3.5" /> 导出 Markdown
        </ContextMenuItem>
        <ContextMenuItem onClick={() => void doExport('json')}>
          <Download className="mr-2 h-3.5 w-3.5" /> 导出 JSON
        </ContextMenuItem>
        <ContextMenuItem className="text-red-500" onClick={() => void removeConversation(conv.id)}>
          <Trash2 className="mr-2 h-3.5 w-3.5" /> 删除
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
});

/** 虚拟滚动列表行（react-window v2：rowProps 传递数据） */
interface RowData {
  items: Conversation[];
  currentId: string;
  select: (id: string) => void;
}

function Row({ index, style, items, currentId, select }: RowComponentProps<RowData>) {
  const conv = items[index];
  return (
    <div style={style}>
      <ConvItem conv={conv} active={conv.id === currentId} onSelect={() => select(conv.id)} />
    </div>
  );
}

export const Sidebar = memo(function Sidebar() {
  const {
    conversations, currentConvId, personas, currentPersonaId,
    newConversation, selectConversation, selectPersona, search, searchQuery, totalMessages,
  } = useChatStore();
  const { sidebarCollapsed, toggleSidebar } = useWorkspaceStore();
  const [keyword, setKeyword] = useState('');
  const debounced = useDebounced(keyword);

  // 防抖后做本地标题过滤（列表过滤放内存，消息全文搜索走 Worker/DB）
  const filtered = useMemo(() => {
    const kw = debounced.trim().toLowerCase();
    if (!kw) return conversations;
    return conversations.filter((c) => c.title.toLowerCase().includes(kw));
  }, [conversations, debounced]);

  const currentPersona = personas.find((p) => p.id === currentPersonaId);
  const ITEM_H = 62;
  const useVirtual = filtered.length > 100;

  // 总是渲染展开版侧边栏，通过 CSS 宽度过渡实现收起/展开动画
  return (
    <div
      className={cn(
        'flex shrink-0 flex-col border-r border-border/40 mica overflow-hidden transition-all duration-300',
        sidebarCollapsed ? 'w-8' : 'w-[280px]',
      )}
    >
      {/* 顶部：收起 + 标题 */}
      <div className="flex items-center gap-2 p-2">
        <Button variant="ghost" size="icon" className="h-7 w-7 shrink-0" onClick={toggleSidebar} title={sidebarCollapsed ? '展开面板' : '收起面板'}>
          {sidebarCollapsed ? <PanelLeftOpen className="h-4 w-4" /> : <PanelLeftClose className="h-4 w-4" />}
        </Button>
        {!sidebarCollapsed && (
          <span className="flex-1 text-sm font-bold tracking-wide text-muted-foreground">ClawDesk 记忆</span>
        )}
      </div>
      {!sidebarCollapsed && (
        <div className="px-2 pb-2">
          <div className="relative">
            <Search className="absolute left-2.5 top-2.5 h-3.5 w-3.5 text-muted-foreground" />
            <Input
              className="h-8 rounded-lg pl-8 text-xs"
              placeholder="搜索记忆…"
              value={keyword}
              onChange={(e) => { setKeyword(e.target.value); void search(e.target.value); }}
            />
          </div>
          {searchQuery && (
            <p className="mt-1 px-1 text-[10px] text-muted-foreground">消息全文搜索结果显示在主区域</p>
          )}
        </div>
      )}

      {!sidebarCollapsed && (
        <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
          <div className={cn('rounded-lg px-3 py-2 transition-colors', currentConvId ? 'bg-accent' : 'hover:bg-accent/50')}>
            <p className="text-sm font-medium">全部记忆</p>
            <p className="text-[11px] text-muted-foreground">{totalMessages} 条消息 · 永不遗忘</p>
          </div>
          <p className="mt-3 px-2 text-[10px] text-muted-foreground/50">所有对话自动存储为永久记忆</p>
        </div>
      )}
    </div>
  );
});
