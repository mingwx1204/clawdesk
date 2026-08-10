import { create } from 'zustand';
import type { AppSettings, BuiltinModel, CustomModel, ModelConfig } from '@/types';
import { getSetting, setSetting } from '@/lib/db';
import { registerGlobalShortcut, setAutoStart, winSetAlwaysOnTop } from '@/lib/backend';
import { encryptApiKeys, decryptApiKeys } from '@/lib/crypto';

/** 内置模型：DeepSeek 系列 */
export const BUILTIN_MODELS: BuiltinModel[] = [
  { id: 'deepseek-v4-pro', label: 'DeepSeek-V4-Pro', apiBase: 'https://api.deepseek.com', model: 'deepseek-v4-pro', builtin: true },
  { id: 'deepseek-v4-flash', label: 'DeepSeek-V4-Flash', apiBase: 'https://api.deepseek.com', model: 'deepseek-v4-flash', builtin: true },
];

export const DEFAULT_SETTINGS: AppSettings = {
  theme: 'dark',
  fontSize: 14,
  language: 'zh-CN',
  defaultModelId: '__auto__',
  defaultMode: 'standard',
  modelParams: { temperature: 0.7, maxTokens: 393216, topP: 0.9 },
  customModels: [],
  apiKeys: {},
  globalShortcut: 'Ctrl+Shift+O',
  permissionMode: 'confirm_each',
  autoStart: false,
  alwaysOnTop: false,
  closeToTray: true,
  botPlatform: {
    enabled: false,
    webhookPort: 19527,
    botName: 'ClawDesk',
    platforms: [
      { id: 'wechat', name: '微信', icon: '💬', enabled: false, connected: false, config: {}, description: '微信公众号/企业微信消息桥接' },
      { id: 'feishu', name: '飞书', icon: '🐦', enabled: false, connected: false, config: {}, description: '飞书机器人消息互通' },
      { id: 'webhook', name: 'Webhook', icon: '🪝', enabled: false, connected: false, config: {}, description: '通用 HTTP Webhook 接入' },
      { id: 'dingtalk', name: '钉钉', icon: '📌', enabled: false, connected: false, config: {}, description: '钉钉机器人消息互通' },
      { id: 'slack', name: 'Slack', icon: '💜', enabled: false, connected: false, config: {}, description: 'Slack Bot 消息集成' },
      { id: 'http-api', name: 'HTTP API', icon: '🔌', enabled: false, connected: false, config: {}, description: 'REST API 直接调用 ClawDesk' },
    ],
  },
  wechatBot: {
    apiBase: '',
    token: '',
    botName: 'ClawBot',
    pollIntervalSecs: 10,
  },
  soundEnabled: true,
  ttsEnabled: true,
  ttsVoice: '',
  customBackground: '',
  backgroundOpacity: 100,
  accentHue: 207,
  autoSaveChat: true,
  savePath: 'D:\\数据库',
  autoEvolve: true,
  mediaGen: {
    provider: 'pollinations',
    comfyuiUrl: 'http://127.0.0.1:8188',
    stabilityKey: '',
    replicateKey: '',
    defaultWidth: 1024,
    defaultHeight: 1024,
    defaultSteps: 20,
    defaultCfg: 7,
  },
  ollama: {
    enabled: false,
    baseUrl: 'http://127.0.0.1:11434',
    defaultModel: 'qwen2.5:7b',
  },
};

const SETTINGS_KEY = 'app-settings';

interface SettingsState {
  settings: AppSettings;
  loaded: boolean;
  load: () => Promise<void>;
  update: (patch: Partial<AppSettings>) => Promise<void>;
  allModels: () => ModelConfig[];
  resolveModel: (id: string) => ModelConfig | undefined;
  routeModel: (message: string) => ModelConfig;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: DEFAULT_SETTINGS,
  loaded: false,

