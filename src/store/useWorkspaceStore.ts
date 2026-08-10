import { create } from 'zustand';
import type { FileNode, TerminalEntry } from '@/types';
import { fsReadDirTree, fsWatchDir, tauriListen, terminalKill, terminalSpawn, terminalWrite } from '@/lib/backend';

interface WorkspaceState {
  workdir: string;
  fileTree: FileNode[];
  treeLoading: boolean;
  filePanelOpen: boolean;
  terminalOpen: boolean;
  sidebarCollapsed: boolean;
  terminalEntries: TerminalEntry[];
  terminalSessionId: string;
  terminalPaused: boolean;
  previewFile: { path: string; kind: 'text' | 'image' } | null;

  setWorkdir: (dir: string) => Promise<void>;
  refreshTree: () => Promise<void>;
  toggleFilePanel: () => void;
  toggleTerminal: () => void;
  toggleSidebar: () => void;
  startTerminal: () => Promise<void>;
  writeTerminal: (input: string) => Promise<void>;
  appendTerminal: (text: string) => void;
  clearTerminal: () => void;
  setTerminalPaused: (p: boolean) => void;
  setPreviewFile: (f: WorkspaceState['previewFile']) => void;
}

let termSeq = 0;
let termUnlisten: (() => void) | null = null;
let watchUnlisten: (() => void) | null = null;
let refreshTimer: ReturnType<typeof setTimeout> | null = null;
const MAX_TERMINAL_ENTRIES = 5000; // 内存回收：环形截断

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  workdir: '',
  fileTree: [],
  treeLoading: false,
  filePanelOpen: false,
  terminalOpen: false,
  sidebarCollapsed: false,
  terminalEntries: [],
  terminalSessionId: '',
  terminalPaused: false,
  previewFile: null,

  setWorkdir: async (dir) => {
    set({ workdir: dir });
    if (watchUnlisten) { watchUnlisten(); watchUnlisten = null; }
    if (dir) {
      await fsWatchDir(dir).catch(() => {});
      // 文件变化 -> 300ms 防抖刷新
      watchUnlisten = await tauriListen('workspace-changed', () => {
        if (refreshTimer) clearTimeout(refreshTimer);
        refreshTimer = setTimeout(() => void get().refreshTree(), 300);
      }).catch(() => null);
    }
    await get().refreshTree();
  },

  refreshTree: async () => {
    const { workdir } = get();
    if (!workdir) { set({ fileTree: [] }); return; }
    set({ treeLoading: true });
    try {
      const tree = await fsReadDirTree(workdir);
      set({ fileTree: tree, treeLoading: false });
    } catch {
      set({ fileTree: [], treeLoading: false });
    }
  },

  toggleFilePanel: () => set((s) => ({ filePanelOpen: !s.filePanelOpen })),
  toggleTerminal: () => {
    const next = !get().terminalOpen;
    set({ terminalOpen: next });
    if (next && !get().terminalSessionId) void get().startTerminal();
  },
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),

  startTerminal: async () => {
    if (termUnlisten) { termUnlisten(); termUnlisten = null; }
    const old = get().terminalSessionId;
    if (old && old !== 'mock-session') await terminalKill(old).catch(() => {});
    const sid = await terminalSpawn(get().workdir || undefined).catch(() => 'mock-session');
    set({ terminalSessionId: sid });
    if (sid !== 'mock-session') {
      termUnlisten = await tauriListen<string>(`terminal-output-${sid}`, (text) => {
        get().appendTerminal(text);
      }).catch(() => null);
    } else {
      get().appendTerminal('\x1b[36m[浏览器预览模式] 模拟终端已就绪\x1b[0m\r\n$ ');
    }
  },

  appendTerminal: (text) => {
    set((s) => {
      let entries = [...s.terminalEntries, { id: ++termSeq, text, ts: Date.now() }];
      if (entries.length > MAX_TERMINAL_ENTRIES) entries = entries.slice(-MAX_TERMINAL_ENTRIES);
      return { terminalEntries: entries };
    });
  },

  writeTerminal: async (input) => {
    const { terminalSessionId } = get();
    if (!terminalSessionId || terminalSessionId === 'mock-session') {
      // 浏览器模式：模拟回显
      get().appendTerminal(`${input}\r\n`);
      get().appendTerminal('\x1b[90m[模拟终端] 命令在 Tauri 桌面版中执行\x1b[0m\r\n$ ');
      return;
    }
    await terminalWrite(terminalSessionId, input);
  },

  clearTerminal: () => set({ terminalEntries: [] }),
  setTerminalPaused: (p) => set({ terminalPaused: p }),
  setPreviewFile: (f) => set({ previewFile: f }),
}));
