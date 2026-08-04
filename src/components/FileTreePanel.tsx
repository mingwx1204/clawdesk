import { memo, useCallback, useEffect, useRef, useState } from 'react';
import {
  ChevronDown, ChevronRight, Copy, File, FileCode2, FileImage, FileText,
  Folder, FolderOpen, Pencil, RefreshCw, Trash2, X,
} from 'lucide-react';
import type { FileNode } from '@/types';
import { useWorkspaceStore } from '@/store/useWorkspaceStore';
import { fsDelete, fsReadFileText, fsRename, openPathInExplorer } from '@/lib/backend';
import { notifyError } from '@/lib/notify';
import { Button } from '@/components/ui/button';
import {
  ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger,
} from '@/components/ui/context-menu';
import { cn } from '@/lib/utils';

const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp']);
const CODE_EXTS = new Set(['ts', 'tsx', 'js', 'jsx', 'py', 'rs', 'go', 'java', 'c', 'cpp', 'h', 'json', 'yaml', 'yml', 'toml', 'css', 'html']);

function FileIcon({ node }: { node: FileNode }) {
  if (node.is_dir) return <Folder className="h-3.5 w-3.5 shrink-0 text-sky-400" />;
  if (IMAGE_EXTS.has(node.ext)) return <FileImage className="h-3.5 w-3.5 shrink-0 text-purple-400" />;
  if (CODE_EXTS.has(node.ext)) return <FileCode2 className="h-3.5 w-3.5 shrink-0 text-green-400" />;
  if (['md', 'txt', 'log'].includes(node.ext)) return <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />;
  return <File className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />;
}

const TreeNode = memo(function TreeNode({ node, depth }: { node: FileNode; depth: number }) {
  const [open, setOpen] = useState(depth < 1);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(node.name);
  const { refreshTree, setPreviewFile } = useWorkspaceStore();

  const onClick = () => {
    if (node.is_dir) setOpen((o) => !o);
    else if (IMAGE_EXTS.has(node.ext)) setPreviewFile({ path: node.path, kind: 'image' });
    else setPreviewFile({ path: node.path, kind: 'text' });
  };

  const doRename = async () => {
    setRenaming(false);
    if (draft.trim() && draft !== node.name) {
      await fsRename(node.path, draft.trim()).catch((e) => notifyError(String(e)));
      void refreshTree();
    }
  };

  return (
    <div>
      <ContextMenu>
        <ContextMenuTrigger>
          <div
            className="flex cursor-pointer items-center gap-1 rounded px-1 py-[3px] text-[13px] hover:bg-accent/60"
            style={{ paddingLeft: depth * 14 + 6 }}
            onClick={onClick}
          >
            {node.is_dir ? (
              open ? <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" /> : <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
            ) : (
              <span className="w-3 shrink-0" />
            )}
            <FileIcon node={node} />
            {renaming ? (
              <input
                className="w-full rounded bg-background px-1 text-xs outline-none ring-1 ring-primary"
                value={draft}
                autoFocus
                onChange={(e) => setDraft(e.target.value)}
                onBlur={() => void doRename()}
                onKeyDown={(e) => { if (e.key === 'Enter') void doRename(); if (e.key === 'Escape') setRenaming(false); }}
                onClick={(e) => e.stopPropagation()}
              />
            ) : (
              <span className="truncate">{node.name}</span>
            )}
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onClick={() => void openPathInExplorer(node.path)}>
            <FolderOpen className="mr-2 h-3.5 w-3.5" /> 打开
          </ContextMenuItem>
          <ContextMenuItem onClick={() => void navigator.clipboard.writeText(node.path)}>
            <Copy className="mr-2 h-3.5 w-3.5" /> 复制路径
          </ContextMenuItem>
          <ContextMenuItem onClick={() => { setDraft(node.name); setRenaming(true); }}>
            <Pencil className="mr-2 h-3.5 w-3.5" /> 重命名
          </ContextMenuItem>
          <ContextMenuItem
            className="text-red-500"
            onClick={() => {
              if (node.is_dir) {
                // 目录删除：要求输入名称确认
                const answer = window.prompt(
                  `⚠️ 即将删除目录「${node.name}」及其所有内容！\n此操作不可恢复。\n\n请输入目录名称以确认：`,
                );
                if (answer !== node.name) return;
              } else if (!confirm(`确认删除 ${node.name}？此操作不可恢复。`)) {
                return;
              }
              void fsDelete(node.path).then(() => refreshTree()).catch((e) => notifyError(String(e)));
            }}
          >
            <Trash2 className="mr-2 h-3.5 w-3.5" /> 删除
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      {node.is_dir && open && node.children.map((c) => <TreeNode key={c.path} node={c} depth={depth + 1} />)}
    </div>
  );
});

