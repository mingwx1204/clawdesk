/**
 * Agentic RAG — 基于Loop的智能检索
 *
 * 核心理念（来自笔记）：
 * - 传统RAG是Pipeline（单向流水线），方向错了就锁死
 * - Agentic RAG是Loop（循环探索）：查询→检索→判断→不满足则重新查询
 *
 * 关键机制：
 * 1. 问题→陈述句转换（让查询更接近文档语义）
 * 2. 多轮检索 + 相关性判断
 * 3. Adaptive Routine：简单问题走Pipeline，复杂问题升级Loop
 * 4. 元数据感知（利用文件结构、时间戳等信号）
 */

import { searchMemoryRag, searchLocalDocs, getRagStats } from './rag';
import type { FileNode } from '@/types';

/* ───────── 类型定义 ───────── */

export interface AgenticSearchStep {
  step: number;
  action: string;
  query: string;
  result: string;
  relevance: 'high' | 'medium' | 'low' | 'none';
  reasoning: string;
  timestamp: number;
}

export interface AgenticSearchResult {
  answer: string;
  steps: AgenticSearchStep[];
  totalSteps: number;
  finalRelevance: 'high' | 'medium' | 'low' | 'none';
  usedAdaptiveRoutine: boolean;
  timeMs: number;
}

/* ───────── 问题→陈述句转换 ───────── */

/**
 * 将用户问题转换为陈述句形式
 * 这是Agentic RAG的关键第一步：陈述句在语义上更接近文档原文
 *
 * 例如：
 *   "RAG和Agentic Search有什么区别？" →
 *   "RAG和Agentic Search的区别"
 *   "如何搭建知识库？" →
 *   "搭建知识库的方法步骤"
 */
function questionToStatement(question: string): string {
  // 移除疑问词
  let stmt = question
    .replace(/^(什么|怎么|如何|怎样|为什么|哪[些个]|谁|何时|多久)/, '')
    .replace(/[？?！!]+$/, '')
    .replace(/^(是|有|能|会|可以|应该|需要)/, '')
    .trim();

  // 如果移除后太短，保留原文
  if (stmt.length < 4) stmt = question.replace(/[？?]+$/, '');

  // 添加陈述标记
  const suffixes = ['的方法', '的步骤', '的区别', '的原理', '的概念', '的定义', '的实践', '的方案'];
  const hasSuffix = suffixes.some(s => stmt.includes(s));
  if (!hasSuffix && stmt.length < 20) {
    // 尝试推断合适后缀
    if (/什么|哪/.test(question)) stmt = stmt + '的相关信息';
    else if (/怎么|如何/.test(question)) stmt = stmt + '的方法';
    else if (/为什么/.test(question)) stmt = stmt + '的原因';
  }

  return stmt;
}

/* ───────── 相关性判断 ───────── */

/**
 * 判断检索结果是否与问题相关（启发式）
 * 在实际Agent中，这一步应由LLM判断，这里做轻量规则版本
 */
function judgeRelevance(
  originalQuery: string,
  searchResults: string,
): { relevant: boolean; score: number; reason: string } {
  const queryTerms = originalQuery.toLowerCase().split(/\s+/).filter(w => w.length > 1);
  const resultsLower = searchResults.toLowerCase();

  // 1. 检查是否有内容匹配
  if (!searchResults.includes('📖') && !searchResults.includes('高相关') && !searchResults.includes('中相关')) {
    return { relevant: false, score: 0, reason: '未找到内容匹配' };
  }

  // 2. 检查高相关命中数
  const highHits = (searchResults.match(/🔴 高相关/g) || []).length;
  const mediumHits = (searchResults.match(/🟡 中相关/g) || []).length;
  const totalHits = (searchResults.match(/📄/g) || []).length;

  // 3. 查询词在结果中的覆盖率
  let termHits = 0;
  for (const term of queryTerms) {
    if (resultsLower.includes(term)) termHits++;
  }
  const coverage = termHits / queryTerms.length;

  // 4. 综合判断
  const score = highHits * 3 + mediumHits * 1.5 + totalHits * 0.5 + coverage * 5;

  if (score >= 8) {
    return { relevant: true, score, reason: `高置信度：${highHits}条高相关 + ${mediumHits}条中相关，查询覆盖率${Math.round(coverage*100)}%` };
  } else if (score >= 3) {
    return { relevant: true, score, reason: `中等置信度：${totalHits}条匹配，查询覆盖率${Math.round(coverage*100)}%` };
  } else if (score > 0) {
    return { relevant: false, score, reason: `低置信度：仅${totalHits}条弱匹配，可能需要调整查询方向` };
  }
  return { relevant: false, score: 0, reason: '无有效匹配' };
}

