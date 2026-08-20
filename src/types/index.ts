/** ClawDesk 全局类型定义 */

/** 推理模式：快速 / 标准 / 深度思考 */
export type ReasoningMode = 'fast' | 'standard' | 'deep';

/** 消息角色 */
export type MessageRole = 'user' | 'assistant' | 'system';

/** 内置模型定义 */
export interface BuiltinModel {
  id: string;
  label: string;
  apiBase: string;
  model: string;
  builtin: true;
}

/** 自定义模型（用户添加） */
export interface CustomModel {
  id: string;
  label: string;
  apiBase: string;
  apiKey: string;
  model: string;
  builtin: false;
}

export type ModelConfig = BuiltinModel | CustomModel;

/** 模型高级参数 */
export interface ModelParams {
  temperature: number;
  maxTokens: number;
  topP: number;
}

/** Agent 分身 */
export interface Persona {
  id: string;
  name: string;
  systemPrompt: string;
  modelId: string;
  mode: ReasoningMode;
  workdir: string;
  createdAt: number;
}

/** 对话 */
export interface Conversation {
  id: string;
  personaId: string;
  title: string;
  pinned: boolean;
  workdir: string;
  modelId: string;
  createdAt: number;
  updatedAt: number;
}

/** 消息附件 */
export interface Attachment {
  kind: 'image' | 'file';
  name: string;
  /** dataURL（图片）或文件路径（文件） */
  data: string;
  mime?: string;
}

/** 消息 */
export interface ChatMessage {
  id: string;
  convId: string;
  role: MessageRole;
  content: string;
  /** 推理模型的思考链（reasoning_content） */
  reasoning?: string;
  attachments?: Attachment[];
  createdAt: number;
  /** 流式输出中 */
  streaming?: boolean;
}

/** 文件树节点（与 Rust 端 FileNode 对应） */
export interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  children: FileNode[];
  size: number;
  ext: string;
}

/** 多平台 Bot 配置（内置 ClawDesk 引擎） */
export interface BotPlatform {
  id: string;
  name: string;
  icon: string;
  enabled: boolean;
  connected: boolean;
  config: Record<string, string>;
  description: string;
}

export interface BotPlatformConfig {
  /** 内置 Bot 服务器是否启用 */
  enabled: boolean;
  /** 各平台适配器 */
  platforms: BotPlatform[];
  /** 内置 webhook 端口 */
  webhookPort: number;
  /** Bot 名称 */
  botName: string;
}

/** 保留兼容旧版远程 OpenClaw 连接（逐步废弃） */
export interface WechatBotConfig {
  apiBase: string;
  token: string;
  botName: string;
  pollIntervalSecs: number;
}

/** 微信消息结构 */
export interface WechatMessage {
  msgId: string;
  fromUser: string;
  /** 消息文本（含语音云端转写 `[语音] …`、引用消息注记） */
  content: string;
  msgType: string;
  timestamp: number;
  /** iLink 消息上下文令牌（回复时必须携带） */
  contextToken?: string;
  /** 所属微信槽位（单账号固定为 0） */
  botSlot?: number;
  /** 图片本地路径（后端已下载解密，AI 用 analyze_image 读取） */
  images?: string[];
  /** 文件/语音/视频本地路径（AI 用 file_read 读取） */
  attachments?: string[];
  /** 语音云端转写文本（腾讯已转好；内容里已含 `[语音] …`，此字段供前端标记） */
  voiceTranscript?: string;
}

/** 微信 Bot 运行状态 */
export interface WechatBotState {
  running: boolean;
  connected: boolean;
  botName: string;
  lastPoll: number;
  messageCount: number;
  /** 是否已登录（本地保存了 bot token） */
  loggedIn?: boolean;
  botId?: string;
  /** 能力声明（对齐 AstrBot PlatformMetadata）：上层据此决定 UI 与行为 */
  capabilities?: {
    sendText: boolean;
    sendImage: boolean;
    receiveVoiceTranscript: boolean;
    receiveImages: boolean;
    receiveFiles: boolean;
    replyQuote: boolean;
    typing: boolean;
    proactive: boolean;
    groupChat: boolean;
    sendVoice: boolean;
  };
}

/** 微信登录二维码 */
export interface WechatQrResult {
  qrcode: string;
  qrcodeUrl: string;
}

