/**
 * Tauri 调用封装 + 浏览器降级。
 * 浏览器 dev 预览时没有 Tauri 运行时，所有原生能力降级为
 * localStorage / mock，保证界面完全可用、可演示。
 */

export const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** 动态引入 @tauri-apps/api，避免浏览器环境打包/运行报错 */
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(cmd, args);
}

export async function tauriListen<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<() => void> {
  const { listen } = await import('@tauri-apps/api/event');
  const un = await listen<T>(event, (e) => handler(e.payload));
  return un;
}

/* ---------- 窗口控制 ---------- */

export async function winMinimize() {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().minimize();
}

export async function winToggleMaximize() {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  const w = getCurrentWindow();
  (await w.isMaximized()) ? await w.unmaximize() : await w.maximize();
}

export async function winClose() {
  if (!isTauri()) {
    window.close();
    return;
  }
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().close();
}

export async function winStartDragging() {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().startDragging();
}

export async function winSetAlwaysOnTop(flag: boolean) {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().setAlwaysOnTop(flag);
}

/* ---------- 权限闸门（敏感操作统一入口） ---------- */

async function gate(action: string, title: string, detail: string): Promise<boolean> {
  if (!isTauri()) return true; // 浏览器预览不拦截
  const { usePermissionStore } = await import('@/store/usePermissionStore');
  return usePermissionStore.getState().request({ action, title, detail });
}

/* ---------- 文件系统（Rust 命令） ---------- */

import type { FileNode } from '@/types';

export async function fsReadDirTree(path: string): Promise<FileNode[]> {
  if (!isTauri()) return mockDirTree();
  return tauriInvoke<FileNode[]>('read_dir_tree', { path });
}

export async function fsReadFileText(path: string): Promise<string> {
  if (!isTauri()) return `// 浏览器预览模式：无法读取本地文件\n// 路径: ${path}`;
  return tauriInvoke<string>('read_file_text', { path });
}

export async function fsReadFileBase64(path: string): Promise<string> {
  if (!isTauri()) return '';
  return tauriInvoke<string>('read_file_base64', { path });
}

export async function fsWriteFileText(path: string, content: string): Promise<void> {
  if (!isTauri()) { console.log(`[mock] write file: ${path}`); return; }
  if (!(await gate('fs-write', '写入文件', path))) {
    throw new Error('用户拒绝了写入文件操作');
  }
  return tauriInvoke('write_file_text', { path, content });
}

export async function fsRename(oldPath: string, newName: string): Promise<void> {
  if (!isTauri()) return;
  if (!(await gate('fs-rename', '重命名文件/文件夹', `${oldPath} → ${newName}`))) {
    throw new Error('用户拒绝了重命名操作');
  }
  return tauriInvoke('rename_path', { oldPath, newName });
}

export async function fsDelete(path: string): Promise<void> {
  if (!isTauri()) return;
  if (!(await gate('fs-delete', '删除文件/文件夹（不可恢复）', path))) {
    throw new Error('用户拒绝了删除操作');
  }
  return tauriInvoke('delete_path', { path });
}

export async function fsWatchDir(path: string): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('watch_dir', { path });
}

export async function openPathInExplorer(path: string): Promise<void> {
  if (!isTauri()) return;
  if (!(await gate('opener', '在系统资源管理器中打开', path))) return;
  const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
  await revealItemInDir(path).catch(async () => {
    const { openPath } = await import('@tauri-apps/plugin-opener');
    await openPath(path);
  });
}

/* ---------- 终端 ---------- */

export async function terminalSpawn(cwd?: string): Promise<string> {
  if (!isTauri()) return 'mock-session';
  if (!(await gate('terminal', '启动终端会话（Shell）', cwd ?? '默认目录'))) {
    throw new Error('用户拒绝了终端启动');
  }
  return tauriInvoke<string>('terminal_spawn', { cwd });
}

export async function terminalWrite(sessionId: string, data: string): Promise<void> {
  if (!isTauri()) return;
  if (!(await gate('terminal-write', '向终端写入命令', data.trim()))) {
    throw new Error('用户拒绝了终端写入');
  }
  return tauriInvoke('terminal_write', { sessionId, data });
}

export async function terminalKill(sessionId: string): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('terminal_kill', { sessionId });
}

