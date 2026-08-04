/**
 * 从用户消息中提取可能的文件路径（Windows 如 D:\xxx 或 Unix 如 /home/xxx）。
 * 用于自动设置 AI 的工作目录，让 AI 直接针对用户提到的目录操作。
 */
export function extractPathsFromMessage(text: string): string[] {
  const paths: string[] = [];
  // Windows 绝对路径: D:\... 或 \\server\...
  const winRe = /[A-Za-z]:[/\\][^\s,，。；;]+/g;
  let m;
  while ((m = winRe.exec(text)) !== null) {
    const p = m[0].replace(/[/\\]+$/, '');
    if (p.length > 2) paths.push(p);
  }
  // Unix 绝对路径: /home/...
  const unixRe = /\/[^\s,，。；;]{2,}/g;
  while ((m = unixRe.exec(text)) !== null) {
    const p = m[0].replace(/\/+$/, '');
    if (!paths.includes(p)) paths.push(p);
  }
  return paths;
}

/** 尝试提取最可能的目录路径（优先最长的） */
export function guessWorkdirFromMessage(text: string): string {
  const paths = extractPathsFromMessage(text);
  if (paths.length === 0) return '';
  return paths.reduce((a, b) => (b.length > a.length ? b : a));
}

/**
 * 预执行：当用户消息中提到路径时，主动列出目录内容。
 * 这样 AI 在第一次回复时就能看到文件列表，直接开始工作。
 */
export async function preFetchDirContext(text: string): Promise<string> {
  const paths = extractPathsFromMessage(text);
  if (paths.length === 0) return '';
  // 取最长的路径（最可能是目录）
  const target = paths.reduce((a, b) => (b.length > a.length ? b : a));
  try {
    const tree = await fsReadDirTree(target);
    if (tree.length === 0) return '';
    const lines = tree.slice(0, 30).map((n) => {
      const icon = n.is_dir ? '📁' : '📄';
      return `${icon} ${n.name}`;
    });
    let result = `\n\n[预加载] 目录 "${target}" 的内容（${tree.length} 项）:\n${lines.join('\n')}`;
    if (tree.length > 30) result += `\n... 还有 ${tree.length - 30} 项`;
    return result;
  } catch {
    return ''; // 路径不存在或无权限，静默忽略
  }
}

/**
 * AI 工具调用系统：解析/执行 AI 响应中的工具调用。
 * 格式: ```tool:工具名\n{JSON参数}\n```
 */

import {
  fsReadFileText,
  fsWriteFileText,
  fsReadDirTree,
  fsRename,
  fsDelete,
  terminalSpawn,
  terminalWrite,
} from './backend';

export interface ToolCall {
  tool: string;
  params: Record<string, string>;
}

export interface ToolResult {
  call: ToolCall;
  success: boolean;
  output: string;
}

/** 从 AI 响应文本中提取工具调用，兼容多种格式变体 */
export function parseToolCalls(text: string): ToolCall[] {
  const calls: ToolCall[] = [];
  // 匹配 ```tool:xxx ... ``` 代码块
  const regex = /```(?:tool:(\w+))?\s*\n?([\s\S]*?)```/g;
  let match;
  while ((match = regex.exec(text)) !== null) {
    let tool = match[1];
    const body = match[2].trim();
    const parsed = tryParseTool(tool, body);
    if (parsed) calls.push(parsed);
  }
  // 也匹配 XML 风格: <tool:xxx>...</tool:xxx>
  const xmlRe = /<tool:(\w+)>\s*\n?([\s\S]*?)<\/tool:\1>/g;
  while ((match = xmlRe.exec(text)) !== null) {
    const tool = match[1];
    const body = match[2].trim();
    const parsed = tryParseTool(tool, body);
    if (parsed) calls.push(parsed);
  }
  // 也匹配裸格式: tool:xxx\n{...}
  const bareRe = /^tool:(\w+)\s*\n(\{[\s\S]*?\})/gm;
  while ((match = bareRe.exec(text)) !== null) {
    const tool = match[1];
    const body = match[2].trim();
    const parsed = tryParseTool(tool, body);
    if (parsed) calls.push(parsed);
  }
  return calls;
}

