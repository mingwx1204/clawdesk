/**
 * Token 估算与上下文窗口配置。
 * 估算规则（无需加载大词表，性能零开销）：
 *   CJK 字符 ≈ 1 token/字，ASCII ≈ 1 token/4 字符，混合文本取两者之和。
 * 与真实分词误差通常在 ±15% 内，用于用量指示足够。
 */

/** 各模型的上下文窗口（tokens） */
export const CONTEXT_WINDOWS: Record<string, number> = {
  'deepseek-v4-pro': 1_048_576,
  'deepseek-v4-flash': 1_048_576,
  'glm-5.2': 1_048_576,
  'glm-5v-turbo': 65_536,
  'glm-5-turbo': 1_048_576,
  'deepseek-chat': 1_048_576,
};

export const DEFAULT_CONTEXT_WINDOW = 1_048_576;

/**
 * Token 估算与上下文窗口配置。
 * 对齐 DeepSeek 官方比例：英文 ≈ 0.3 token/字，中文 ≈ 0.6 token/字。
 */
export function estimateTokens(text: string): number {
  let cjk = 0;
  let ascii = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (code > 0x2e7f) cjk++;
    else ascii++;
  }
  // DeepSeek: 中文 0.6t/字, 英文 0.3t/字
  return Math.ceil(cjk * 0.6 + ascii * 0.3);
}

export function contextWindowFor(model: string): number {
  return CONTEXT_WINDOWS[model] ?? DEFAULT_CONTEXT_WINDOW;
}

export function formatTokens(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}
