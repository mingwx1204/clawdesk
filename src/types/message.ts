// 聊天消息与工具调用卡片类型（从 App.vue 拆出，供消息区/工具卡复用）

export interface ToolCallInfo {
  toolId: string;
  arguments: unknown;
  status: "running" | "success" | "error" | "danger";
  output?: unknown;
  error?: string;
  open?: boolean; // 输出详情是否展开（默认收起；运行中自动展开）
}

export interface ChatMsg {
  id: string;
  role: "user" | "assistant";
  content: string;
  timestamp: number;
  thinking?: string; // 思考链（可折叠显示）
  thinkingOpen?: boolean;
  toolCalls?: ToolCallInfo[];
  images?: string[]; // dataUrl 预览
  attachments?: string[]; // 附件文件绝对路径（非图片，任意文件）
}
