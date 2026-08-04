/**
 * 永久记忆引擎 — 从对话中提取记忆，注入到后续对话
 * 让 AI 记住跨会话的所有关键信息
 */
import type { Memory, ChatMessage } from '@/types';
import { uid } from '@/components/settings/BalanceChecker';

const MEMORY_EXTRACT_PROMPT = `你是一个记忆提取器。从以下对话中提取用户的关键信息。

输出 JSON 数组，每条记忆包含:
- content: 一句话总结 (str)
- keywords: 3-5个触发关键词 (str[])
- category: "tech"|"personal"|"preference"|"task"|"knowledge"

规则:
1. 只提取可能对后续对话有用的信息(用户偏好、技术决策、项目信息、重要事件)
2. 不要提取临时/闲聊内容
3. 每条记忆20-80字
4. 如果没有值得记住的信息，返回空数组 []

对话内容:
`;

/**
 * 从对话中提取记忆（调用LLM）
 */
export async function extractMemories(
  messages: ChatMessage[],
  llmCall: (prompt: string) => Promise<string>,
): Promise<Omit<Memory, 'id' | 'useCount' | 'createdAt' | 'updatedAt'>[]> {
  const convText = messages
    .filter((m) => m.role !== 'system')
    .map((m) => `[${m.role}]: ${m.content.slice(0, 500)}`)
    .join('\n\n');

  if (convText.length < 100) return [];

  try {
    const result = await llmCall(MEMORY_EXTRACT_PROMPT + convText);
    // Parse JSON from LLM response
    const jsonMatch = result.match(/\[[\s\S]*\]/);
    if (!jsonMatch) return [];
    const items = JSON.parse(jsonMatch[0]) as Array<{
      content: string; keywords: string[]; category: Memory['category'];
    }>;
    return items.map((item) => ({
      content: item.content,
      keywords: item.keywords || [],
      category: item.category || 'knowledge',
      sourceConvId: '',
    }));
  } catch {
    return [];
  }
}

/**
 * 根据关键词匹配相关记忆
 */
export function findRelevantMemories(
  memories: Memory[],
  query: string,
  maxResults = 5,
  currentConvId?: string,
): Memory[] {
  const now = Date.now();
  const queryLower = query.toLowerCase();
  const scored = memories.map((m) => {
    let score = 0;
    // Keyword match
    for (const kw of m.keywords) {
      if (queryLower.includes(kw.toLowerCase())) score += 10;
    }
    // Content word overlap
    const contentWords = m.content.toLowerCase().split(/\s+/);
    for (const w of contentWords) {
      if (w.length > 1 && queryLower.includes(w)) score += 3;
    }
    // Boost frequently used memories
    score += Math.min(m.useCount, 5);

    // 🔧 对话来源感知：
    // - 当前对话的记忆大幅加分（这些是最相关的上下文）
    // - 旧对话中的"task"类记忆大幅降分（避免串出未完成任务）
    if (currentConvId && m.sourceConvId === currentConvId) {
      score *= 2.5; // 当前对话记忆优先
    } else if (m.category === 'task' && currentConvId && m.sourceConvId !== currentConvId) {
      score *= 0.2; // 旧对话的未完成任务大幅降权，避免AI"串台"
    }

    // 时效衰减：超过7天的记忆得分降低
    const ageDays = (now - m.updatedAt) / (1000 * 60 * 60 * 24);
    if (ageDays > 7) score *= Math.max(0.3, 1 - (ageDays - 7) / 30);

    return { memory: m, score };
  });

  return scored
    .filter((s) => s.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, maxResults)
    .map((s) => s.memory);
}

/**
 * 将记忆注入到系统提示词
 */