/** 内置文件预览弹层 */
const PreviewModal = memo(function PreviewModal() {
  const { previewFile, setPreviewFile } = useWorkspaceStore();
  const [content, setContent] = useState<string>('');
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    if (!previewFile) return;
    setLoading(true);
    if (previewFile.kind === 'text') {
      setContent(await fsReadFileText(previewFile.path).catch((e) => String(e)));
    } else {
      const { fsReadFileBase64 } = await import('@/lib/backend');
      const b64 = await fsReadFileBase64(previewFile.path).catch(() => '');
      const ext = previewFile.path.split('.').pop() ?? 'png';
      setContent(b64 ? `data:image/${ext};base64,${b64}` : '');
    }
    setLoading(false);
  }, [previewFile]);

  useEffect(() => { void load(); }, [load]);

  // ESC 关闭预览
  useEffect(() => {
    if (!previewFile) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setPreviewFile(null); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [previewFile, setPreviewFile]);

  if (!previewFile) return null;
  return (
    <div className="absolute inset-0 z-30 flex items-center justify-center bg-black/60 p-8" onClick={() => setPreviewFile(null)}>
      <div className="flex max-h-full w-full max-w-3xl flex-col rounded-xl border border-border bg-card shadow-2xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between border-b border-border px-4 py-2">
          <span className="truncate text-sm">{previewFile.path}</span>
          <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => setPreviewFile(null)}>
            <X className="h-4 w-4" />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-4">
          {loading ? (
            <p className="text-sm text-muted-foreground">加载中…</p>
          ) : previewFile.kind === 'image' ? (
            content ? <img src={content} alt="" className="mx-auto max-h-[60vh] rounded-lg" /> : <p className="text-sm text-muted-foreground">无法预览</p>
          ) : (
            <pre className="whitespace-pre-wrap font-mono text-xs leading-relaxed">{content}</pre>
          )}
        </div>
      </div>
    </div>
  );
});

/** 右侧文件树面板 */
export const FileTreePanel = memo(function FileTreePanel() {
  const { fileTree, treeLoading, workdir, refreshTree, filePanelOpen } = useWorkspaceStore();
  const [exiting, setExiting] = useState(false);
  const prevOpen = useRef(filePanelOpen);

  useEffect(() => {
    if (!filePanelOpen && prevOpen.current) {
      setExiting(true);
      const t = setTimeout(() => setExiting(false), 200);
      prevOpen.current = false;
      return () => clearTimeout(t);
    }
    prevOpen.current = filePanelOpen;
  }, [filePanelOpen]);

  if (!filePanelOpen && !exiting) return null;

  return (
    <div className={`flex w-[300px] shrink-0 flex-col border-l border-border/40 bg-card ${exiting ? 'animate-fade-out' : 'animate-slide-in-right'}`}>
      <div className="flex h-11 shrink-0 items-center justify-between border-b border-border px-3">
        <span className="text-sm font-medium">工作目录</span>
        <Button variant="ghost" size="icon" className="h-7 w-7" title="刷新" onClick={() => void refreshTree()}>
          <RefreshCw className={cn('h-3.5 w-3.5', treeLoading && 'animate-spin')} />
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto py-1 pr-1">
        {!workdir ? (
          <p className="p-4 text-xs text-muted-foreground">当前对话未绑定工作目录。可在分身设置中配置。</p>
        ) : fileTree.length === 0 && !treeLoading ? (
          <p className="p-4 text-xs text-muted-foreground">目录为空或不可访问</p>
        ) : (
          fileTree.map((n) => <TreeNode key={n.path} node={n} depth={0} />)
        )}
      </div>
      <PreviewModal />
    </div>
  );
});
