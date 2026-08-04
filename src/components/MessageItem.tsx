import { memo, useState } from 'react';
import { Brain, Check, ChevronDown, ChevronRight, Copy, RefreshCw, Trash2 } from 'lucide-react';
import type { ChatMessage } from '@/types';
import { MarkdownView } from './MarkdownView';
import { useChatStore } from '@/store/useChatStore';
import { cn } from '@/lib/utils';

/** 思考过程（推理模型的 reasoning_content）：可折叠区块 */
function ReasoningBlock({ reasoning, streaming }: { reasoning: string; streaming?: boolean }) {
  const [open, setOpen] = useState(Boolean(streaming));
  return (
    <div className="mb-2 rounded-lg border border-border/60 bg-muted/40">
      <button
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground"
        onClick={() => setOpen((o) => !o)}
      >
        <Brain className={cn('h-3.5 w-3.5', streaming && 'animate-pulse text-purple-400')} />
        <span>{streaming ? '正在深度思考…' : '思考过程'}</span>
        {open ? <ChevronDown className="ml-auto h-3 w-3" /> : <ChevronRight className="ml-auto h-3 w-3" />}
      </button>
      {open && (
        <div className="max-h-64 overflow-y-auto border-t border-border/60 px-3 py-2">
          <p className="whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground">{reasoning}</p>
        </div>
      )}
    </div>
  );
}

/** 单条消息气泡：React.memo 隔离重渲染，流式时只有目标消息更新 */
export const MessageItem = memo(function MessageItem({ msg }: { msg: ChatMessage }) {
  const { regenerate, removeMessage, generating } = useChatStore();
  const [copied, setCopied] = useState(false);
  const isUser = msg.role === 'user';
  const isSystem = msg.role === 'system';

  // 系统消息（工具执行结果）不渲染给用户
  if (isSystem) return null;

  const doCopy = () => {
    void navigator.clipboard.writeText(msg.content).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <div className={cn('group flex w-full px-4 py-1.5', isUser ? 'justify-end' : 'justify-start', 'animate-bubble-in')}>
      <div
        className={cn(
          'relative max-w-[78%] rounded-2xl px-4 py-2.5',
          isUser
            ? 'bg-primary/15 text-foreground rounded-br-md'
            : 'acrylic-card rounded-bl-md',
          msg.streaming && 'streaming-glow',
        )}
      >
        {/* 附件（图片缩略图） */}
        {msg.attachments?.map((a, i) =>
            a.kind === 'image' ? (
              <img key={i} src={a.data} alt={a.name} loading="lazy" className="mb-2 max-h-48 cursor-zoom-in rounded-lg" onClick={() => window.open(a.data, '_blank')} />
            ) : (
              <div key={i} className="mb-2 rounded-lg bg-muted px-3 py-1.5 text-xs">📎 {a.name}</div>
            ),
          )}
          {isUser ? (
            <p className="whitespace-pre-wrap text-sm leading-relaxed">{msg.content}</p>
          ) : (
            <>
              {msg.reasoning ? <ReasoningBlock reasoning={msg.reasoning} streaming={msg.streaming && !msg.content} /> : null}
              <MarkdownView content={msg.content} />
              {msg.streaming && <span className="streaming-cursor" />}
            </>
          )}
          {/* 悬停操作栏 */}
          {!msg.streaming && (
            <div className={cn('absolute -bottom-6 flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100', isUser ? 'right-1' : 'left-1')}>
              {!isUser && (
                <button className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40" title="重新生成" disabled={generating} onClick={() => void regenerate(msg.id)}>
                  <RefreshCw className="h-3.5 w-3.5" />
                </button>
              )}
              <button className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground" title={copied ? '已复制' : '复制'} onClick={doCopy}>
                {copied ? <Check className="h-3.5 w-3.5 text-green-500" /> : <Copy className="h-3.5 w-3.5" />}
              </button>
              <button className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-red-500" title="删除" onClick={() => void removeMessage(msg.id)}>
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          )}
      </div>
    </div>
  );
});