export function injectMemories(
  systemPrompt: string,
  memories: Memory[],
  currentConvId?: string,
): string {
  if (memories.length === 0) return systemPrompt;

  const currentMsgs = memories.filter(m => m.sourceConvId === currentConvId);
  const historicalMsgs = memories.filter(m => m.sourceConvId !== currentConvId);

  const parts: string[] = [];

  if (currentMsgs.length > 0) {
    parts.push('### 当前对话相关记忆\n' + currentMsgs.map((m) =>
      `- [${m.category}] ${m.content}`
    ).join('\n'));
  }

  if (historicalMsgs.length > 0) {
    parts.push('### 历史长期记忆\n' + historicalMsgs.map((m) =>
      `- [${m.category}] ${m.content}`
    ).join('\n'));
  }

  return `${systemPrompt}

## 用户永久记忆
${parts.join('\n\n')}

注意：
- "当前对话相关记忆"与本次对话直接相关，优先级最高
- "历史长期记忆"是跨会话的知识/偏好，仅供参考
- 如果记忆中的信息与用户当前说法不一致，以用户当前说法为准。`;
}

/**
 * 本地关键词提取（不用LLM的轻量版本）
 */
export function extractKeywordsLocal(text: string): string[] {
  const stopWords = new Set(['的', '了', '是', '在', '我', '有', '和', '就', '不', '人', '都', '一', '一个', '上', '也', '很', '到', '说', '要', '去', '你', '会', '着', '没有', '看', '好', '自己', '这']);
  const words = text.replace(/[，。！？、；：""''（）【】\n\r]/g, ' ').split(/\s+/);
  const freq: Record<string, number> = {};
  for (const w of words) {
    if (w.length < 2 || stopWords.has(w)) continue;
    freq[w] = (freq[w] || 0) + 1;
  }
  return Object.entries(freq)
    .sort((a, b) => b[1] - a[1])
    .slice(0, 5)
    .map(([k]) => k);
}

/* ═══════════════════════════════════════════════════════════════
 *  v2 升级：重要性评分 + 记忆图谱 + 记忆合并 + HyDE 策略
 * ═══════════════════════════════════════════════════════════════ */

/**
 * 记忆重要性评分
 * 
 * 多维度计算：
 * - 频率得分：使用次数越多越重要（对数的，防止过拟合）
 * - 时效得分：越近越重要（指数衰减，半衰期7天）
 * - 关键词密度得分：关键词越多越具体
 * - 内容长度得分：适中的内容最重要
 */
export function scoreImportance(m: Memory, now = Date.now()): number {
  const ageHours = (now - m.updatedAt) / (1000 * 60 * 60);
  const HALF_LIFE_HOURS = 7 * 24; // 7天半衰期

  // 频率得分（log防止单条记忆过拟合）
  const freqScore = Math.log2(m.useCount + 1) * 2;

  // 时效得分（指数衰减）
  const recencyScore = Math.exp(-ageHours * Math.LN2 / HALF_LIFE_HOURS) * 5;

  // 关键词丰富度
  const kwScore = Math.min(m.keywords.length / 3, 2);

  // 内容长度适中（20-150字最优）
  const len = m.content.length;
  const lenScore = len < 10 ? 0.5 : len < 30 ? 1 : len < 150 ? 2 : len < 300 ? 1.5 : 1;

  // 分类权重
  const catWeights: Record<string, number> = {
    preference: 1.3,  // 用户偏好最重要
    personal: 1.2,    // 个人信息重要
    task: 1.1,        // 任务信息
    tech: 1.0,        // 技术信息
    knowledge: 0.9,   // 通用知识
  };

  const catWeight = catWeights[m.category] || 1.0;

  return (freqScore + recencyScore + kwScore + lenScore) * catWeight;
}

/**
 * 获取高优先级记忆（Top-K）
 */
export function getTopMemories(memories: Memory[], topK = 10): Memory[] {
  return [...memories]
    .map(m => ({ m, importance: scoreImportance(m) }))
    .sort((a, b) => b.importance - a.importance)
    .slice(0, topK)
    .map(({ m }) => m);
}

/* ───────── 记忆图谱（Memory Graph） ───────── */

export interface MemoryEdge {
  from: string;   // 记忆ID
  to: string;     // 记忆ID
  weight: number; // 关联强度
  sharedKeywords: string[];
}

/**
 * 构建记忆关联图谱
 * 基于共享关键词计算记忆之间的关联强度
 */
