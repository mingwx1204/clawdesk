import { memo, useMemo } from 'react';
import { useChatStore } from '@/store/useChatStore';
import { useSettingsStore } from '@/store/useSettingsStore';
import { buildContext } from '@/lib/llm';
import { contextWindowFor, estimateTokens, formatTokens } from '@/lib/tokens';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Progress } from '@/components/ui/progress';
import { cn } from '@/lib/utils';

/** 上下文查看：展示即将发送给模型的完整上下文（系统提示词 + 历史消息） */
export const ContextViewerDialog = memo(function ContextViewerDialog({
  open, onClose,
}: {
  open: boolean; onClose: () => void;
}) {
  const { messages, personas, currentPersonaId, conversations, currentConvId } = useChatStore();
  const { settings, resolveModel } = useSettingsStore();

  const persona = personas.find((p) => p.id === currentPersonaId);
  const conv = conversations.find((c) => c.id === currentConvId);
  const model = resolveModel(conv?.modelId || persona?.modelId || settings.defaultModelId);

  const ctx = useMemo(
    () => buildContext(persona?.systemPrompt ?? '', messages),
    [persona?.systemPrompt, messages],
  );
  const items = useMemo(
    () => ctx.map((m) => ({ ...m, tokens: estimateTokens(typeof m.content === 'string' ? m.content : JSON.stringify(m.content)) })),
    [ctx],
  );
  const total = items.reduce((s, i) => s + i.tokens, 0);
  const window_ = contextWindowFor(model?.model ?? '');
  const pct = Math.min(100, (total / window_) * 100);

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[85vh] max-w-2xl flex-col">
        <DialogHeader>
          <DialogTitle>上下文查看</DialogTitle>
        </DialogHeader>
        <div className="space-y-1.5">
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>模型：{model?.label ?? '未选择'} · 上下文窗口 {formatTokens(window_)} tokens</span>
            <span className={cn(pct > 85 ? 'text-red-400' : pct > 60 ? 'text-yellow-400' : 'text-green-400')}>
              已用约 {formatTokens(total)} / {formatTokens(window_)}（{pct.toFixed(1)}%）
            </span>
          </div>
          <Progress value={pct} className="h-1.5" />
          <p className="text-[11px] text-muted-foreground">按 CJK≈1 token/字、ASCII≈1 token/4 字符估算，实际以模型分词为准</p>
        </div>
        <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pt-2">
          {items.map((m, i) => (
            <div key={i} className="rounded-lg border border-border p-3">
              <div className="mb-1 flex items-center gap-2">
                <span className={cn(
                  'rounded px-1.5 py-0.5 text-[10px] font-medium',
                  m.role === 'system' && 'bg-purple-500/20 text-purple-300',
                  m.role === 'user' && 'bg-sky-500/20 text-sky-300',
                  m.role === 'assistant' && 'bg-green-500/20 text-green-300',
                )}>
                  {m.role === 'system' ? '系统提示词' : m.role === 'user' ? '用户' : 'AI'}
                </span>
                <span className="text-[10px] text-muted-foreground">≈ {formatTokens(m.tokens)} tokens</span>
              </div>
              <p className="whitespace-pre-wrap break-all text-xs leading-relaxed text-muted-foreground">
                {(() => { const s = typeof m.content === 'string' ? m.content : JSON.stringify(m.content); return s.length > 400 ? s.slice(0, 400) + ' …' : s; })()}
              </p>
            </div>
          ))}
          {items.length === 0 && <p className="py-6 text-center text-sm text-muted-foreground">暂无上下文</p>}
        </div>
      </DialogContent>
    </Dialog>
  );
});
