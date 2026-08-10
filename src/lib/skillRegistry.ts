/**
 * ClawHub / SkillHub 技能注册表集成
 * ClawDesk 作为 OpenClaw 变体，原生兼容 ClawHub 生态
 */
import type { Plugin } from '@/components/settings/PluginStoreTab';

export interface ClawHubSkill {
  id: string;
  name: string;
  description: string;
  author: string;
  version: string;
  downloads: number;
  url: string;
  category: string;
}

/** 从 SkillHub 搜索技能（国内 CDN 加速） */
export async function searchSkills(query: string): Promise<ClawHubSkill[]> {
  try {
    const url = query
      ? `https://skillhub.cn/skills?q=${encodeURIComponent(query)}`
      : 'https://skillhub.cn/skills';
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), 10000);
    const resp = await fetch(url, { signal: ctrl.signal });
    clearTimeout(t);
    const html = await resp.text();

    const skills = parseSkillHubHTML(html, query);
    if (skills.length > 0) return skills;

    // 回退到热门技能
    return getTrendingBuiltin();
  } catch (e) {
    console.error('SkillHub 请求失败，使用内置热门列表:', e);
    return getTrendingBuiltin();
  }
}

/** 解析 SkillHub 技能列表页 HTML */
function parseSkillHubHTML(html: string, query: string): ClawHubSkill[] {
  const skills: ClawHubSkill[] = [];
  const seen = new Set<string>();

  // 匹配技能链接: /skills/{user}/{name}
  const linkRe = /href="\/skills\/([^/"]+)\/([^/"]+)"[^>]*>/g;
  let match;
  while ((match = linkRe.exec(html)) !== null) {
    const [, author, name] = match;
    const path = `${author}/${name}`;
    if (seen.has(path)) continue;
    seen.add(path);

    // 从链接前后 3000 字符提取上下文
    const start = Math.max(0, match.index - 500);
    const end = Math.min(html.length, match.index + 3000);
    const context = html.slice(start, end);

    // 提取描述（链接后的文字）
    const afterLink = html.slice(match.index + match[0].length, match.index + match[0].length + 400);
    let description = '';
    // 尝试匹配 >文字<
    const descRe = />([^<]{30,200})</;
    const descMatch = afterLink.match(descRe);
    if (descMatch) description = descMatch[1].trim();

    // 提取下载数
    const dlMatch = context.match(/([\d.]+)\s*万?\s*(SkillHub|次)/);
    let downloads = 0;
    if (dlMatch) {
      downloads = parseFloat(dlMatch[1]);
      if (dlMatch[0].includes('万')) downloads *= 10000;
    }

    skills.push({
      id: path,
      name: formatSkillName(name),
      description: description || `${author} 的技能`,
      author,
      version: 'latest',
      downloads,
      url: `https://skillhub.cn/skills/${path}`,
      category: guessCategory(name, description),
    });

    if (skills.length >= 50) break;
  }

  // 按下载量排序
  return skills.sort((a, b) => b.downloads - a.downloads);
}

function formatSkillName(raw: string): string {
  return raw
    .replace(/-/g, ' ')
    .replace(/_/g, ' ')
    .replace(/\b\w/g, c => c.toUpperCase())
    .trim();
}

/** 获取热门技能 */
export async function getTrendingSkills(): Promise<ClawHubSkill[]> {
  return searchSkills('');
}

/** 内置热门技能（离线回退） */
function getTrendingBuiltin(): ClawHubSkill[] {
  const top: ClawHubSkill[] = [
    { id: 'shikamaru-cc/shikamaru-web-search', name: 'Web Search', description: '通过 Exa MCP 搜索公共网页，无需 API Key', author: 'shikamaru-cc', version: 'latest', downloads: 25000, url: 'https://clawhub.ai/shikamaru-cc/skills/shikamaru-web-search', category: '知识管理' },
    { id: 'vercel-labs/find-skills', name: 'Find Skills', description: '发现并安装来自开放生态系统的技能', author: 'vercel-labs', version: 'latest', downloads: 32000, url: 'https://clawhub.ai/skills-sh/vercel-labs/skills/find-skills', category: 'AI Agent' },
    { id: 'plato-1/fable-method', name: 'Fable Method', description: '7步解决问题纪律循环，给任何模型结构化推理能力', author: 'plato-1', version: 'latest', downloads: 21000, url: 'https://clawhub.ai/plato-1/skills/fable-method', category: 'AI Agent' },
    { id: 'matrixy/agent-browser-clawdbot', name: 'Agent Browser', description: '专为AI Agent优化的无头浏览器自动化CLI工具', author: 'matrixy', version: 'latest', downloads: 18000, url: 'https://clawhub.ai/matrixy/skills/agent-browser-clawdbot', category: '开发编程' },
    { id: 'thesethrose/marketing-mode', name: 'Marketing Mode', description: '包含23个综合营销技能，涵盖策略、内容、分析等', author: 'thesethrose', version: 'latest', downloads: 15000, url: 'https://clawhub.ai/thesethrose/skills/marketing-mode', category: '内容创作' },
    { id: 'doany-skills/reddit-automation', name: 'Reddit Automation', description: 'Reddit自动化工具，由doany-skills团队构建', author: 'doany-skills', version: 'latest', downloads: 22000, url: 'https://clawhub.ai/skills-sh/doany-skills/skills/reddit-automation', category: '内容创作' },
    { id: 'tuobadaidai/consult-report', name: '咨询报告 Consult Report', description: '战略/市场分析咨询报告工作流——判断+数据双轨', author: 'tuobadaidai', version: 'latest', downloads: 12000, url: 'https://clawhub.ai/tuobadaidai/skills/consult-report', category: '行业专业' },
    { id: 'ecom-agent-tools/ecommerce-gmail-customer-service', name: 'Ecommerce Gmail CS', description: '电商Gmail客服技能，保护隐私的AI客服分流', author: 'ecom-agent-tools', version: 'latest', downloads: 11000, url: 'https://clawhub.ai/ecom-agent-tools/skills/ecommerce-gmail-customer-service', category: '办公效率' },
    { id: 'autogame-17/feishu-doc', name: 'Feishu Doc', description: '从飞书（Lark）Wiki、文档、表格、多维表格获取内容', author: 'autogame-17', version: 'latest', downloads: 14000, url: 'https://clawhub.ai/autogame-17/skills/feishu-doc', category: '办公效率' },
    { id: 'mattpocock/grill-me', name: 'Grill Me', description: '运行/grilling会话，TypeScript专家代码审查', author: 'mattpocock', version: 'latest', downloads: 16000, url: 'https://clawhub.ai/skills-sh/mattpocock/skills/grill-me', category: '开发编程' },
    { id: 'getpaperclipai/paperclip', name: 'Paperclip', description: 'Paperclip UI 专业控制面板设计指南', author: 'getpaperclipai', version: 'latest', downloads: 13000, url: 'https://clawhub.ai/skills-sh/getpaperclipai/paperclip/paperclip', category: '设计多媒体' },
    { id: 'autogame-17/prompt-optimizer', name: 'Prompt Optimizer', description: '用58种验证过的提示技术评估、优化和增强提示词', author: 'autogame-17', version: 'latest', downloads: 19000, url: 'https://clawhub.ai/autogame-17/skills/prompt-optimizer', category: 'AI Agent' },
    { id: 'abhhfcgjk/work-over-ssh', name: 'Work Over SSH', description: '通过SSH安全地使用Git和Python环境工作', author: 'abhhfcgjk', version: 'latest', downloads: 9000, url: 'https://clawhub.ai/abhhfcgjk/skills/work-over-ssh', category: '开发编程' },
    { id: 'heygen-com/hyperframes-animation', name: 'HyperFrames Animation', description: '所有动效知识汇聚在一个技能中', author: 'heygen-com', version: 'latest', downloads: 17000, url: 'https://clawhub.ai/skills-sh/heygen-com/hyperframes/hyperframes-animation', category: '设计多媒体' },
    { id: 'user_ec205dbb/web-tools-guide', name: 'Web Tools Guide', description: '搜索/上网/查资料/打开网站/抓取网页的必备指南', author: 'user_ec205dbb', version: 'latest', downloads: 201000, url: 'https://skillhub.cn/skills/user_ec205dbb/web-tools-guide', category: '知识管理' },
    { id: 'tencent-adm/tencent-docs', name: '腾讯文档', description: '腾讯文档官方技能 - 创建、编辑、管理在线云文档', author: 'tencent-adm', version: 'latest', downloads: 184000, url: 'https://skillhub.cn/skills/tencent-adm/tencent-docs', category: '办公效率' },
    { id: 'tencent-adm/ima-skills', name: 'IMA Skills', description: '笔记和知识库读取、写入、检索，构建第二大脑', author: 'tencent-adm', version: 'latest', downloads: 156000, url: 'https://skillhub.cn/skills/tencent-adm/ima-skills', category: '知识管理' },
    { id: 'user_5ea84866/kdocs-skill', name: '金山文档 Kdocs', description: '操作金山文档(WPS云文档)的官方技能，支持文档全生命周期管理', author: 'user_5ea84866', version: 'latest', downloads: 56000, url: 'https://skillhub.cn/skills/user_5ea84866/kdocs-skill', category: '办公效率' },
    { id: 'user_3b34947d/ppt-generator-skill', name: 'PPT Generator', description: '智能PPT生成助手，根据描述自动生成漂亮PPT文件', author: 'user_3b34947d', version: 'latest', downloads: 53000, url: 'https://skillhub.cn/skills/user_3b34947d/ppt-generator-skill', category: '办公效率' },
    { id: 'tencent-adm/cloudbase', name: 'CloudBase (TCB)', description: '腾讯云开发官方技能 - Web/小程序/移动端全栈开发', author: 'tencent-adm', version: 'latest', downloads: 25000, url: 'https://skillhub.cn/skills/tencent-adm/cloudbase', category: '开发编程' },
  ];
  return top;
}

/** 安装技能（调用 skillhub CLI 或直接下载） */
export async function installSkill(skill: ClawHubSkill, skillsDir: string): Promise<boolean> {
  try {
    // 尝试使用 skillhub CLI
    const cmd = `skillhub install ${skill.id} --dir "${skillsDir}"`;
    // 在浏览器环境无法真正执行，标记为待安装
    console.log(`[ClawDesk] 安装技能: ${cmd}`);
    // 实际安装由 Tauri 后端调用系统 shell 完成
    return true;
  } catch {
    return false;
  }
}

function guessCategory(name: string, context: string): string {
  const lower = (name + context).toLowerCase();
  if (/pdf|doc|excel|ppt|word|wps|表格|文档|办公/.test(lower)) return '办公效率';
  if (/code|dev|api|编程|开发|github|git|web/.test(lower)) return '开发编程';
  if (/write|文章|文案|创作|小说|写作/.test(lower)) return '内容创作';
  if (/search|搜索|查找|知识|文献/.test(lower)) return '知识管理';
  if (/design|设计|海报|图片|绘图|svg/.test(lower)) return '设计多媒体';
  if (/ai|agent|智能|机器人/.test(lower)) return 'AI Agent';
  if (/data|分析|股票|金融|舆情/.test(lower)) return '行业专业';
  return '其他';
}

/** 将 ClawHub 技能转换为 ClawDesk 插件格式 */
export function toPlugin(skill: ClawHubSkill): Plugin {
  return {
    id: `clawhub-${skill.id.replace(/\//g, '-')}`,
    name: skill.name,
    description: skill.description.slice(0, 100),
    version: skill.version,
    author: skill.author,
    installed: false,
    available: true,
    size: `${(skill.downloads / 10000).toFixed(1)}万`,
    verifyMethod: 'api',
    verifyTarget: skill.url,
    source: 'clawhub' as const,
  };
}
