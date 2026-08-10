/**
 * 微信 ClawBot 状态管理（Zustand Store）。
 * 管理微信 iLink Bot 连接生命周期：启动/停止/状态轮询。
 * 微信消息通过 Tauri 事件推送到此 Store，再分发到聊天。
 */

import { create } from 'zustand';
import {
  wechatBotStart,
  wechatBotStop,
  wechatBotStatus,
  wechatBotReply,
  wechatSendMessage,
  tauriListen,
} from '@/lib/backend';
import { useSettingsStore } from './useSettingsStore';
import type { WechatBotConfig, WechatMessage } from '@/types';

interface WechatBotStore {
  running: boolean;
  connected: boolean;
  loggedIn: boolean;
  botId: string;
  lastPoll: number;
  messageCount: number;
  loading: boolean;
  error: string;

  /** 消息回调：收到微信消息时触发 */
  onMessage: ((msg: WechatMessage) => void) | null;

  start: (config?: WechatBotConfig) => Promise<void>;
  stop: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  reply: (msgId: string, toUser: string, content: string, contextToken?: string) => Promise<void>;
  setOnMessage: (cb: ((msg: WechatMessage) => void) | null) => void;
}

let unlisten: (() => void) | null = null;

export const useWechatBotStore = create<WechatBotStore>((set, get) => ({
  running: false,
  connected: false,
  loggedIn: false,
  botId: '',
  lastPoll: 0,
  messageCount: 0,
  loading: false,
  error: '',
  onMessage: null,

  start: async (overrideConfig) => {
    set({ loading: true, error: '' });
    try {
      const config = overrideConfig || useSettingsStore.getState().settings.wechatBot;
      await wechatBotStart(config);

      // 监听 Tauri 推送的微信消息
      if (!unlisten) {
        unlisten = await tauriListen<WechatMessage>('wechat-message', (msg) => {
          const cb = get().onMessage;
          if (cb) cb(msg);
          set((s) => ({ messageCount: s.messageCount + 1 }));
        });
      }

      set({ running: true, connected: false, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  stop: async () => {
    await wechatBotStop().catch(() => {});
    if (unlisten) { unlisten(); unlisten = null; }
    set({ running: false, connected: false });
  },

  refreshStatus: async () => {
    const s = await wechatBotStatus().catch(() => ({ running: false, connected: false, botName: '', lastPoll: 0, messageCount: 0, loggedIn: false, botId: '' }));
    if (get().running && !s.running) {
      set({ running: false, connected: false });
    } else {
      set({
        connected: s.connected,
        messageCount: s.messageCount,
        lastPoll: s.lastPoll,
        loggedIn: s.loggedIn ?? false,
        botId: s.botId ?? '',
      });
    }
  },

  reply: async (msgId, toUser, content, contextToken) => {
    // 优先使用带 context_token 的发送入口
    if (contextToken) {
      await wechatSendMessage(toUser, content, contextToken).catch(async () => {
        await wechatBotReply(msgId, toUser, content);
      });
    } else {
      await wechatBotReply(msgId, toUser, content);
    }
  },

  setOnMessage: (cb) => set({ onMessage: cb }),
}));