  load: async () => {
    try {
      const raw = await getSetting(SETTINGS_KEY);
      if (raw) {
        const parsed = JSON.parse(raw) as Partial<AppSettings>;
        // 解密 API Keys（兼容旧的明文数据）
        if (parsed.apiKeys && Object.keys(parsed.apiKeys).length > 0) {
          parsed.apiKeys = await decryptApiKeys(parsed.apiKeys);
        }
        if (!parsed.apiKeys) parsed.apiKeys = {};
        // 迁移：旧 maxTokens < 100K 或超过 393216 → 设为 393216
        if (parsed.modelParams && (parsed.modelParams.maxTokens < 100000 || parsed.modelParams.maxTokens > 393216)) {
          parsed.modelParams.maxTokens = 393216;
        }
        // 迁移：重置异常色相值
        if (parsed.accentHue === undefined || parsed.accentHue === 360 || parsed.accentHue === 0) {
          parsed.accentHue = 207;
        }
        // 迁移：删除已废弃的 peakDoubleConsumption 字段
        delete (parsed as any).peakDoubleConsumption;
        set({ settings: { ...DEFAULT_SETTINGS, ...parsed }, loaded: true });
        return;
      }
    } catch { /* 使用默认设置 */ }
    // 首次启动：无已保存设置，写入默认值
    const defaults = { ...DEFAULT_SETTINGS };
    const toSave = { ...defaults };
    if (toSave.apiKeys && Object.keys(toSave.apiKeys).length > 0) {
      toSave.apiKeys = await encryptApiKeys(toSave.apiKeys);
    }
    await setSetting(SETTINGS_KEY, JSON.stringify(toSave)).catch(() => {});
    set({ loaded: true });
  },

  update: async (patch) => {
    const next = { ...get().settings, ...patch };
    set({ settings: next });
    // 加密 API Keys 后再持久化
    const toSave = { ...next };
    if (toSave.apiKeys && Object.keys(toSave.apiKeys).length > 0) {
      toSave.apiKeys = await encryptApiKeys(toSave.apiKeys);
    }
    await setSetting(SETTINGS_KEY, JSON.stringify(toSave)).catch(() => {});
    // 副作用同步到系统
    if (patch.autoStart !== undefined) await setAutoStart(patch.autoStart).catch(() => {});
    if (patch.alwaysOnTop !== undefined) await winSetAlwaysOnTop(patch.alwaysOnTop).catch(() => {});
    if (patch.globalShortcut) await registerGlobalShortcut(patch.globalShortcut).catch(() => {});
    if (patch.closeToTray !== undefined) {
      // 通知 Rust 后端更新关闭行为
      if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('set_close_to_tray', { flag: patch.closeToTray }).catch(() => {});
      }
    }
  },

  allModels: () => [...BUILTIN_MODELS, ...get().settings.customModels],

  resolveModel: (id) => get().allModels().find((m) => m.id === id),

  /** 智能路由：根据消息内容自动选择 Flash（省钱）还是 Pro（复杂任务） */
  routeModel: (message: string): ModelConfig => {
    const models = get().allModels();
    const pro = models.find(m => m.id === 'deepseek-v4-pro');
    const flash = models.find(m => m.id === 'deepseek-v4-flash');
    if (!pro && !flash) return models[0];

    // 路由规则
    const len = message.length;
    const hasCode = /```|function|class |import |def |fn |let |const |var |async |await/.test(message);
    const hasTool = /tool:|read_file|write_file|list_dir|run_command|search_files|delete|rename/.test(message);
    const hasComplex = /分析原因|原理是什么|为什么|如何实现|重构|设计模式|架构|底层|源码|debug|修复bug|错误排查|性能优化/.test(message);
    // 中文闲聊短语不触发复杂判定
    const isCasual = /^(你好|在吗|谢谢|好的|OK|行|可以|怎么样|怎么用|什么是|介绍|推荐).{0,30}$/.test(message.trim());

    // Flash：简单闲聊、短问答
    if (!hasComplex || isCasual) {
      if (len < 100 && !hasCode && !hasTool) {
        console.log(`[路由] → Flash: "${message.slice(0,30)}"`);
        return flash || pro!;
      }
    }
    // Pro：代码、工具、复杂推理、长文本
    console.log(`[路由] → Pro: "${message.slice(0,30)}" (len=${len} code=${hasCode} tool=${hasTool} complex=${hasComplex} casual=${isCasual})`);
    return pro || flash!;
  },

}));

