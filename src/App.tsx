/**
 * ClawDesk 应用根组件。
 * 组件树：TitleBar → Sidebar + (ChatArea + FileTreePanel) + TerminalPanel + InputBox。
 * 负责主题/字号管理、Tauri 事件监听（托盘、快捷键、手机桥接）。
 */

import { useEffect, useState } from 'react';
import { TitleBar } from '@/components/TitleBar';
import { ChatArea } from '@/components/ChatArea';
import { InputBox } from '@/components/InputBox';
import { TodoPanel } from '@/components/TodoPanel';
import { FileTreePanel } from '@/components/FileTreePanel';
import { TerminalPanel } from '@/components/TerminalPanel';
import { SettingsDialog } from '@/components/SettingsDialog';
import { PermissionDialog } from '@/components/PermissionDialog';
import { useChatStore } from '@/store/useChatStore';
import { useSettingsStore } from '@/store/useSettingsStore';
import { isTauri, registerGlobalShortcut, tauriListen, botServerStart, wechatBotStart } from '@/lib/backend';

/** 开机自动恢复：若配置了 Bot 引擎 / 微信 Bot，应用启动后自动拉起 */
function autoRestoreBot() {
  if (!isTauri()) return;
  try {
    const settings = useSettingsStore.getState().settings;
    if (!settings.botPlatform?.enabled) return;
    // 启动内置 Bot HTTP 引擎（webhook / 多平台消息）
    botServerStart(settings.botPlatform).catch(() => {
      // 已运行或端口占用等：忽略，不影响微信 iLink 直连
    });
    // 若启用微信平台，自动连接微信（自动加载扫码凭据 + 启动消息监听）
    const wechat = settings.botPlatform.platforms?.find((p) => p.id === 'wechat');
    if (wechat?.enabled) {
      wechatBotStart(settings.wechatBot).catch(() => {
        // 未登录或网络未就绪：静默，用户可在 Bot 面板重新连接
      });
    }
  } catch {
    // 启动期异常一律静默
  }
}

/** 主题应用：dark / light / system */
function applyTheme(theme: 'dark' | 'light' | 'system') {
  const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  const dark = theme === 'dark' || (theme === 'system' && prefersDark);
  document.documentElement.classList.toggle('dark', dark);
}

import { WorkspacePanel } from '@/components/WorkspacePanel';

