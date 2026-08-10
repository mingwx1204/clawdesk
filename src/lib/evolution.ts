/**
 * 自我进化引擎 — ClawDesk Evolution Engine
 *
 * 设计原理（参考 Hermes + DeepSeek-R1 + GRPO）：
 *
 * 1. 经验提取：对话结束后，LLM 自省提取可复用经验
 * 2. 技能生成：识别重复成功模式 → 自动封装 Skill
 * 3. 自我反思：定时评估输出质量 → 优化策略
 * 4. 偏好学习：跟踪用户反馈信号 → 调整行为
 */

import type {
  Experience,
  ExperienceCategory,
  Skill,
  EvolutionEvent,
  ChatMessage,
} from '../types';
import { upsertExperience, listExperiences, upsertSkill, upsertEvolution } from './db';
import { useSettingsStore } from '../store/useSettingsStore';

// ─── UUID 生成 ───
const uid = () => crypto.randomUUID();

// ─── 反思用 System Prompt ───
const EXTRACT_PROMPT = `你是一个自我进化的 AI 助手。请从以下对话中提取可复用的经验。

## 提取规则
1. 如果用户解决了某个具体问题（Bug、配置、工作流），提取为 bug_fix 或 workflow
2. 如果出现了优秀的代码模式或最佳实践，提取为 code_pattern
3. 如果用户表达了明确的偏好或习惯，提取为 user_pref
4. 如果涉及领域知识（API、框架、工具用法），提取为 knowledge

## 输出格式（严格 JSON）
\`\`\`json
[
  {
    "category": "bug_fix|code_pattern|workflow|knowledge|user_pref",
    "triggers": ["触发关键词1", "触发关键词2"],
    "content": "可复用的经验描述（1-3句话）",
    "codeSnippet": "相关代码片段（可选，没有则省略此字段）"
  }
]
\`\`\`

如果没有可提取的经验，返回空数组 []。`;

const REFLECT_PROMPT = `你是一个自我反思的 AI 助手。评估以下对话片段的质量。

## 评估维度（1-10 分）
1. 回答准确性：是否正确解决了用户问题
2. 回答完整性：是否覆盖了所有关键点
3. 工具使用效率：工具调用是否合理、不冗余
4. 用户满意度信号：用户是否有正面反馈

## 输出格式（严格 JSON）
\`\`\`json
{
  "scores": { "accuracy": 8, "completeness": 7, "toolEfficiency": 9, "satisfaction": 8 },
  "summary": "一句话评估",
  "improvement": "可改进方向（没有则写 none）",
  "shouldOptimizePrompt": false
}
\`\`\``;

// ─── 核心函数 ───

/**
 * 从对话中提取经验（对话结束后自动调用）
 */
export async function extractExperiences(
  messages: ChatMessage[],
): Promise<Experience[]> {
  const settings = useSettingsStore.getState().settings;
  const content = messages
    .filter(m => m.role !== 'system')
    .map(m => `[${m.role}]: ${m.content}`)
    .join('\n\n');

  try {
    const result = await callLLMForExtraction(EXTRACT_PROMPT, content);
    const items = parseJSON(result);
    if (!Array.isArray(items) || items.length === 0) return [];

    return items.map((item: Record<string, unknown>) => ({
      id: uid(),
      category: (item.category as ExperienceCategory) || 'knowledge',
      triggers: Array.isArray(item.triggers) ? item.triggers as string[] : [],
      content: (item.content as string) || '',
      codeSnippet: item.codeSnippet as string | undefined,
      useCount: 0,
      successRate: 1.0,
      sourceConvId: messages[0]?.convId || '',
      createdAt: Date.now(),
      updatedAt: Date.now(),
    }));
  } catch {
    return [];
  }
}

/**
 * 自我反思：评估对话质量
 */
export async function reflect(messages: ChatMessage[]): Promise<{
  scores: Record<string, number>;
  summary: string;
  improvement: string;
  shouldOptimizePrompt: boolean;
} | null> {
  const content = messages
    .filter(m => m.role !== 'system')
    .slice(-10) // 只看最后 10 条
    .map(m => `[${m.role}]: ${m.content.slice(0, 500)}`)
    .join('\n\n');

  try {
    const result = await callLLMForExtraction(REFLECT_PROMPT, content);
    return parseJSON(result) as unknown as ReturnType<typeof reflect>;
  } catch {
    return null;
  }
}

/**
 * 自动生成技能：检测重复成功的工具调用模式
 */
export async function generateSkill(
  experiences: Experience[],
): Promise<Skill | null> {
  // 至少 2 条同类别经验才生成技能
  const grouped = new Map<ExperienceCategory, Experience[]>();
  for (const exp of experiences) {
    const group = grouped.get(exp.category) || [];
    group.push(exp);
    grouped.set(exp.category, group);
  }

  for (const [, group] of grouped) {
    if (group.length >= 2) {
      // 合并经验为技能
      const combined = group.map(e => e.content).join('\n');
      const snippet = group.find(e => e.codeSnippet)?.codeSnippet;

      return {
        id: uid(),
        name: `${group[0].category}_skill`,
        description: `从 ${group.length} 条经验自动生成的技能`,
        type: 'workflow',
        definition: combined,
        paramsSchema: {},
        useCount: 0,
        successRate: 1.0,
        autoActivate: false,
        source: 'generated',
        version: 1,
        createdAt: Date.now(),
        updatedAt: Date.now(),
      };
    }
  }

  return null;
}