/** 应用设置 */
export interface AppSettings {
  theme: 'dark' | 'light' | 'system';
  fontSize: number;
  language: 'zh-CN';
  defaultModelId: string;
  defaultMode: ReasoningMode;
  modelParams: ModelParams;
  customModels: CustomModel[];
  /** 各模型 ID -> API Key（内置模型的密钥也存这里） */
  apiKeys: Record<string, string>;
  globalShortcut: string; // 例如 "Ctrl+Shift+O"
  /** 权限模式：allow_all 全部允许；confirm_each 每次使用工具/插件前询问 */
  permissionMode: 'allow_all' | 'confirm_each';
  autoStart: boolean;
  alwaysOnTop: boolean;
  closeToTray: boolean;
  /** 内置 ClawDesk 多平台 Bot */
  botPlatform: BotPlatformConfig;
  /** @deprecated 保留兼容旧版远程 OpenClaw 连接 */
  wechatBot: WechatBotConfig;
  /** 音效开关 */
  soundEnabled: boolean;
  /** AI 朗读（TTS）—— 默认开启，AI 输出完自动朗读 */
  ttsEnabled: boolean;
  /** 朗读音色名称（空=自动中文） */
  ttsVoice: string;
  /** 自定义背景图 URL（空则无） */
  customBackground: string;
  /** 背景透明度 0-100 */
  backgroundOpacity: number;
  /** 自定义主题色（HSL hue，-1 为默认） */
  accentHue: number;
  /** 对话完成后自动保存到本地目录 */
  autoSaveChat: boolean;
  /** 自动保存路径（默认 D:\数据库） */
  savePath: string;
  /** 自动进化：对话结束后自动提取经验（消耗LLM额度） */
  autoEvolve: boolean;
  /** 媒体生成配置（文生图/图生图/文生视频/图生视频） */
  mediaGen: {
    provider: 'pollinations' | 'comfyui' | 'stability' | 'replicate';
    comfyuiUrl: string;
    stabilityKey: string;
    replicateKey: string;
    defaultWidth: number;
    defaultHeight: number;
    defaultSteps: number;
    defaultCfg: number;
  };
  /** llama.cpp 本地模型配置（llama-server 的 OpenAI 兼容端点，默认 http://127.0.0.1:8080/v1） */
  llamacpp: {
    enabled: boolean;
    baseUrl: string;
    defaultModel: string;
  };
}

/** 终端日志条目 */
export interface TerminalEntry {
  id: number;
  text: string; // 含 ANSI 转义
  ts: number;
}

// ─── 自我进化 Agent 类型 ───

/** 经验片段：从对话中提取的可复用知识 */
export interface Experience {
  id: string;
  /** 经验类别：bug_fix | code_pattern | workflow | knowledge | user_pref */
  category: ExperienceCategory;
  /** 触发关键词（用于语义匹配） */
  triggers: string[];
  /** 经验内容 */
  content: string;
  /** 示例代码（可选） */
  codeSnippet?: string;
  /** 成功使用次数 */
  useCount: number;
  /** 成功率 0-1 */
  successRate: number;
  /** 来源对话 ID */
  sourceConvId: string;
  createdAt: number;
  updatedAt: number;
}

export type ExperienceCategory =
  | 'bug_fix'      // Bug 修复方案
  | 'code_pattern' // 代码模式/最佳实践
  | 'workflow'     // 工作流程
  | 'knowledge'    // 领域知识
  | 'user_pref';   // 用户偏好

/** 技能：可执行的能力单元 */
export interface Skill {
  id: string;
  name: string;
  description: string;
  /** 技能类型 */
  type: 'tool' | 'prompt' | 'workflow';
  /** 技能内容：工具定义 / 提示词模板 / 工作流步骤 */
  definition: string;
  /** 参数 schema（JSON Schema） */
  paramsSchema?: Record<string, unknown>;
  /** 使用次数 */
  useCount: number;
  /** 成功率 */
  successRate: number;
  /** 是否自动激活 */
  autoActivate: boolean;
  /** 来源：generated(自动生成) | imported(导入) | manual(手动) */
  source: 'generated' | 'imported' | 'manual';
  /** 每次进化递增 */
  version: number;
  createdAt: number;
  updatedAt: number;
}

/** 进化事件：记录 Agent 的学习历程 */
export interface EvolutionEvent {
  id: string;
  type: 'experience_created' | 'skill_generated' | 'prompt_optimized' | 'reflection_completed' | 'error_learned';
  summary: string;
  /** 关联的经验/Skill ID */
  relatedId?: string;
  /** 改进指标 */
  metrics?: Record<string, number>;
  timestamp: number;
}

// ─── 永久记忆类型 ───

/** 对话记忆：AI 从每次对话中提取的永久记忆 */
export interface Memory {
  id: string;
  /** 记忆内容 */
  content: string;
  /** 触发关键词（用于快速匹配） */
  keywords: string[];
  /** 分类：tech | personal | preference | task | knowledge */
  category: 'tech' | 'personal' | 'preference' | 'task' | 'knowledge';
  /** 来源对话 ID */
  sourceConvId: string;
  /** 使用次数 */
  useCount: number;
  createdAt: number;
  updatedAt: number;
}