/* ───────── Adaptive Routine ───────── */

/**
 * 判断问题复杂度，决定使用Pipeline还是Loop
 * 简单问题：单个概念、直接查询 → Pipeline
 * 复杂问题：比较/分析/多步骤 → Agentic Loop
 */
function assessComplexity(query: string): { complex: boolean; reason: string } {
  const lower = query.toLowerCase();
  
  const complexPatterns = [
    /比较|区别|对比|vs/i,
    /分析|评估|判断/i,
    /为什么|原因|原理/i,
    /如何实现|怎么搭建|怎么构建/i,
    /关系|联系|影响/i,
    /最优|最佳|推荐/i,
    /多个|哪些|各种/i,
  ];

  for (const pattern of complexPatterns) {
    if (pattern.test(query)) {
      return { complex: true, reason: `检测到复杂查询模式: ${pattern.source}` };
    }
  }

  // 查询词多通常意味着复杂
  const terms = lower.split(/\s+/).filter(w => w.length > 1);
  if (terms.length > 5) {
    return { complex: true, reason: `查询词较多(${terms.length}个)` };
  }

  return { complex: false, reason: '简单查询' };
}

/**
 * 生成替代查询方向（当一轮检索不满足时）
 * 用于Agentic Loop的探索机制
 */
function generateAlternativeQueries(originalQuery: string, previousResults: string): string[] {
  const terms = originalQuery.toLowerCase().split(/\s+/).filter(w => w.length > 1);
  const alternatives: string[] = [];

  // 策略1：去掉修饰词，保留核心词
  if (terms.length > 3) {
    const core = terms.filter(t => t.length > 2).slice(0, 3);
    alternatives.push(core.join(' '));
  }

  // 策略2：添加同义扩展
  const expandMap: Record<string, string[]> = {
    'rag': ['检索增强生成', '知识库', '向量检索'],
    'agent': ['智能体', 'agentic', '自主'],
    'agi': ['通用人工智能', '强人工智能'],
    '搜索': ['检索', '查找', '查询'],
    '记忆': ['memory', '存储', '记录'],
    '模型': ['大模型', 'llm', 'gpt'],
    '工具': ['tool', 'function', 'api'],
  };

  for (const term of terms) {
    const expansions = expandMap[term];
    if (expansions) {
      for (const exp of expansions) {
        const alt = originalQuery.toLowerCase().replace(term, exp);
        if (alt !== originalQuery.toLowerCase()) {
          alternatives.push(alt);
        }
      }
    }
  }

  // 策略3：更泛化的查询
  if (terms.length >= 2) {
    alternatives.push(terms.slice(0, Math.ceil(terms.length / 2)).join(' '));
  }

  return [...new Set(alternatives)].slice(0, 3);
}

/* ───────── 主搜索循环 ───────── */

/**
 * Agentic RAG 主入口：
 * 1. Adaptive Routine 判断复杂度
 * 2. 简单→单次Pipeline检索
 * 3. 复杂→多轮Loop探索
 */
