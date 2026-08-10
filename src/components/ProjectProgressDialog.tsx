import { memo, useCallback, useEffect, useState } from 'react';
import { FileText, FolderTree, HardDrive, RefreshCw } from 'lucide-react';
import { useWorkspaceStore } from '@/store/useWorkspaceStore';
import { useChatStore } from '@/store/useChatStore';
import { getProjectStats, type ProjectStats } from '@/lib/backend';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';

function formatSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function formatTime(secs: number): string {
  return new Date(secs * 1000).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}

/** 项目进度查看：工作目录规模统计 + 最近修改文件 + 会话活动 */
export const ProjectProgressDialog = memo(function ProjectProgressDialog({
  open, onClose,
}: {
  open: boolean; onClose: () => void;
}) {
  const { workdir, terminalEntries } = useWorkspaceStore();
  const { messages, totalMessages, conversations, currentConvId } = useChatStore();
  const [stats, setStats] = useState<ProjectStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    if (!workdir) { setStats(null); return; }
    setLoading(true);
    setError('');
    try {
      setStats(await getProjectStats(workdir));
    } catch (e) {
      setError(String(e));
    }
    setLoading(false);
  }, [workdir]);

  useEffect(() => { if (open) void load(); }, [open, load]);

  const conv = conversations.find((c) => c.id === currentConvId);

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[85vh] max-w-2xl flex-col">
        <DialogHeader>
          <DialogTitle className="flex items-center justify-between pr-6">
            项目进度
            <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => void load()} title="刷新">
              <RefreshCw className={loading ? 'h-3.5 w-3.5 animate-spin' : 'h-3.5 w-3.5'} />
            </Button>
          </DialogTitle>
        </DialogHeader>

        {!workdir ? (
          <p className="py-6 text-center text-sm text-muted-foreground">当前对话未绑定工作目录（可在设置 → 分身中配置）</p>
        ) : (
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto">
            <p className="break-all font-mono text-xs text-muted-foreground">{workdir}</p>

            {/* 概览卡片 */}
            <div className="grid grid-cols-3 gap-3">
              <div className="rounded-lg border border-border p-3 text-center">
                <FileText className="mx-auto mb-1 h-4 w-4 text-sky-400" />
                <p className="text-lg font-semibold">{stats?.total_files ?? '-'}</p>
                <p className="text-[11px] text-muted-foreground">文件</p>
              </div>
              <div className="rounded-lg border border-border p-3 text-center">
                <FolderTree className="mx-auto mb-1 h-4 w-4 text-green-400" />
                <p className="text-lg font-semibold">{stats?.total_dirs ?? '-'}</p>
                <p className="text-[11px] text-muted-foreground">目录</p>
              </div>
              <div className="rounded-lg border border-border p-3 text-center">
                <HardDrive className="mx-auto mb-1 h-4 w-4 text-purple-400" />
                <p className="text-lg font-semibold">{stats ? formatSize(stats.total_size) : '-'}</p>
                <p className="text-[11px] text-muted-foreground">总大小</p>
              </div>
            </div>
            {error && <p className="text-xs text-red-400">{error}</p>}

            {/* 会话活动 */}
            <div className="rounded-lg border border-border p-3">
              <p className="mb-2 text-sm font-medium">会话活动</p>
              <div className="grid grid-cols-3 gap-2 text-center text-xs text-muted-foreground">
                <div><p className="text-base font-semibold text-foreground">{totalMessages}</p>当前对话消息</div>
                <div><p className="text-base font-semibold text-foreground">{messages.filter((m) => m.role === 'assistant').length}</p>AI 回复</div>
                <div><p className="text-base font-semibold text-foreground">{terminalEntries.length}</p>终端输出行</div>
              </div>
              <p className="mt-2 text-[11px] text-muted-foreground">对话：{conv?.title ?? '-'}</p>
            </div>

            {/* 最近修改文件 */}
            <div className="rounded-lg border border-border p-3">
              <p className="mb-2 text-sm font-medium">最近修改的文件</p>
              <div className="space-y-1">
                {stats?.recent.map((f) => (
                  <div key={f.path} className="flex items-center gap-2 text-xs">
                    <span className="shrink-0 text-muted-foreground">{formatTime(f.modified)}</span>
                    <span className="min-w-0 flex-1 truncate font-mono" title={f.path}>{f.path.replace(workdir, '…')}</span>
                    <span className="shrink-0 text-muted-foreground">{formatSize(f.size)}</span>
                  </div>
                ))}
                {stats && stats.recent.length === 0 && (
                  <p className="text-xs text-muted-foreground">暂无文件</p>
                )}
              </div>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
});
