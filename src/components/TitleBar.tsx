import { memo, useState } from 'react';
import { Minus, Square, X, Copy } from 'lucide-react';
import { isTauri, winClose, winMinimize, winStartDragging, winToggleMaximize } from '@/lib/backend';
import { CLAWDESK_VERSION } from '@/lib/version';

/** 自定义无边框标题栏：拖拽移动 + 最小化/最大化/关闭 */
export const TitleBar = memo(function TitleBar() {
  const [maximized, setMaximized] = useState(false);
  const tauri = isTauri();

  return (
    <div
      className="flex h-9 shrink-0 items-center justify-between border-b border-border/40 acrylic select-none"
      data-tauri-drag-region="true"
      onMouseDown={(e) => {
        // 双击空白处切换最大化
        if (e.detail === 2 && (e.target as HTMLElement).dataset.drag === 'true') {
          void winToggleMaximize().then(() => setMaximized((m) => !m));
        }
      }}
    >
      <div
        data-drag="true"
        className="flex h-full flex-1 items-center gap-2 px-3"
        onMouseDown={() => void winStartDragging()}
      >
        <img src="./src-tauri/icons/32x32.png" alt="" className="h-6 w-6" onError={(e) => ((e.target as HTMLImageElement).style.display = 'none')} />
        <span className="text-sm font-bold tracking-wide">ClawDesk</span>
        <span className="text-[10px] text-muted-foreground/50">v{CLAWDESK_VERSION}</span>
      </div>
      {tauri && (
        <div className="flex h-full">
          <button
            className="flex h-full w-11 items-center justify-center text-muted-foreground hover:bg-accent hover:text-foreground"
            onClick={() => void winMinimize()}
            title="最小化"
          >
            <Minus className="h-4 w-4" />
          </button>
          <button
            className="flex h-full w-11 items-center justify-center text-muted-foreground hover:bg-accent hover:text-foreground"
            onClick={() => void winToggleMaximize().then(() => setMaximized((m) => !m))}
            title={maximized ? '还原' : '最大化'}
          >
            {maximized ? <Copy className="h-3.5 w-3.5" /> : <Square className="h-3.5 w-3.5" />}
          </button>
          <button
            className="flex h-full w-11 items-center justify-center text-muted-foreground hover:bg-red-600 hover:text-white"
            onClick={() => void winClose()}
            title="关闭（最小化到托盘）"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
      )}
    </div>
  );
});