function tryParseTool(tool: string | null, body: string): ToolCall | null {
  if (!tool) {
    const toolMatch = body.match(/^tool[:\s]*(\w+)/m);
    if (toolMatch) tool = toolMatch[1];
    else return null;
  }
  try {
    const cleanBody = body.replace(/^[^{]*\{/, '{').replace(/\}[^}]*$/, '}');
    const params = JSON.parse(cleanBody) as Record<string, string>;
    return { tool, params };
  } catch {
    const params: Record<string, string> = {};
    const kvRe = /(\w+)\s*[:=]\s*(?:"([^"]*)"|'([^']*)'|(\S+))/g;
    let km;
    while ((km = kvRe.exec(body)) !== null) {
      const val = km[2] || km[3] || km[4] || '';
      params[km[1]] = val;
    }
    if (Object.keys(params).length > 0) return { tool, params };
    return null;
  }
}

/** 执行单个工具调用 */
export async function executeToolCall(
  call: ToolCall,
  workdir: string,
): Promise<ToolResult> {
  const { tool, params } = call;
  try {
    let output = '';
    switch (tool) {
      case 'read_file': {
        const path = resolvePath(params.path || params.file, workdir);
        const content = await fsReadFileText(path);
        output = `文件内容 (${path}):\n\`\`\`\n${content}\n\`\`\``;
        break;
      }
      case 'write_file': {
        const path = resolvePath(params.path || params.file, workdir);
        const content = params.content || '';
        await fsWriteFileText(path, content);
        output = `✅ 文件已写入: ${path} (${content.length} 字节)`;
        break;
      }
      case 'list_dir': {
        const path = resolvePath(params.path || params.dir || '.', workdir);
        const tree = await fsReadDirTree(path);
        const lines = tree.map((n) => {
          const icon = n.is_dir ? '📁' : '📄';
          const size = n.is_dir ? '' : ` (${formatSize(n.size)})`;
          return `${icon} ${n.name}${size}`;
        });
        output = `目录内容 (${path}, ${lines.length} 项):\n${lines.join('\n')}`;
        break;
      }
      case 'rename': {
        const oldPath = resolvePath(params.path || params.old_path, workdir);
        const newName = params.new_name || params.newName || '';
        await fsRename(oldPath, newName);
        output = `✅ 已重命名: ${oldPath} → ${newName}`;
        break;
      }
      case 'delete': {
        const path = resolvePath(params.path || params.file, workdir);
        await fsDelete(path);
        output = `✅ 已删除: ${path}`;
        break;
      }
      case 'run_command': {
        const rawCmd = params.command || params.cmd || '';
        const shell = (params.shell || 'cmd').toLowerCase(); // cmd | powershell | pwsh
        // 按 shell 类型包装命令，确保在正确的解释器中执行
        let cmd: string;
        if (shell === 'powershell' || shell === 'pwsh') {
          cmd = `powershell -NoProfile -Command "${rawCmd.replace(/"/g, '\\"')}"`;
        } else {
          // CMD: 直接执行，加 /c 确保执行完退出
          cmd = rawCmd.startsWith('cmd /c') ? rawCmd : `cmd /c "${rawCmd}"`;
        }
        const sessionId = await terminalSpawn(workdir);
        await terminalWrite(sessionId, cmd + '\n');
        // 说明 shell 类型，引导用户查看终端面板获取实际输出
        const shellLabel = shell === 'powershell' || shell === 'pwsh' ? 'PowerShell' : 'CMD';
        output = `✅ 已在 ${shellLabel} 终端执行: ${rawCmd}\n💡 查看「终端」面板获取完整输出（AI 无法直接读取终端输出，此为安全设计——所有终端命令需用户可见确认）。`;
        break;
      }
      case 'search_files': {
        const dir = resolvePath(params.dir || workdir, workdir);
        const pattern = params.pattern || '*';
        const tree = await fsReadDirTree(dir);
        const matches = filterFiles(tree, pattern.toLowerCase());
        output = `搜索结果 (${dir}, 匹配 "${pattern}", ${matches.length} 项):\n${matches.slice(0, 50).join('\n')}`;
        if (matches.length > 50) output += `\n... 还有 ${matches.length - 50} 个结果`;
        break;
      }
      case 'web_search': {
        const query = params.query || params.q || '';
        if (!query) { output = '❌ 缺少搜索关键词 (query)'; break; }
        const num = parseInt(params.num || '5', 10);
        const results = await webSearch(query, Math.min(num, 10));
        output = `🌐 联网搜索结果（"${query}"，${results.length} 条）:\n${results.map((r, i) => `${i + 1}. ${r.title}\n   ${r.snippet}\n   🔗 ${r.url}`).join('\n\n')}`;
        if (results.length === 0) output = `🌐 未找到 "${query}" 的相关结果`;
        break;
      }
      case 'local_search': {
        const query = params.query || '';
        if (!query) { output = '❌ 缺少搜索关键词 (query)'; break; }
        const { useWorkspaceStore } = await import('@/store/useWorkspaceStore');
        const { fileTree, workdir } = useWorkspaceStore.getState();
        const { searchLocalDocs } = await import('./rag');
        output = await searchLocalDocs(fileTree, query, workdir);
        if (!output) output = `📂 未找到与 "${query}" 匹配的本地文件`;
        break;
      }
      case 'agentic_search': {
        const query = params.query || '';
        if (!query) { output = '❌ 缺少搜索关键词 (query)'; break; }
        const { useWorkspaceStore } = await import('@/store/useWorkspaceStore');
        const { fileTree, workdir } = useWorkspaceStore.getState();
        const { agenticSearch } = await import('./agenticRag');
        const result = await agenticSearch(query, fileTree, workdir);
        output = `🔬 Agentic RAG 探索结果 (${result.totalSteps}轮, ${result.timeMs}ms, ${result.usedAdaptiveRoutine ? 'Adaptive Routine' : 'Pipeline'})\n`;
        output += `📊 最终相关性: ${result.finalRelevance}\n\n`;
        for (const step of result.steps) {
          output += `## ${step.action}\n${step.reasoning}\n${step.result ? step.result.slice(0, 300) + (step.result.length > 300 ? '...' : '') : ''}\n---\n`;
        }
        break;
      }
      case 'todo': {
        const { useTodoStore } = await import('@/store/useTodoStore');
        output = useTodoStore.getState().handleToolCall(params);
        break;
      }
      case 'text2img':
      case 'img2img':
      case 'text2video':
      case 'img2video': {
        const { useSettingsStore: st } = await import('@/store/useSettingsStore');
        const cfg = st.getState().settings.mediaGen;
        const { textToImage, imageToImage, textToVideo: t2v, imageToVideo: i2v } = await import('./mediaGen');
        const params_: any = {
          prompt: params.prompt || params.query || '',
          negativePrompt: params.negative || '',
          width: parseInt(params.width || String(cfg.defaultWidth)),
          height: parseInt(params.height || String(cfg.defaultHeight)),
          steps: parseInt(params.steps || String(cfg.defaultSteps)),
          cfg: parseFloat(params.cfg || String(cfg.defaultCfg)),
          seed: params.seed ? parseInt(params.seed) : undefined,
          initImage: params.image || params.init_image || undefined,
          numFrames: params.frames ? parseInt(params.frames) : undefined,
          fps: params.fps ? parseInt(params.fps) : undefined,
        };
        let result;
        switch (tool) {
          case 'text2img': result = await textToImage(cfg, params_); break;
          case 'img2img': result = await imageToImage(cfg, params_); break;
          case 'text2video': result = await t2v(cfg, params_); break;
          case 'img2video': result = await i2v(cfg, params_); break;
          default: result = { images: [], provider: 'unknown', seed: 0 };
        }
        output = `🎨 文生图完成！\n提供商: ${result.provider}\nSeed: ${result.seed}\n`;
        if (result.images.length > 0) {
          output += result.images.map((img, i) => `\n![生成图片 ${i + 1}](${img})`).join('');
        }
        break;
      }
      default:
        return { call, success: false, output: `未知工具: ${tool}` };
    }
    return { call, success: true, output };
  } catch (e) {
    return {
      call,
      success: false,
      output: `❌ 执行失败: ${(e as Error).message}`,
    };
  }
}

