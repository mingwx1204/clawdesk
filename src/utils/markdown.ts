// Markdown 渲染（从 App.vue 拆出）：渲染 + 缓存 + 转义，纯函数、无副作用。
import MarkdownIt from "markdown-it";

// Markdown 渲染（html:false 安全模式：不解析原始 HTML，仅渲染 Markdown 语法）
const md = new MarkdownIt({ html: false, linkify: true, breaks: true });

// ★ 放行 data:image 协议：允许 AI 回答里 ![图](data:image/png;base64,...) 直接渲染成图片
(md as unknown as { validateLink: (url: string) => boolean }).validateLink = (url: string) =>
  /^(https?:\/\/|data:image\/|mailto:|#)/i.test(url);

// ★ 渲染缓存：流式期间每条消息 content 变化时才重算，避免全列表重复渲染 Markdown
const mdCache = new Map<string, string>();

export function renderMd(text: string): string {
  if (mdCache.size > 400) mdCache.clear();
  const cached = mdCache.get(text);
  if (cached !== undefined) return cached;
  let out: string;
  try {
    out = md.render(text ?? "");
  } catch {
    out = text ?? "";
  }
  mdCache.set(text, out);
  return out;
}

// 用户消息转义（纯文本，防 XSS）
export function escapeHtml(text: string): string {
  return (text ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// 代码块渲染：带语言标签 + 复制按钮（点击走全局委托，兼容 Tauri CSP）
md.renderer.rules.fence = (tokens, idx) => {
  const token = tokens[idx];
  const lang = (token.info || "").trim().split(/\s+/)[0] || "";
  const content = token.content || "";
  const head =
    '<div class="code-head"><span class="code-lang">' + (escapeHtml(lang) || "code") +
    '</span><button class="code-copy">复制</button></div>';
  const code = '<pre class="code-pre"><code>' + escapeHtml(content) + '</code></pre>';
  return '<div class="code-block">' + head + code + '</div>';
};
