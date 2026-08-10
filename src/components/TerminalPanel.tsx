import { memo, useEffect, useRef, useState } from 'react';
import { Copy, Eraser, Pause, Play, Search, Terminal } from 'lucide-react';
import { useWorkspaceStore } from '@/store/useWorkspaceStore';
import { ansiToHtml } from '@/lib/ansi';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';

/** 底部终端输出面板：ANSI 渲染 + 搜索 + 复制 + 清空 + 自动滚动 */
export const TerminalPanel = memo(function TerminalPanel() {
  const {
    terminalOpen, terminalEntries, clearTerminal,
    terminalPaused, setTerminalPaused, terminalSessionId,
    writeTerminal,
  } = useWorkspaceStore();
  const [keyword, setKeyword] = useState('');
  const [copied, setCopied] = useState(false);
  const [exiting, setExiting] = useState(false);
  const [cmd, setCmd] = useState('');
  const [cmdHistory, setCmdHistory] = useState<string[]>([]);
  const [historyIdx, setHistoryIdx] = useState(-1);
  const prevOpen = useRef(terminalOpen);
  const bodyRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!terminalOpen && prevOpen.current) {
      setExiting(true);
      const t = setTimeout(() => setExiting(false), 200);
      prevOpen.current = false;
      return () => clearTimeout(t);
    }
    prevOpen.current = terminalOpen;
  }, [terminalOpen]);

  if (!terminalOpen && !exiting) return null;

  const kw = keyword.trim().toLowerCase();
  const visible = kw
    ? terminalEntries.filter((e) => e.text.toLowerCase().includes(kw))
    : terminalEntries;

  const doCopy = () => {
    const text = terminalEntries.map((e) => e.text.replace(/\x1b\[[0-9;]*m/g, '')).join('');
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <div className={`flex h-[200px] shrink-0 flex-col border-t border-border/40 bg-card ${exiting ? 'animate-fade-out' : 'animate-slide-up'}`}>
      {/* 工具条 */}
      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-border/50 px-2">
        <span className="text-xs font-medium text-muted-foreground">终端输出</span>
        {terminalSessionId && <span className="ml-1 text-[10px] text-muted-foreground/60">#{terminalSessionId.slice(0, 8)}</span>}
        <div className="flex-1" />
        <div className="relative">
          <Search className="absolute left-2 top-1.5 h-3 w-3 text-muted-foreground" />
          <Input
            className="h-6 w-40 rounded pl-7 text-[11px]"
            placeholder="搜索日志…"
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
          />
        </div>
        <Button variant="ghost" size="icon" className="h-6 w-6" title={copied ? '已复制' : '复制全部'} onClick={doCopy}>
          <Copy className="h-3 w-3" />
        </Button>
        <Button variant="ghost" size="icon" className="h-6 w-6" title="清空" onClick={clearTerminal}>
          <Eraser className="h-3 w-3" />
        </Button>
        <Button
          variant="ghost" size="icon"
          className={cn('h-6 w-6', terminalPaused && 'text-yellow-400')}
          title={terminalPaused ? '恢复自动滚动' : '暂停自动滚动'}
          onClick={() => setTerminalPaused(!terminalPaused)}
        >
          {terminalPaused ? <Play className="h-3 w-3" /> : <Pause className="h-3 w-3" />}
        </Button>
      </div>
      {/* 输出体 */}
      <div ref={bodyRef} className="min-h-0 flex-1 overflow-y-auto px-3 py-1 font-mono text-xs leading-5 text-gray-300">
        {visible.length === 0 ? (
          <p className="py-2 text-muted-foreground/60">暂无输出</p>
        ) : (
          visible.map((e) => (
            <div key={e.id} dangerouslySetInnerHTML={{ __html: ansiToHtml(e.text) }} className="whitespace-pre-wrap break-all" />
          ))
        )}
      </div>
      {/* 命令行输入 */}
      <div className="flex items-center gap-1.5 border-t border-border/50 px-2 py-1">
        <Terminal className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <Input
          ref={inputRef}
          className="h-7 flex-1 border-0 bg-transparent px-1 font-mono text-xs shadow-none outline-none focus-visible:ring-0"
          placeholder={terminalSessionId === 'mock-session' ? '模拟终端 — 桌面版可执行真实命令' : '输入命令，Enter 执行…'}
          value={cmd}
          onChange={(e) => setCmd(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && cmd.trim()) {
              const input = cmd.trim() + '\n';
              setCmdHistory((h) => [...h, cmd.trim()]);
              setHistoryIdx(-1);
              setCmd('');
              void writeTerminal(input);
            } else if (e.key === 'ArrowUp') {
              e.preventDefault();
              const h = cmdHistory;
              if (h.length === 0) return;
              const idx = historyIdx === -1 ? h.length - 1 : Math.max(0, historyIdx - 1);
              setHistoryIdx(idx);
              setCmd(h[idx]);
            } else if (e.key === 'ArrowDown') {
              e.preventDefault();
              const h = cmdHistory;
              if (h.length === 0 || historyIdx === -1) return;
              const idx = historyIdx + 1;
              if (idx >= h.length) { setHistoryIdx(-1); setCmd(''); return; }
              setHistoryIdx(idx);
              setCmd(h[idx]);
            }
          }}
        />
      </div>
    </div>
  );
});