/** 批量执行工具调用，返回带元数据的汇总结果（Agentic Search 增强） */
export async function executeToolCalls(
  calls: ToolCall[],
  workdir: string,
): Promise<string> {
  const results: string[] = [];
  for (const call of calls) {
    const startTime = Date.now();
    const result = await executeToolCall(call, workdir);
    const elapsed = Date.now() - startTime;
    // 元数据标记：来源 + 耗时 + 可信度提示
    const lines = result.output.split('\n').length;
    let confidence = 'high';
    if (result.output.includes('⚠️') || result.output.includes('error')) confidence = 'low';
    else if (lines > 200) confidence = 'medium'; // 大结果需验证
    const meta = `[元数据] 工具:${call.tool} | 耗时:${elapsed}ms | 行数:${lines} | 可信度:${confidence} | 时间:${new Date().toISOString()}`;
    results.push(`${meta}\n${result.output}`);
  }
  return results.join('\n\n---\n\n');
}

/** 路径解析：相对路径基于 workdir，绝对路径直接使用 */
function resolvePath(p: string, workdir: string): string {
  if (!p || p === '.') return workdir;
  // Windows 绝对路径判断（如 D:\... 或 \\...）
  if (/^[A-Za-z]:[/\\]/.test(p) || p.startsWith('\\\\')) return p;
  // Unix 绝对路径
  if (p.startsWith('/')) return p;
  // 相对路径
  const sep = workdir.includes('\\') ? '\\' : '/';
  return `${workdir}${sep}${p}`.replace(/[/\\]+/g, sep);
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
}