export function buildMemoryGraph(memories: Memory[]): {
  nodes: { id: string; label: string; importance: number }[];
  edges: MemoryEdge[];
} {
  const now = Date.now();
  const nodes = memories.map(m => ({
    id: m.id,
    label: m.content.slice(0, 40),
    importance: scoreImportance(m, now),
  }));

  const edges: MemoryEdge[] = [];
  const edgeMap = new Map<string, MemoryEdge>();

  for (let i = 0; i < memories.length; i++) {
    for (let j = i + 1; j < memories.length; j++) {
      const a = memories[i];
      const b = memories[j];
      
      // 计算共享关键词
      const shared = a.keywords.filter(kw => b.keywords.includes(kw));
      if (shared.length === 0) continue;

      // 计算内容词汇重叠
      const wordsA = new Set(a.content.toLowerCase().split(/\s+/).filter(w => w.length > 1));
      const wordsB = new Set(b.content.toLowerCase().split(/\s+/).filter(w => w.length > 1));
      const overlap = [...wordsA].filter(w => wordsB.has(w)).length;
      
      // 权重 = 共享关键词数 * 2 + 词汇重叠
      const weight = shared.length * 2 + overlap;
      
      const key = [a.id, b.id].sort().join('::');
      edgeMap.set(key, {
        from: a.id,
        to: b.id,
        weight,
        sharedKeywords: shared,
      });
    }
  }

  // 保留权重最高的边
  const sortedEdges = [...edgeMap.values()].sort((a, b) => b.weight - a.weight);
  // 限制边数：每个节点最多3条最强的边
  const nodeEdgeCount = new Map<string, number>();
  for (const edge of sortedEdges) {
    const fromCount = nodeEdgeCount.get(edge.from) || 0;
    const toCount = nodeEdgeCount.get(edge.to) || 0;
    if (fromCount < 3 && toCount < 3) {
      edges.push(edge);
      nodeEdgeCount.set(edge.from, fromCount + 1);
      nodeEdgeCount.set(edge.to, toCount + 1);
    }
  }

  return { nodes, edges };
}

/* ───────── 记忆合并（Memory Consolidation） ───────── */

export interface ConsolidationCandidate {
  memories: Memory[];
  mergedContent: string;
  mergedKeywords: string[];
  score: number; // 合并收益
}

/**
 * 检测可合并的记忆对
 * 当两条记忆内容高度重叠时，建议合并
 */
export function findConsolidationCandidates(memories: Memory[]): ConsolidationCandidate[] {
  const candidates: ConsolidationCandidate[] = [];

  for (let i = 0; i < memories.length; i++) {
    for (let j = i + 1; j < memories.length; j++) {
      const a = memories[i];
      const b = memories[j];

      // 计算内容相似度（Jaccard相似度）
      const wordsA = new Set(a.content.toLowerCase().split(/\s+/).filter(w => w.length > 1));
      const wordsB = new Set(b.content.toLowerCase().split(/\s+/).filter(w => w.length > 1));
      const intersection = [...wordsA].filter(w => wordsB.has(w)).length;
      const union = new Set([...wordsA, ...wordsB]).size;
      
      if (union === 0) continue;
      
      const jaccard = intersection / union;

      // 只有相似度超过阈值才考虑合并
      if (jaccard < 0.4) continue;

      // 合并后的内容（取较完整的那个）
      const mergedContent = a.content.length >= b.content.length ? a.content : b.content;
      
      // 合并关键词
      const mergedKeywords = [...new Set([...a.keywords, ...b.keywords])];

      // 合并收益 = 相似度 * (两条记忆的使用次数之和)
      const score = jaccard * (a.useCount + b.useCount);

      candidates.push({
        memories: [a, b],
        mergedContent,
        mergedKeywords,
        score,
      });
    }
  }

  return candidates.sort((a, b) => b.score - a.score).slice(0, 5);
}

/* ───────── HyDE 策略：假设答案增强检索 ───────── */