/**
 * 提示词优化：根据反思结果微调 System Prompt
 */
export async function optimizePrompt(
  currentPrompt: string,
  reflectionSummary: string,
): Promise<string> {
  const optimizePrompt = `你是一个提示词优化器。根据反思建议优化以下 System Prompt。

## 当前 System Prompt
${currentPrompt}

## 反思建议
${reflectionSummary}

## 优化要求
- 保持原有角色和核心功能不变
- 融入反思建议的改进方向
- 输出完整的优化后 System Prompt（不要省略）

## 输出
直接输出优化后的完整 System Prompt：`;

  try {
    return await callLLMForExtraction(optimizePrompt, '');
  } catch {
    return currentPrompt;
  }
}

/**
 * 查找相关经验（基于关键词匹配 + 类别相关性）
 */
export function findRelevantExperiences(
  experiences: Experience[],
  userMessage: string,
  limit = 5,
): Experience[] {
  const lower = userMessage.toLowerCase();
  return experiences
    .filter(e => {
      if (e.successRate < 0.5) return false;
      return e.triggers.some(t => lower.includes(t.toLowerCase()));
    })
    .sort((a, b) => b.useCount * b.successRate - a.useCount * a.successRate)
    .slice(0, limit);
}

/**
 * 将经验注入到上下文（增强 AI 回答）
 */
export function injectExperiences(
  systemPrompt: string,
  relevant: Experience[],
): string {
  if (relevant.length === 0) return systemPrompt;

  const expBlock = relevant
    .map(
      (e, i) =>
        `[经验 ${i + 1}](${e.category}) ${e.content}${e.codeSnippet ? `\n代码:\n\`\`\`\n${e.codeSnippet}\n\`\`\`` : ''}`,
    )
    .join('\n\n');

  return `${systemPrompt}

---
## 🔄 相关历史经验（可参考）
${expBlock}

请参考以上经验来更好地回答当前问题。`;
}

// ─── 内部工具 ───

async function callLLMForExtraction(
  systemPrompt: string,
  userContent: string,
): Promise<string> {
  const settings = useSettingsStore.getState().settings;
  const model = settings.defaultModelId;
  const modelConfig = useSettingsStore.getState().resolveModel(model);

  if (!modelConfig) return '';

  return new Promise((resolve) => {
    let full = '';
    // 桌面端必须走 llmStream（Rust 后端绕过 CORS），浏览器端自动降级 streamChat
    void import('@/lib/backend').then(({ llmStream }) => {
      void llmStream(
      {
        apiBase: modelConfig.apiBase,
        apiKey: (modelConfig as { apiKey?: string }).apiKey ?? useSettingsStore.getState().settings.apiKeys[modelConfig.id] ?? '',
        model: modelConfig.model,
        messages: [
          { role: 'system', content: systemPrompt },
          { role: 'user', content: userContent || '请分析上述对话。' },
        ],
        params: { temperature: 0.3, maxTokens: 2048, topP: 0.9 },
        mode: 'fast',
        signal: new AbortController().signal,
      },
      {
        onDelta: (t: string) => { full += t; },
        onDone: () => resolve(full),
        onError: () => resolve(''),
      },
    );
  });
  });
}

function parseJSON(text: string): unknown {
  // 尝试提取 JSON 块
  const jsonMatch = text.match(/```(?:json)?\s*([\s\S]*?)```/);
  const jsonStr = jsonMatch ? jsonMatch[1].trim() : text.trim();
  try {
    return JSON.parse(jsonStr);
  } catch {
    // 尝试修复常见问题
    const cleaned = jsonStr.replace(/,\s*}/g, '}').replace(/,\s*]/g, ']');
    try {
      return JSON.parse(cleaned);
    } catch {
      return null;
    }
  }
}

// ─── 进化事件记录 ───

export async function logEvolution(event: Omit<EvolutionEvent, 'id'>): Promise<void> {
  await upsertEvolution({
    ...event,
    id: uid(),
  });
}

// ─── 主循环：对话完成后的完整进化流程 ───

export async function evolve(messages: ChatMessage[]): Promise<{
  newExperiences: Experience[];
  newSkill: Skill | null;
  reflection: Awaited<ReturnType<typeof reflect>>;
}> {
  // 1. 提取经验
  const newExperiences = await extractExperiences(messages);

  // 2. 保存经验到数据库
  for (const exp of newExperiences) {
    await upsertExperience(exp);
    await logEvolution({
      type: 'experience_created',
      summary: `提取经验: ${exp.content.slice(0, 100)}`,
      relatedId: exp.id,
      timestamp: Date.now(),
    });
  }

  // 3. 自我反思
  const reflection = await reflect(messages);
  if (reflection) {
    await logEvolution({
      type: 'reflection_completed',
      summary: reflection.summary,
      metrics: reflection.scores,
      timestamp: Date.now(),
    });
  }

  // 4. 尝试生成技能
  const allExperiences = await listExperiences(50);
  const newSkill = await generateSkill([...allExperiences, ...newExperiences]);
  if (newSkill) {
    await upsertSkill(newSkill);
    await logEvolution({
      type: 'skill_generated',
      summary: `生成技能: ${newSkill.name}`,
      relatedId: newSkill.id,
      timestamp: Date.now(),
    });
  }

  return { newExperiences, newSkill, reflection };
}