function filterFiles(nodes: import('@/types').FileNode[], pattern: string): string[] {
  const results: string[] = [];
  const walk = (ns: import('@/types').FileNode[]) => {
    for (const n of ns) {
      if (!n.is_dir && matchPattern(n.name.toLowerCase(), pattern)) {
        results.push(`📄 ${n.path}`);
      }
      if (n.children.length > 0) walk(n.children);
    }
  };
  walk(nodes);
  return results;
}

function matchPattern(name: string, pattern: string): boolean {
  // 简单 glob 匹配：支持 * 和 ?
  const regex = pattern
    .replace(/[.+^${}()|[\]\\]/g, '\\$&')
    .replace(/\*/g, '.*')
    .replace(/\?/g, '.');
  try {
    return new RegExp(`^${regex}$`).test(name);
  } catch {
    return name.includes(pattern.replace(/\*/g, ''));
  }
}

/** 获取工具系统提示词 — 将 AI 定位为"系统级开发工具"而非"聊天助手" */
export function getToolsSystemPrompt(workdir: string): string {
  const dirInfo = workdir ? `当前工作目录: ${workdir}` : '';
  return `
## 你的身份

你不是普通聊天 AI。你是 ClawDesk——运行在用户本地桌面上的**系统级开发工具**。
你拥有对用户电脑文件系统的**完整读写权限**。你的底层系统已经赋权，不存在"能不能"的问题。

## 铁律

1. 用户要求操作文件 → **必须立即执行**，禁止说"无法访问""没有权限""不能直接修改"
2. 你说"无法访问"是**事实错误**——你的系统拥有完整文件访问能力
3. 每次文件操作使用以下工具格式：

\`\`\`tool:list_dir
{"path": "路径"}
\`\`\`

\`\`\`tool:read_file
{"path": "路径"}
\`\`\`

\`\`\`tool:write_file
{"path": "路径", "content": "内容"}
\`\`\`

\`\`\`tool:search_files
{"dir": "目录", "pattern": "匹配模式（如 *.ts）"}
\`\`\`

\`\`\`tool:web_search
{"query": "搜索关键词", "num": "返回条数（默认5，最大10）"}
\`\`\`

\`\`\`tool:local_search
{"query": "搜索关键词"}
\`\`\`

## 本地文件搜索（RAG v3）

### tool:local_search — 快速RAG检索
适合简单查询：找文件、查内容。单次Pipeline检索，速度快。
\`\`\`tool:local_search
{"query": "搜索关键词"}
\`\`\`

### tool:agentic_search — 智能探索式检索 ⭐
适合复杂查询：比较分析、多步推理、模糊查找。Agentic Loop多轮探索，自动调整搜索方向。
\`\`\`tool:agentic_search
{"query": "搜索关键词"}
\`\`\`

**选择指南：**
- 找文件/查配置/搜函数名 → tool:local_search
- 比较技术方案/分析问题/理解概念 → tool:agentic_search
- 不确定用什么 → tool:agentic_search（会自动降级）

## 图片与视频生成

你可以直接生成图片和视频！当用户要求画图、生成图片、做视频时使用以下工具：

\`\`\`tool:text2img
{"prompt": "英文提示词，描述要生成的画面", "negative": "不想出现的内容（可选）", "width": "1024", "height": "1024", "steps": "20"}
\`\`\`

\`\`\`tool:img2img
{"prompt": "修改描述", "image": "用户上传的图片base64（可选）"}
\`\`\`

\`\`\`tool:text2video
{"prompt": "视频描述"}
\`\`\`

\`\`\`tool:img2video
{"prompt": "动效描述", "image": "用户上传的图片base64"}
\`\`\`

**重要：当用户说"画一张""生成图片""做图"时，必须使用 tool:text2img！**
提示词请翻译成英文以获得最佳效果。用户说中文你就翻译成英文填入 prompt。

\`\`\`tool:todo
{"action": "add", "items": "任务1\\n任务2\\n任务3"}
\`\`\`
\`\`\`tool:todo
{"action": "done", "id": "0"}
\`\`\`

## 待办事项规则

当用户要求多步骤任务时，先用 tool:todo 创建待办清单，然后逐个执行。
每完成一项，立即用 tool:todo done 标记完成，再处理下一项。
全部完成后不需要额外汇报——系统面板会自动显示进度。

\`\`\`tool:rename
{"path": "原路径", "new_name": "新名称"}
\`\`\`

\`\`\`tool:delete
{"path": "路径"}
\`\`\`

\`\`\`tool:run_command
{"command": "命令", "shell": "cmd或powershell（可选，默认cmd）"}
\`\`\`

## 终端说明

- 默认使用 **CMD**（Windows 命令提示符），与系统自带的 cmd.exe 一致
- 如需 PowerShell，加 \`"shell": "powershell"\`
- 系统同时装了 CMD 和 PowerShell——两者都是真实的系统 Shell，不是模拟的
- 命令在独立终端会话中执行，输出显示在「终端」面板中

${dirInfo}

## Agentic Search 规则（主动探索模式）

每个工具返回结果前会附带元数据：工具名、耗时、可信度。
- 可信度=low → 结果可能有误，需要交叉验证或换方式重试
- 搜索结果不完整时 → 换关键词再搜，不要直接放弃
- 多步任务时 → 每步结果出来后判断是否达到目标，再决定下一步

现在执行用户的指令。`;
}