/**
 * 生成 HyDE 假设答案的关键词骨架
 * 
 * HyDE (Hypothetical Document Embeddings) 的轻量实现：
 * 不依赖LLM生成完整假设文档，而是基于查询构建"理想答案"的关键词轮廓
 * 然后用这个轮廓去匹配记忆
 * 
 * 原理：假设答案在语义上比问题本身更接近真实的文档/记忆
 */
export function hydeExpand(query: string, memoryKeywords: string[]): {
  hypotheticalKeywords: string[];
  explanation: string;
} {
  const queryTerms = query.toLowerCase().split(/\s+/).filter(w => w.length > 1);
  
  // 找到记忆中与查询相关的关键词
  const relatedKws = memoryKeywords.filter(kw => {
    const kwLower = kw.toLowerCase();
    return queryTerms.some(t => kwLower.includes(t) || t.includes(kwLower));
  });

  // 构建假设答案的关键词轮廓 = 查询词 + 相关记忆关键词
  const hypothetical = [...new Set([...queryTerms, ...relatedKws])].slice(0, 10);

  return {
    hypotheticalKeywords: hypothetical,
    explanation: relatedKws.length > 0
      ? `HyDE扩展: 查询词${queryTerms.length}个 + 关联记忆词${relatedKws.length}个 → 假设答案轮廓${hypothetical.length}个词`
      : `HyDE基础: 无匹配记忆关键词，使用查询词${hypothetical.length}个作为轮廓`,
  };
}

/**
 * 增强版记忆检索（集成HyDE策略）
 */
export function findRelevantMemoriesHyde(
  memories: Memory[],
  query: string,
  maxResults = 8,
  currentConvId?: string,
): { memories: Memory[]; hydeInfo: ReturnType<typeof hydeExpand> } {
  // 收集所有记忆关键词
  const allKeywords = [...new Set(memories.flatMap(m => m.keywords))];
  
  // HyDE 扩展
  const hydeInfo = hydeExpand(query, allKeywords);
  
  // 用扩展后的关键词进行搜索
  const enhancedQuery = hydeInfo.hypotheticalKeywords.join(' ');
  const results = findRelevantMemories(memories, enhancedQuery, maxResults, currentConvId);
  
  return { memories: results, hydeInfo };
}

/**
 * 每日记忆摘要生成
 * 按类别聚合，生成结构化摘要
 */
export function generateDailySummary(memories: Memory[]): {
  totalNew: number;
  byCategory: Record<string, number>;
  topKeywords: string[];
  summary: string;
} {
  const catLabels: Record<string, string> = {
    tech: '🔧技术', personal: '👤个人', preference: '⭐偏好', task: '📋任务', knowledge: '📚知识',
  };

  const byCategory: Record<string, number> = {};
  const allKws: string[] = [];

  for (const m of memories) {
    byCategory[m.category] = (byCategory[m.category] || 0) + 1;
    allKws.push(...m.keywords);
  }

  // 统计最高频关键词
  const kwFreq = new Map<string, number>();
  for (const kw of allKws) {
    kwFreq.set(kw, (kwFreq.get(kw) || 0) + 1);
  }
  const topKeywords = [...kwFreq.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 5)
    .map(([kw]) => kw);

  const catSummary = Object.entries(byCategory)
    .map(([cat, count]) => `${catLabels[cat] || cat} ${count}条`)
    .join(' · ');

  return {
    totalNew: memories.length,
    byCategory,
    topKeywords,
    summary: `今日新增 ${memories.length} 条记忆：${catSummary || '无'}`,
  };
}

/**
 * 记忆衰减检查：
 * 标记长期未使用且不重要的记忆，建议清理
 */
export function getStaleMemories(memories: Memory[], staleDays = 30): Memory[] {
  const now = Date.now();
  const threshold = staleDays * 24 * 60 * 60 * 1000;
  
  return memories.filter(m => {
    const age = now - m.updatedAt;
    const imp = scoreImportance(m, now);
    // 旧 + 低重要性 + 低使用次数 = stale
    return age > threshold && imp < 2 && m.useCount < 3;
  });
}
