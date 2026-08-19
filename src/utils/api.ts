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

// ── 系统 / 诊断 ──
export const systemApi = {
  lastError: () => invoke<{ message: string; location: string; logPath: string; timestamp?: string } | null>("app_last_error"),
  openInExplorer: (path: string) => invoke("win_open_in_explorer", { path }),
};
