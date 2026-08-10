import { memo, useEffect, useMemo, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import { Check, Copy } from 'lucide-react';
import 'katex/dist/katex.min.css';

/**
 * Markdown 渲染：GFM 表格/删除线 + KaTeX 公式 + Shiki 代码高亮。
 * Shiki 按需懒加载语言，高亮结果缓存，避免流式输出时重复高亮造成卡顿。
 */

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let highlighterPromise: Promise<any> | null = null;
const loadedLangs = new Set<string>();
const htmlCache = new Map<string, string>();
const CACHE_LIMIT = 500;

const COMMON_LANGS = ['typescript', 'javascript', 'tsx', 'jsx', 'python', 'rust', 'json', 'bash', 'html', 'css', 'markdown', 'yaml', 'sql', 'cpp', 'c', 'go', 'java'];

async function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = import('shiki').then((shiki) =>
      shiki.createHighlighter({
        themes: ['github-dark-default', 'github-light-default'],
        langs: [],
      }),
    );
  }
  return highlighterPromise;
}

async function highlight(code: string, lang: string, dark: boolean): Promise<string> {
  const key = `${dark ? 'd' : 'l'}:${lang}:${code.length}:${hash(code)}`;
  const hit = htmlCache.get(key);
  if (hit) return hit;
  const hl = await getHighlighter();
  const useLang = COMMON_LANGS.includes(lang) ? lang : 'text';
  if (useLang !== 'text' && !loadedLangs.has(useLang)) {
    await hl.loadLanguage(useLang);
    loadedLangs.add(useLang);
  }
  const html = hl.codeToHtml(code, {
    lang: useLang,
    theme: dark ? 'github-dark-default' : 'github-light-default',
  });
  if (htmlCache.size >= CACHE_LIMIT) htmlCache.delete(htmlCache.keys().next().value as string);
  htmlCache.set(key, html);
  return html;
}

function hash(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return h;
}

const CodeBlock = memo(function CodeBlock({ lang, code }: { lang: string; code: string }) {
  const [html, setHtml] = useState<string>('');
  const [copied, setCopied] = useState(false);
  const dark = document.documentElement.classList.contains('dark');

  useEffect(() => {
    let alive = true;
    // 流式期间先用纯文本，停顿 200ms 后再高亮（防抖，避免逐字重排）
    const t = setTimeout(() => {
      void highlight(code, lang, dark).then((h) => { if (alive) setHtml(h); });
    }, 200);
    return () => { alive = false; clearTimeout(t); };
  }, [code, lang, dark]);

  const doCopy = () => {
    void navigator.clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <div className="group/code my-2 overflow-hidden rounded-lg border border-border">
      <div className="flex items-center justify-between bg-muted px-3 py-1.5">
        <span className="text-[11px] font-mono text-muted-foreground">{lang || 'text'}</span>
        <button
          className="flex items-center gap-1 text-[11px] text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover/code:opacity-100"
          onClick={doCopy}
        >
          {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
          {copied ? '已复制' : '复制'}
        </button>
      </div>
      {html ? (
        <div
          className="shiki-wrapper overflow-x-auto text-[13px] [&_pre]:!bg-transparent [&_pre]:p-3"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre className="overflow-x-auto p-3 text-[13px]"><code>{code}</code></pre>
      )}
    </div>
  );
});

export const MarkdownView = memo(function MarkdownView({ content }: { content: string }) {
  const components = useMemo(
    () => ({
      // 代码块与行内代码
      code(props: { className?: string; children?: React.ReactNode }) {
        const { className, children } = props;
        const text = String(children ?? '').replace(/\n$/, '');
        const match = /language-(\w+)/.exec(className ?? '');
        if (match || text.includes('\n')) {
          return <CodeBlock lang={match?.[1] ?? ''} code={text} />;
        }
        return <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em]">{text}</code>;
      },
      // 表格自适应
      table(props: { children?: React.ReactNode }) {
        return (
          <div className="my-2 overflow-x-auto">
            <table className="w-full border-collapse text-sm">{props.children}</table>
          </div>
        );
      },
      // 图片懒加载 + 点击放大
      img(props: { src?: string; alt?: string }) {
        return (
          <img
            src={props.src}
            alt={props.alt ?? ''}
            loading="lazy"
            className="my-2 max-h-80 cursor-zoom-in rounded-lg border border-border"
            onClick={() => window.open(props.src, '_blank')}
          />
        );
      },
    }),
    [],
  );

  return (
    <div className="markdown-body text-sm leading-relaxed">
      <ReactMarkdown remarkPlugins={[remarkGfm, remarkMath]} rehypePlugins={[rehypeKatex]} components={components}>
        {content}
      </ReactMarkdown>
    </div>
  );
});