/* ---------- 截屏 ---------- */

export async function captureScreen(): Promise<string> {
  if (!isTauri()) throw new Error('浏览器预览模式不支持截屏');
  if (!(await gate('screenshot', '截取屏幕画面', '主显示器整屏截图'))) {
    throw new Error('用户拒绝了截屏操作');
  }
  return tauriInvoke<string>('capture_screen');
}

/* ---------- 通知 / 自启 / 快捷键 ---------- */

export async function notify(title: string, body: string): Promise<void> {
  if (!isTauri()) return;
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      '@tauri-apps/plugin-notification'
    );
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === 'granted';
    if (granted) sendNotification({ title, body });
  } catch { /* 通知不可用则静默 */ }
}

export async function setAutoStart(enable: boolean): Promise<void> {
  if (!isTauri()) return;
  const { enable: en, disable: dis } = await import('@tauri-apps/plugin-autostart');
  enable ? await en() : await dis();
}

export async function registerGlobalShortcut(accel: string): Promise<void> {
  if (!isTauri()) return;
  const { register, unregisterAll } = await import('@tauri-apps/plugin-global-shortcut');
  await unregisterAll().catch(() => {});
  await register(accel, (e) => {
    if (e.state === 'Pressed') window.dispatchEvent(new CustomEvent('clawdesk:toggle-window'));
  });
}

/* ---------- LLM 流式对话（Tauri 走 Rust 后端，避免 WebView CORS 限制） ---------- */

import type { ChatRequest, StreamCallbacks } from '@/lib/llm';

export interface LlmStreamHandle {
  cancel: () => void;
}

export async function llmStream(req: ChatRequest, cb: StreamCallbacks): Promise<LlmStreamHandle> {
  if (!isTauri()) {
    // 浏览器预览：直接 fetch（允许跨域的端点或离线模拟）
    const { streamChat } = await import('@/lib/llm');
    const controller = new AbortController();
    // 外部 signal 触发时联动中断
    req.signal.addEventListener('abort', () => controller.abort());
    void streamChat({ ...req, signal: controller.signal }, cb);
    return { cancel: () => controller.abort() };
  }
  // 桌面端：Rust 发起请求，事件回流
  const { applyMode } = await import('@/lib/llm');
  const p = applyMode(req.params, req.mode);
  const requestId = await tauriInvoke<string>('llm_chat_start', {
    req: {
      apiBase: req.apiBase,
      apiKey: req.apiKey,
      model: req.model,
      messages: req.messages,
      temperature: p.temperature,
      maxTokens: p.maxTokens,
      topP: p.topP,
      mode: req.mode,
      isDeepSeek: req.apiBase.includes('deepseek.com'),
    },
  });
  let finished = false;
  const unlistens: (() => void)[] = [];
  const cleanup = () => unlistens.forEach((u) => u());
  unlistens.push(await tauriListen<string>(`llm-delta-${requestId}`, (t) => cb.onDelta(t)));
  if (cb.onReasoning) {
    const onReasoning = cb.onReasoning;
    unlistens.push(await tauriListen<string>(`llm-reasoning-${requestId}`, (t) => onReasoning(t)));
  }
  unlistens.push(
    await tauriListen<string>(`llm-done-${requestId}`, () => {
      if (finished) return;
      finished = true;
      cleanup();
      cb.onDone();
    }),
  );
  unlistens.push(
    await tauriListen<string>(`llm-error-${requestId}`, (e) => {
      if (finished) return;
      finished = true;
      cleanup();
      cb.onError(e);
    }),
  );
  return {
    cancel: () => {
      void tauriInvoke('llm_chat_cancel', { requestId }).catch(() => {});
    },
  };
}

/* ---------- 账户余额（DeepSeek 兼容端点） ---------- */

export interface BalanceInfo {
  available: boolean;
  currency: string;
  totalBalance: string;
  grantedBalance?: string;
  toppedUpBalance?: string;
}