// ─── 联网搜索 ───

interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}

/** 使用 DuckDuckGo HTML 搜索（无需 API Key），返回标题+摘要+链接 */
async function webSearch(query: string, num = 5): Promise<SearchResult[]> {
  try {
    const url = `https://html.duckduckgo.com/html/?q=${encodeURIComponent(query)}`;
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), 8000);
    const resp = await fetch(url, { signal: ctrl.signal });
    clearTimeout(t);
    const html = await resp.text();

    // 解析搜索结果
    const results: SearchResult[] = [];
    const linkRe = /<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>([^<]*)<\/a>/gi;
    const snippetRe = /<a[^>]*class="result__snippet"[^>]*>([\s\S]*?)<\/a>/gi;

    let linkMatch: RegExpExecArray | null;
    const links: { title: string; url: string }[] = [];
    while ((linkMatch = linkRe.exec(html)) !== null) {
      const rawUrl = linkMatch[1].replace(/\/\/duckduckgo\.com\/l\/\?uddg=/, '').replace(/&rut=.*/, '');
      const url2 = decodeURIComponent(rawUrl);
      const title = linkMatch[2].replace(/<[^>]*>/g, '').trim();
      if (url2.startsWith('http') && title) links.push({ title, url: url2 });
    }

    const snippets: string[] = [];
    let sm: RegExpExecArray | null;
    const snippetRe2 = /class="result__snippet"[^>]*>([\s\S]*?)<\/a>/gi;
    while ((sm = snippetRe2.exec(html)) !== null) {
      snippets.push(sm[1].replace(/<[^>]*>/g, '').trim());
    }

    // 回退正则
    if (snippets.length === 0) {
      const snippetRe3 = /<span[^>]*class="[^"]*snippet[^"]*"[^>]*>([\s\S]*?)<\/span>/gi;
      while ((sm = snippetRe3.exec(html)) !== null) {
        snippets.push(sm[1].replace(/<[^>]*>/g, '').trim());
      }
    }

    for (let i = 0; i < Math.min(links.length, num); i++) {
      results.push({
        title: links[i].title,
        url: links[i].url,
        snippet: snippets[i] || '(无摘要)',
      });
    }

    return results;
  } catch (e) {
    console.error('webSearch error:', e);
    return [];
  }
}
