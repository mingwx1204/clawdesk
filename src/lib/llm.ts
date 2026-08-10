/**
 * LLM 流式调用客户端（OpenAI 兼容协议，覆盖 GLM 系列与自定义模型）。
 * 使用 fetch + ReadableStream 解析 SSE，支持 AbortController 中断。
 * 浏览器预览模式（无 API Key）时回退为本地模拟流式回复。
 */

import type { ChatMessage, ModelParams, ReasoningMode, Attachment } from '@/types';

/** 流式回调接口 */
export interface StreamCallbacks {
  onDelta: (text: string) => void;
  onReasoning?: (text: string) => void;
  onDone: () => void;
  onError: (err: string) => void;
}

/** 消息内容：纯文本或视觉数组 */
export type MessageContent = string | VisionContent[];

export interface VisionContent {
  type: 'text' | 'image_url';
  text?: string;
  image_url?: { url: string; detail?: 'auto' | 'low' | 'high' };
}

/** 聊天请求参数 */
export interface ChatRequest {
  apiBase: string;
  apiKey: string;
  model: string;
  messages: { role: string; content: MessageContent }[];
  params: ModelParams;
  mode: ReasoningMode;
  signal: AbortSignal;
}

/**
 * 按推理模式裁剪参数。
 * fast: 短回复低温度(1024 tokens, temp 0.3)
 * deep: 长回复高温度(8192+ tokens, temp 0.7)
 */
export function applyMode(params: ModelParams, mode: ReasoningMode): ModelParams {
  switch (mode) {
    case 'fast':
      return { ...params, maxTokens: Math.min(params.maxTokens, 1024), temperature: 0.3 };
    case 'deep':
      return { ...params, maxTokens: Math.max(params.maxTokens, 8192), temperature: 0.7 };
    default:
      return params;
  }
}

/** 核心流式聊天：POST SSE，逐行解析 data: 事件 */
export async function streamChat(req: ChatRequest, cb: StreamCallbacks): Promise<void> {
  // 无密钥 -> 离线演示模式
  if (!req.apiKey) return mockStream(req, cb);

  const p = applyMode(req.params, req.mode);
  const isDeepSeek = req.apiBase.includes('deepseek.com');
  try {
    const base = req.apiBase.replace(/\/+$/, '');
    const body: Record<string, unknown> = {
      model: req.model,
      messages: req.messages,
      temperature: p.temperature,
      max_tokens: p.maxTokens,
      top_p: p.topP,
      stream: true,
    };
    // DeepSeek 思考模式：temperature/top_p 会被忽略，无需发送
    if (isDeepSeek && req.mode === 'deep') {
      body.thinking = { type: 'enabled' };
      body.reasoning_effort = 'high';
      // 思考模式下 temperature/top_p 不生效，不发送以减少混淆
      delete body.temperature;
      delete body.top_p;
    } else if (isDeepSeek) {
      body.thinking = { type: 'disabled' };
    }
    const res = await fetch(`${base}/chat/completions`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${req.apiKey}`,
      },
      body: JSON.stringify(body),
      signal: req.signal,
    });

    if (!res.ok || !res.body) {
      cb.onError(`请求失败: HTTP ${res.status} ${await res.text().catch(() => '')}`);
      return;
    }

    // ReadableStream 读 SSE 分片
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() ?? '';
      for (const line of lines) {
        const t = line.trim();
        // DeepSeek keep-alive 注释，跳过
        if (!t || t.startsWith(':')) continue;
        if (!t.startsWith('data:')) continue;
        const data = t.slice(5).trim();
        if (data === '[DONE]') { cb.onDone(); return; }
        try {
          const json = JSON.parse(data);
          const delta = json.choices?.[0]?.delta?.content;
          if (typeof delta === 'string') cb.onDelta(delta);
          // 推理链（DeepSeek R1 等模型特有）
          const reasoning = json.choices?.[0]?.delta?.reasoning_content;
          if (typeof reasoning === 'string' && reasoning) cb.onReasoning?.(reasoning);
        } catch { /* 跨 chunk 不完整 JSON，忽略 */ }
      }
    }
    cb.onDone();
  } catch (e) {
    if ((e as Error).name === 'AbortError') { cb.onDone(); return; }
    cb.onError(`网络错误: ${(e as Error).message}`);
  }
}

/** 离线模拟流式回复：打字机效果，演示 Markdown/代码/公式 */
async function mockStream(req: ChatRequest, cb: StreamCallbacks): Promise<void> {
  const lastUser = [...req.messages].reverse().find((m) => m.role === 'user');
  const question = lastUser?.content ?? '';
  const reply = [
    `收到你的消息：「${question.slice(0, 80)}${question.length > 80 ? '…' : ''}」。\n`,
    '当前处于**离线演示模式**（未配置 API Key）：\n',
    '- 支持 **Markdown**、`代码高亮`、表格与公式\n',
    '- 在「设置 → 模型设置」填入 API Key 即可连接真实模型\n\n',
    '```ts\nfunction hello(name: string): string {\n  return `Hello, ${name}!`\n}\n```\n\n',
    '$$E = mc^2$$\n',
  ].join('');
  for (const ch of reply) {
    if (req.signal?.aborted) { cb.onDone(); return; }
    cb.onDelta(ch);
    await new Promise((r) => setTimeout(r, 12)); // 每字 12ms 打字机效果
  }
  cb.onDone();
}

/**
 * 构建 LLM 上下文消息数组。
 * @param systemPrompt 系统提示词（定义 AI 行为）
 * @param history 历史对话
 * @param maxHistory 最多保留条数，防 token 溢出
 */
export function buildContext(
  systemPrompt: string,
  history: ChatMessage[],
  maxHistory = 20,
): { role: string; content: MessageContent }[] {
  const msgs: { role: string; content: MessageContent }[] = [];
  if (systemPrompt) msgs.push({ role: 'system', content: systemPrompt });
  for (const m of history.slice(-maxHistory)) {
    if (m.role !== 'user' && m.role !== 'assistant' && m.role !== 'system') continue;
    // 如果有图片附件，构建 vision content 数组
    if (m.attachments?.some(a => a.kind === 'image')) {
      const parts: VisionContent[] = [];
      if (m.content) parts.push({ type: 'text', text: m.content });
      for (const a of m.attachments) {
        if (a.kind === 'image' && a.data) {
          parts.push({ type: 'image_url', image_url: { url: a.data, detail: 'auto' } });
        }
      }
      msgs.push({ role: m.role, content: parts });
    } else {
      msgs.push({ role: m.role, content: m.content });
    }
  }
  return msgs;
}