export async function fetchBalance(apiBase: string, apiKey: string): Promise<BalanceInfo> {
  if (!apiKey) throw new Error('请先填写 API Key');
  if (!isTauri()) {
    const root = apiBase.replace(/\/+$/, '').replace(/\/v1$/, '');
    const res = await fetch(`${root}/user/balance`, {
      headers: { Authorization: `Bearer ${apiKey}` },
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const j = await res.json();
    const b = j.balance_infos?.[0] ?? {};
    return {
      available: Boolean(j.is_available),
      currency: b.currency ?? 'CNY',
      totalBalance: b.total_balance ?? '-',
      grantedBalance: b.granted_balance,
      toppedUpBalance: b.topped_up_balance,
    };
  }
  const j = await tauriInvoke<{
    is_available?: boolean;
    balance_infos?: { currency?: string; total_balance?: string; granted_balance?: string; topped_up_balance?: string }[];
  }>('llm_balance', { apiBase, apiKey });
  const b = j.balance_infos?.[0] ?? {};
  return {
    available: Boolean(j.is_available),
    currency: b.currency ?? 'CNY',
    totalBalance: b.total_balance ?? '-',
    grantedBalance: b.granted_balance,
    toppedUpBalance: b.topped_up_balance,
  };
}

/* ---------- 项目进度统计 ---------- */

export interface ProjectStats {
  total_files: number;
  total_dirs: number;
  total_size: number;
  recent: { path: string; modified: number; size: number }[];
}

export async function getProjectStats(path: string): Promise<ProjectStats> {
  if (!isTauri()) {
    // 浏览器预览 mock
    return {
      total_files: 128, total_dirs: 24, total_size: 3_407_872,
      recent: [
        { path: `${path}\\src\\App.tsx`, modified: Math.floor(Date.now() / 1000) - 120, size: 3571 },
        { path: `${path}\\README.md`, modified: Math.floor(Date.now() / 1000) - 3600, size: 5554 },
      ],
    };
  }
  return tauriInvoke<ProjectStats>('project_stats', { path });
}

/* ---------- 手机端桥接（局域网真实服务） ---------- */

export interface BridgeInfo {
  url: string;
  lan_ip: string;
  port: number;
}

export async function mobileBridgeStart(): Promise<BridgeInfo> {
  if (!isTauri()) return { url: 'http://192.168.1.100:17895', lan_ip: '192.168.1.100', port: 17895 };
  if (!(await gate('mobile-bridge', '开启手机桥接服务', '本机局域网 HTTP 服务（端口 17895）'))) {
    throw new Error('用户拒绝了桥接服务启动');
  }
  return tauriInvoke<BridgeInfo>('mobile_bridge_start', {});
}

export async function mobileBridgeStop(): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('mobile_bridge_stop');
}

export async function mobileBridgePush(role: string, content: string): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('mobile_bridge_push', { role, content });
}

export async function mobileBridgeStatus(): Promise<{ running: boolean; connected: boolean }> {
  if (!isTauri()) return { running: false, connected: false };
  return tauriInvoke('mobile_bridge_status');
}

/** 生成真实二维码 SVG（内容为可扫描 URL） */
export async function mobileQrSvg(text: string): Promise<string> {
  if (!isTauri()) {
    // 浏览器预览：返回占位 SVG
    return `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200"><rect width="200" height="200" fill="#fff"/><text x="100" y="100" text-anchor="middle" font-size="12" fill="#000">预览模式二维码</text></svg>`;
  }
  return tauriInvoke<string>('mobile_qr_svg', { text });
}

/** 通用二维码生成（别名，供 Bot 平台等模块使用） */
export const generateQrSvg = mobileQrSvg;

/* ---------- 微信 ClawBot（腾讯 iLink 直连） ---------- */

import type { WechatBotConfig, WechatBotState, WechatQrResult } from '@/types';

/** 启动微信 Bot 轮询服务 */
export async function wechatBotStart(config: WechatBotConfig): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('wechat_bot_start', { config });
}

/** 停止微信 Bot */
export async function wechatBotStop(): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('wechat_bot_stop');
}

/** 通过 Bot 回复微信用户消息 */
export async function wechatBotReply(msgId: string, toUser: string, content: string): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('wechat_bot_reply', { msgId, toUser, content });
}

/** 获取 Bot 状态 */
export async function wechatBotStatus(): Promise<WechatBotState> {
  if (!isTauri()) return { running: false, connected: false, botName: '', lastPoll: 0, messageCount: 0 };
  return tauriInvoke<WechatBotState>('wechat_bot_status');
}

