// ClawDesk 前端 → 后端 IPC 调用统一封装层（单一数据访问入口）
// 目的：把散落在各组件里的裸 invoke("命令", {参数}) 收敛为类型化函数，
//       便于维护、类型检查与统一错误处理。
import { invoke } from "@tauri-apps/api/core";

// ── 会话管理 ──
export const sessionsApi = {
  list: () => invoke<string[]>("agent_sessions"),
  messages: (sessionId: string) => invoke<any[]>("agent_session_messages", { sessionId }),
  metas: () => invoke<{ id: string; name?: string | null }[]>("agent_session_metas"),
  rename: (sessionId: string, newName: string) => invoke("agent_session_rename", { sessionId, newName }),
  delete: (id: string) => invoke("agent_session_delete", { sessionId: id }),
  clear: (sessionId: string) => invoke<boolean>("agent_session_clear", { sessionId }),
  compact: (sessionId: string, apiKey: string) =>
    invoke<any>("agent_session_compact", { sessionId, apiKey }),
  usage: (sessionId: string) => invoke<any>("agent_session_usage", { sessionId }),
  branches: (parentId: string) => invoke<string[]>("agent_branches", { parentId }),
  checkpoint: (sessionId: string) => invoke<unknown>("agent_checkpoint", { sessionId }),
  fork: (sourceId: string, newId: string) => invoke("agent_fork", { sourceId, newId }),
};

// ── 对话 / Agent ──
export const chatApi = {
  chat: (args: {
    apiKey: string; sessionId: string; runId: string; prompt: string;
    resume: boolean; images?: string[]; persona?: string | null;
  }) => invoke<any>("agent_chat", args),
  cancel: (runId: string) => invoke("agent_cancel", { runId }),
  confirmCall: (callId: string, approve: boolean) =>
    invoke("agent_confirm_call", { callId, approve }),
  maxRounds: () => invoke<number>("agent_get_max_rounds"),
};

// ── 设置 / Key ──
export const settingsApi = {
  get: () => invoke<any>("settings_get"),
  getKeys: () => invoke<{ main?: string; vision?: string; image?: string }>("settings_get_keys"),
  set: (patch: Record<string, unknown>) => invoke<any>("settings_set", { patch }),
};

// ── 路由 / 模型 ──
export const routerApi = {
  setMainModel: (model: string) => invoke("router_set_main_model", { model }),
  /** 自动检索 Key 支持的模型（OpenAI 兼容 /models 端点） */
  listModels: (apiKey: string, endpoint: string) => invoke<any>("list_models", { apiKey, endpoint }),
  checkBalance: (apiKey: string, endpoint: string) => invoke<any>("check_balance", { apiKey, endpoint }),
};

// ── 搜索 / 导出 ──
export const searchApi = {
  search: (keyword: string) => invoke<{ sessionId: string; role: string; content: string }[]>("session_search", { keyword }),
  export: (sessionId: string) => invoke<string>("session_export", { sessionId }),
};

// ── 文件快照（右侧面板文件变动预览）──
export const snapshotApi = {
  list: () => invoke<any[]>("snapshot_list"),
  diff: (snapshotId: string) => invoke<any>("snapshot_diff", { snapshotId }),
  restore: (snapshotId: string) => invoke<any>("snapshot_restore", { snapshotId }),
  remove: (snapshotId: string) => invoke<boolean>("snapshot_delete", { snapshotId }),
};

// ── 系统 / 诊断 ──
export const systemApi = {
  lastError: () => invoke<{ message: string; location: string; logPath: string; timestamp?: string } | null>("app_last_error"),
  openInExplorer: (path: string) => invoke("win_open_in_explorer", { path }),
  // 本地视觉服务（llama-server）是否就绪：供启动动画等待模型加载完成
  localVisionReady: () => invoke<boolean>("local_vision_ready"),
  // 本地视觉是否已安装（模型+llama-server）：无模型则跳过等待
  localVisionAvailable: () => invoke<boolean>("local_vision_available"),
};

// ── 微信 iLink Bot ──
export const wechatApi = {
  botStatus: () => invoke<any>("wechat_bot_status"),
  getQr: () => invoke<{ qrcode: string; qrcodeUrl: string }>("wechat_get_qr"),
  refreshQr: () => invoke<{ qrcode: string; qrcodeUrl: string }>("wechat_refresh_qr"),
  qrStatus: () => invoke<any>("wechat_qr_status"),
  verifyCode: (code: string) => invoke("wechat_verify_code", { code }),
  botStart: () => invoke("wechat_bot_start", { config: {} }),
  botStop: () => invoke("wechat_bot_stop"),
  logout: () => invoke("wechat_logout"),
  setPersona: (persona: string) => invoke("wechat_set_persona", { persona }),
  setProactive: (args: Record<string, unknown>) => invoke<any>("wechat_set_proactive", args),
  setBotRules: (args: Record<string, unknown>) => invoke<any>("wechat_set_bot_rules", args),
  history: () => invoke<{ records: any[] }>("wechat_history"),
  botReply: (args: Record<string, unknown>) => invoke("wechat_bot_reply", args),
  sendMessage: (args: Record<string, unknown>) => invoke("wechat_send_message", args),
  sendImage: (toUser: string, imagePath: string) => invoke("wechat_send_image", { toUser, imagePath }),
  sendVoice: (toUser: string, text: string) => invoke("wechat_send_voice", { toUser, text }),
  typing: (toUser: string, active: boolean) => invoke("wechat_typing", { toUser, active }),
  livingState: () => invoke<string>("wechat_living_state"),
  moodState: () => invoke<any>("wechat_mood_state"),
  soulSnapshot: () => invoke<any>("wechat_soul_snapshot"),
  livingContext: () => invoke<string>("wechat_living_context"),
  soulContext: () => invoke<string>("wechat_soul_context"),
  mobileQrSvg: (text: string) => invoke<string>("mobile_qr_svg", { text }),
};
