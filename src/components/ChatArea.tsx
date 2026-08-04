import { memo, useCallback, useEffect, useRef, useState } from 'react';
import { ArrowDown, BarChart3, Download, FolderOpen, FolderTree, Layers, Pencil, Settings, SquareTerminal } from 'lucide-react';
import { useChatStore } from '@/store/useChatStore';
import { useWorkspaceStore } from '@/store/useWorkspaceStore';
import { useSettingsStore } from '@/store/useSettingsStore';
import { MessageItem } from './MessageItem';
import { ContextViewerDialog } from './ContextViewerDialog';
import { ProjectProgressDialog } from './ProjectProgressDialog';
import { openPathInExplorer } from '@/lib/backend';
import { autoSaveConversation } from '@/lib/autoSave';
import { playOutputTick } from '@/lib/sound';
import type { Attachment } from '@/types';

/** 对话主区域：顶部工具栏 + 消息列表（懒加载 + 回底按钮 + 拖拽发送） */
export const ChatArea = memo(function ChatArea({ onOpenSettings, onToggleWorkspace }: { onOpenSettings: () => void; onToggleWorkspace: () => void }) {
  const { messages, currentConvId, conversations, renameConversation, loadOlderMessages, totalMessages, send } = useChatStore();
  const { toggleFilePanel, toggleTerminal, workdir, setWorkdir } = useWorkspaceStore();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [showJump, setShowJump] = useState(false);
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState('');
  const [dragOver, setDragOver] = useState(false);
  const [ctxOpen, setCtxOpen] = useState(false);
  const [progressOpen, setProgressOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const stickToBottom = useRef(true);
  const prevLen = useRef(0);
  const lastContentLen = useRef(0);

  const conv = conversations.find((c) => c.id === currentConvId);

  // 轻量滚动触发器：只监听长度变化，避免流式输出时每次都重建 effect
  useEffect(() => {
    const len = messages.length;
    if (len !== prevLen.current) {
      prevLen.current = len;
      if (stickToBottom.current) scrollToBottom(false);
      return;
    }
    // 流式增长：仅检查最后一条消息长度
    if (len > 0 && stickToBottom.current) {
      const last = messages[len - 1];
      const curLen = last.content.length + (last.reasoning?.length ?? 0);
      if (curLen !== lastContentLen.current) {
        lastContentLen.current = curLen;
        scrollToBottom(false);        // 输出音效（流式进行中，每 5 字符轻柔咔嗒）
        if (useSettingsStore.getState().settings.soundEnabled && curLen % 5 === 0) {
          playOutputTick();
        }      }
    }
  }, [messages.length, messages[messages.length - 1]?.content.length, messages[messages.length - 1]?.reasoning?.length]);// eslint-disable-line

  const scrollToBottom = useCallback((smooth = true) => {
    const el = scrollRef.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' });
  }, []);

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    stickToBottom.current = nearBottom;
    setShowJump(!nearBottom && el.scrollHeight > el.clientHeight + 400);
    // 滚动到顶部 -> 懒加载更早的 50 条
    if (el.scrollTop < 60 && messages.length < totalMessages) {
      const prevHeight = el.scrollHeight;
      void loadOlderMessages().then(() => {
        // 保持视口位置不跳
        requestAnimationFrame(() => {
          el.scrollTop = el.scrollHeight - prevHeight;
        });
      });
    }
  }, [messages.length, totalMessages, loadOlderMessages]);

  // 拖拽文件到对话区 -> 作为附件发送
  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setDragOver(false);
      const files = Array.from(e.dataTransfer.files);
      if (files.length === 0) return;
      void (async () => {
        const atts: Attachment[] = [];
        for (const f of files.slice(0, 5)) {
          if (f.type.startsWith('image/')) {
            const dataUrl = await new Promise<string>((res) => {
              const r = new FileReader();
              r.onload = () => res(r.result as string);
              r.readAsDataURL(f);
            });
            atts.push({ kind: 'image', name: f.name, data: dataUrl, mime: f.type });
          } else {
            atts.push({ kind: 'file', name: f.name, data: (f as File & { path?: string }).path ?? f.name, mime: f.type });
          }
        }
        await send(`发送了 ${atts.length} 个附件`, atts);
      })();
    },
    [send],
  );

  // 同步当前对话的工作目录到工作区
  useEffect(() => {
    if (conv?.workdir && conv.workdir !== workdir) void setWorkdir(conv.workdir);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conv?.workdir]);

  const commitTitle = () => {
    setEditingTitle(false);
    if (conv && titleDraft.trim()) void renameConversation(conv.id, titleDraft.trim());
  };

  return (
    <div
      className="relative flex min-w-0 flex-1 flex-col"
      onDragOver={(e) => { e.preventDefault(); setDragOver(true); }}
      onDragLeave={() => setDragOver(false)}
      onDrop={onDrop}
    >
      {/* 顶部工具栏 */}
      <div className="flex h-11 shrink-0 items-center gap-1 border-b border-border bg-card/60 backdrop-blur px-3">
        {editingTitle ? (
          <input
            className="h-7 rounded bg-muted px-2 text-sm outline-none ring-1 ring-primary"
            value={titleDraft}
            autoFocus
            onChange={(e) => setTitleDraft(e.target.value)}
            onBlur={commitTitle}
            onKeyDown={(e) => { if (e.key === 'Enter') commitTitle(); if (e.key === 'Escape') setEditingTitle(false); }}
          />
        ) : (
          <button
            className="group flex items-center gap-1.5 rounded px-1.5 py-1 text-sm font-medium hover:bg-accent"
            onClick={() => { setTitleDraft(conv?.title ?? ''); setEditingTitle(true); }}
            title="点击编辑标题"
          >
            <span className="max-w-64 truncate">全部记忆</span>
            <Pencil className="h-3 w-3 text-muted-foreground opacity-0 group-hover:opacity-100" />
          </button>
        )}
        <div className="flex-1" />
        <div className="flex items-center gap-1.5">
          <button className="flex items-center gap-1.5 rounded-lg bg-accent/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground" onClick={() => setCtxOpen(true)} title="上下文查看">
            <Layers className="h-3.5 w-3.5" /> 上下文
          </button>
          <button className="flex items-center gap-1.5 rounded-lg bg-accent/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground" onClick={onToggleWorkspace} title="工作区">
            <BarChart3 className="h-3.5 w-3.5" /> 工作区
          </button>
          <button className="flex items-center gap-1.5 rounded-lg bg-accent/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:opacity-30" disabled={!workdir} onClick={() => void openPathInExplorer(workdir)} title="打开工作文件夹">
            <FolderOpen className="h-3.5 w-3.5" /> 文件夹
          </button>
          <button className="flex items-center gap-1.5 rounded-lg bg-accent/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground" onClick={toggleFilePanel} title="文件夹树">
            <FolderTree className="h-3.5 w-3.5" /> 文件树
          </button>
          <button className="flex items-center gap-1.5 rounded-lg bg-accent/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:opacity-40" disabled={saving || messages.length === 0} onClick={async () => {
            const { settings } = useSettingsStore.getState();
            setSaving(true);
            try {
              const result = await autoSaveConversation(conv, messages, settings.savePath || 'D:\\数据库');
              if (result) console.log('[保存]', result);
            } finally {
              setSaving(false);
            }
          }} title="保存对话到本地">
            <Download className={saving ? 'h-3.5 w-3.5 animate-bounce' : 'h-3.5 w-3.5'} /> {saving ? '保存中' : '保存'}
          </button>
          <button className="flex items-center gap-1.5 rounded-lg bg-accent/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground" onClick={toggleTerminal} title="终端">
            <SquareTerminal className="h-3.5 w-3.5" /> 终端
          </button>
          <button className="flex items-center gap-1.5 rounded-lg bg-accent/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground" onClick={onOpenSettings} title="设置">
            <Settings className="h-3.5 w-3.5" /> 设置
          </button>
        </div>
      </div>

      {/* 消息区 */}
      <div ref={scrollRef} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto pb-8 pt-2">
        {messages.length < totalMessages && (
          <p className="py-2 text-center text-xs text-muted-foreground">上滑加载更早的消息…</p>
        )}
        {messages.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
            <span className="text-4xl">🐾</span>
            <p className="text-sm">开始新的对话吧</p>
          </div>
        ) : (
          messages.map((m) => <MessageItem key={m.id} msg={m} />)
        )}
      </div>

      {/* 回到底部按钮 */}
      {showJump && (
        <button
          className="absolute bottom-24 right-6 z-10 flex h-9 w-9 items-center justify-center rounded-full border border-border bg-card shadow-lg hover:bg-accent"
          onClick={() => { stickToBottom.current = true; scrollToBottom(); }}
          title="滚动到底部"
        >
          <ArrowDown className="h-4 w-4" />
        </button>
      )}

      {/* 拖拽提示遮罩 */}
      {dragOver && (
        <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center rounded-lg border-2 border-dashed border-primary bg-primary/10">
          <p className="text-sm font-medium text-primary">松开以发送文件</p>
        </div>
      )}

      <ContextViewerDialog open={ctxOpen} onClose={() => setCtxOpen(false)} />
      <ProjectProgressDialog open={progressOpen} onClose={() => setProgressOpen(false)} />
    </div>
  );
});