/** 获取微信登录二维码（腾讯 iLink Bot） */
export async function wechatGetQr(): Promise<WechatQrResult> {
  if (!isTauri()) return { qrcode: '', qrcodeUrl: '' };
  return tauriInvoke<WechatQrResult>('wechat_get_qr');
}

/** 长轮询扫码状态（单次，调用方循环） */
export async function wechatQrStatus(): Promise<Record<string, unknown>> {
  if (!isTauri()) return { status: 'wait' };
  return tauriInvoke<Record<string, unknown>>('wechat_qr_status');
}

/** 提交手机微信显示的配对码 */
export async function wechatVerifyCode(code: string): Promise<{ ok: boolean }> {
  if (!isTauri()) return { ok: true };
  return tauriInvoke<{ ok: boolean }>('wechat_verify_code', { code });
}

/** 刷新微信登录二维码 */
export async function wechatRefreshQr(): Promise<WechatQrResult> {
  if (!isTauri()) return { qrcode: '', qrcodeUrl: '' };
  return tauriInvoke<WechatQrResult>('wechat_refresh_qr');
}

/** 登出微信（清除本地 token） */
export async function wechatLogout(): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('wechat_logout');
}

/** 直接发送消息到微信用户（可指定 context_token） */
export async function wechatSendMessage(toUser: string, content: string, contextToken?: string): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('wechat_send_message', { toUser, content, contextToken });
}

/* ---------- 内置 ClawDesk Bot 引擎 ---------- */

import type { BotPlatformConfig } from '@/types';

export interface BotServerStatus {
  running: boolean;
  port: number;
  message_count: number;
  platforms_connected: string[];
}

/** 启动内置 Bot HTTP 服务器 */
export async function botServerStart(config: BotPlatformConfig): Promise<BotServerStatus> {
  if (!isTauri()) {
    console.log('[Dev] Bot 服务器启动模拟:', config);
    return { running: true, port: config.webhookPort, message_count: 0, platforms_connected: config.platforms.filter(p => p.enabled).map(p => p.id) };
  }
  return tauriInvoke<BotServerStatus>('bot_server_start', { config });
}

/** 停止内置 Bot 服务器 */
export async function botServerStop(): Promise<void> {
  if (!isTauri()) { console.log('[Dev] Bot 服务器已停止'); return; }
  return tauriInvoke('bot_server_stop');
}

/** 获取 Bot 服务器状态 */
export async function botServerStatus(): Promise<BotServerStatus> {
  if (!isTauri()) return { running: false, port: 0, message_count: 0, platforms_connected: [] };
  return tauriInvoke<BotServerStatus>('bot_server_status');
}

/** 启动微信桥接子进程 */
export async function startWechatBridge(): Promise<void> {
  if (!isTauri()) { console.log('[Dev] 微信桥接启动(模拟)'); return; }
  return tauriInvoke('start_wechat_bridge');
}

/** 停止微信桥接 */
export async function stopWechatBridge(): Promise<void> {
  if (!isTauri()) return;
  return tauriInvoke('stop_wechat_bridge');
}

/* ---------- 浏览器模式的文件树 mock ---------- */

function mockDirTree(): FileNode[] {
  return [
    {
      name: 'clawdesk', path: 'D:\\workspace\\clawdesk', is_dir: true, size: 0, ext: '',
      children: [
        {
          name: 'src', path: 'D:\\workspace\\clawdesk\\src', is_dir: true, size: 0, ext: '',
          children: [
            { name: 'App.tsx', path: 'D:\\workspace\\clawdesk\\src\\App.tsx', is_dir: false, children: [], size: 2048, ext: 'tsx' },
            { name: 'main.tsx', path: 'D:\\workspace\\clawdesk\\src\\main.tsx', is_dir: false, children: [], size: 512, ext: 'tsx' },
          ],
        },
        { name: 'package.json', path: 'D:\\workspace\\clawdesk\\package.json', is_dir: false, children: [], size: 1024, ext: 'json' },
        { name: 'README.md', path: 'D:\\workspace\\clawdesk\\README.md', is_dir: false, children: [], size: 4096, ext: 'md' },
      ],
    },
  ];
}