export default function App() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [workspaceOpen, setWorkspaceOpen] = useState(false);
  const [ready, setReady] = useState(false);
  const settings = useSettingsStore((s) => s.settings);
  const loaded = useSettingsStore((s) => s.loaded);

  // 启动：加载设置 -> 最低显示 2 秒启动动画
  useEffect(() => {
    void (async () => {
      await useSettingsStore.getState().load();
      await useChatStore.getState().init();
      // 开机自动恢复：若配置了 Bot 引擎 / 微信 Bot，应用启动后自动拉起
      void autoRestoreBot();
      // 至少等 2 秒让启动动画完整播放
      const elapsed = performance.now();
      const remaining = Math.max(0, 2000 - elapsed);
      setTimeout(() => setReady(true), remaining);
    })();
  }, []);

  // 启动画面 ··· 点号循环 — 纯 CSS 动画
  useEffect(() => { /* dots handled by CSS */ }, []);

  // 主题与字号
  useEffect(() => {
    if (!loaded) return;
    applyTheme(settings.theme);
    document.documentElement.style.fontSize = `${settings.fontSize}px`;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => applyTheme(settings.theme);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, [settings.theme, settings.fontSize, loaded]);

  // 自定义背景 + 主题色
  useEffect(() => {
    if (!loaded) return;
    const root = document.documentElement;
    if (settings.customBackground && settings.customBackground !== 'none') {
      const bg = settings.customBackground;
      const isUrl = bg.startsWith('http') || bg.startsWith('/') || bg.startsWith('.') || bg.startsWith('data:');
      root.style.setProperty('--custom-bg', isUrl ? `url(${bg})` : bg);
      document.body.style.setProperty('--app-bg-opacity', `${settings.backgroundOpacity / 100}`);
      document.body.classList.add('has-custom-bg');
    } else {
      root.style.removeProperty('--custom-bg');
      document.body.style.removeProperty('--app-bg-opacity');
      document.body.classList.remove('has-custom-bg');
    }
    // 主题色 — 仅非默认值时覆盖
    if (settings.accentHue !== 207) {
      root.style.setProperty('--accent-hue', String(settings.accentHue));
    } else {
      root.style.removeProperty('--accent-hue');
    }
  }, [settings.customBackground, settings.backgroundOpacity, settings.accentHue, loaded]);

  // Tauri 事件：托盘菜单 / 全局快捷键 / 手机端桥接
  useEffect(() => {
    if (!isTauri()) return;
    const unlistens: Promise<(() => void) | null>[] = [
      tauriListen('tray-new-chat', () => void useChatStore.getState().newConversation()),
      tauriListen('tray-open-settings', () => setSettingsOpen(true)),
      // 手机端发来的消息 -> 进入当前对话（标记来源避免回环）
      tauriListen<string>('mobile-user-msg', (content) => {
        void useChatStore.getState().send(content);
      }),
      // 微信 ClawBot 发来的消息 -> 发送到 AI 并自动回复
      tauriListen<{ msgId: string; fromUser: string; content: string; contextToken?: string }>('wechat-message', (payload) => {
        void useChatStore.getState().sendWithWechatReply(payload.content, payload.msgId, payload.fromUser, payload.contextToken);
      }),
    ];
    void registerGlobalShortcut(useSettingsStore.getState().settings.globalShortcut);
    const onToggle = () => { /* Rust 侧已处理窗口显隐，此事件备用 */ };
    window.addEventListener('clawdesk:toggle-window', onToggle);
    return () => {
      unlistens.forEach((p) => void p.then((un) => un?.()));
      window.removeEventListener('clawdesk:toggle-window', onToggle);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!ready) {
    return (
      <div className="flex h-screen items-center justify-center bg-background">
        <div className="flex flex-col items-center gap-4">
          {/* 🐾 双爪印交替 — position 左右分区不重叠 */}
          <div className="relative inline-block text-5xl leading-none">
            <span className="opacity-25">🐾</span>
            <span className="absolute left-0 top-0 w-1/2 overflow-hidden animate-paw-right">
              <span className="inline-block">🐾</span>
            </span>
            <span className="absolute right-0 top-0 w-1/2 overflow-hidden animate-paw-left" style={{ direction: 'rtl' }}>
              <span className="inline-block" style={{ direction: 'ltr' }}>🐾</span>
            </span>
          </div>
          <p className="text-sm text-muted-foreground">
            ClawDesk 启动中
            <span className="animate-dot1">.</span>
            <span className="animate-dot2">.</span>
            <span className="animate-dot3">.</span>
          </p>
        </div>
      </div>
    );
  }

  return (
    <div
      className="flex h-screen flex-col overflow-hidden text-foreground animate-fade-in"
      style={{
        background: settings.customBackground && settings.customBackground !== 'none'
          ? `hsl(var(--background) / ${settings.backgroundOpacity / 100})`
          : undefined,
      }}
    >
      <TitleBar />
      <div className="flex min-h-0 flex-1">
        <div className="flex min-w-0 flex-1 flex-col">
          {!settingsOpen && (
            <>
              <div className="flex min-h-0 flex-1">
                <ChatArea onOpenSettings={() => setSettingsOpen(true)} onToggleWorkspace={() => setWorkspaceOpen(!workspaceOpen)} />
                {workspaceOpen && <WorkspacePanel onClose={() => setWorkspaceOpen(false)} />}
                <FileTreePanel />
              </div>
              <TerminalPanel />
              <TodoPanel />
              <InputBox />
            </>
          )}
        </div>
      </div>
      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <PermissionDialog />
    </div>
  );
}