export async function agenticSearch(
  query: string,
  fileNodes: FileNode[],
  workdir: string,
  maxRounds = 3,
): Promise<AgenticSearchResult> {
  const startTime = performance.now();
  const steps: AgenticSearchStep[] = [];

  // Step 0: 复杂度判断（Adaptive Routine）
  const { complex, reason: complexityReason } = assessComplexity(query);

  if (!complex) {
    // ── 简单Pipeline路径 ──
    const statement = questionToStatement(query);
    const result = await searchLocalDocs(fileNodes, statement, workdir);
    
    const { relevant, score, reason } = judgeRelevance(query, result);
    
    steps.push({
      step: 1,
      action: `🎯 Adaptive Routine: 简单查询 → 直接Pipeline (${complexityReason})`,
      query: statement,
      result,
      relevance: relevant ? (score >= 8 ? 'high' : 'medium') : 'low',
      reasoning: `问题→陈述句转换: "${query}" → "${statement}"\n${reason}`,
      timestamp: Date.now(),
    });

    return {
      answer: result,
      steps,
      totalSteps: 1,
      finalRelevance: relevant ? (score >= 8 ? 'high' : 'medium') : 'low',
      usedAdaptiveRoutine: true,
      timeMs: Math.round(performance.now() - startTime),
    };
  }

  // ── 复杂查询：Agentic Loop ──
  let currentQuery = questionToStatement(query);
  let allResults = '';
  let bestRelevance: AgenticSearchResult['finalRelevance'] = 'none';

  for (let round = 1; round <= maxRounds; round++) {
    // 执行检索
    const result = await searchLocalDocs(fileNodes, currentQuery, workdir);
    allResults += (allResults ? '\n---\n' : '') + result;

    // 判断相关性
    const { relevant, score, reason } = judgeRelevance(query, result);

    const relevanceLabel = score >= 8 ? 'high' : score >= 3 ? 'medium' : score > 0 ? 'low' : 'none';
    if (relevanceLabel === 'high') bestRelevance = 'high';
    else if (relevanceLabel === 'medium' && bestRelevance !== 'high') bestRelevance = 'medium';
    else if (relevanceLabel === 'low' && bestRelevance === 'none') bestRelevance = 'low';

    steps.push({
      step: round,
      action: round === 1
        ? `🔄 Agentic Loop 第${round}轮: 初始查询 → 检索 → 评估`
        : `🔄 Agentic Loop 第${round}轮: 调整方向 → 重新检索 → 评估`,
      query: currentQuery,
      result,
      relevance: relevanceLabel,
      reasoning: reason,
      timestamp: Date.now(),
    });

    // 满足条件则退出循环
    if (relevant && score >= 5) {
      steps.push({
        step: round + 1,
        action: '✅ 检索完成：信息充分，停止探索',
        query: '',
        result: '',
        relevance: relevanceLabel,
        reasoning: `经过${round}轮检索获得充分信息，得分${score.toFixed(1)}`,
        timestamp: Date.now(),
      });
      break;
    }

    // 最后一轮，即使不满足也退出
    if (round === maxRounds) {
      steps.push({
        step: round + 1,
        action: '⚠️ 达到最大探索轮数，返回已有结果',
        query: '',
        result: '',
        relevance: bestRelevance,
        reasoning: `已探索${maxRounds}轮，返回最佳匹配`,
        timestamp: Date.now(),
      });
      break;
    }

    // 生成替代查询方向
    const alternatives = generateAlternativeQueries(query, result);
    if (alternatives.length > 0) {
      currentQuery = alternatives[0];
    } else {
      // 无法生成替代查询，退出
      break;
    }
  }

  return {
    answer: allResults || '未找到相关信息',
    steps,
    totalSteps: steps.length,
    finalRelevance: bestRelevance,
    usedAdaptiveRoutine: true,
    timeMs: Math.round(performance.now() - startTime),
  };
}

/**
 * 仅对记忆库执行Agentic检索（用于HyDE策略）
 */
export async function agenticMemorySearch(
  memories: { content: string; keywords: string[] }[],
  query: string,
  maxRounds = 2,
): Promise<{ results: { content: string; score: number }[]; steps: string[] }> {
  const steps: string[] = [];
  const statement = questionToStatement(query);
  
  steps.push(`📝 问题→陈述句: "${query}" → "${statement}"`);
  
  // 第一轮：直接搜索
  let results = searchMemoryRag(memories, statement, 5);
  steps.push(`🔍 第1轮检索: ${results.length}条匹配`);
  
  // 如果结果不够，尝试扩展查询
  if (results.length < 3 && memories.length > 5) {
    const broaderQuery = statement.split(/\s+/).slice(0, 3).join(' ');
    const expanded = searchMemoryRag(memories, broaderQuery, 5);
    if (expanded.length > results.length) {
      steps.push(`🔍 第2轮扩展检索: "${broaderQuery}" → ${expanded.length}条`);
      results = expanded;
    }
  }

  return { results: results.map(r => ({ content: r.content, score: r.score })), steps };
}

/* ───────── 导出统计接口 ───────── */

export { getRagStats };
