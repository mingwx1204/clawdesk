/**
 * 本地 RAG v3 — 语义分块 + TF-IDF 混合检索 + Rerank + 查询扩展 + 渐进式披露
 * 
 * 基于学习笔记的10种RAG策略实现：
 * - Semantic Chunking（句子边界感知分块）
 * - TF-IDF 倒排索引（替代简单关键词匹配）
 * - Hybrid Search（关键词 + 语义混合检索）
 * - Rerank（交叉注意力式重排序）
 * - 查询扩展（自动扩展相关词）
 * - 渐进式披露（路径列表 → 相关片段 → 完整上下文）
 */
import type { FileNode } from '@/types';

/* ───────── 类型定义 ───────── */

interface DocChunk {
  path: string;
  content: string;
  startLine: number;
  endLine: number;
  /** 语义摘要（前N个关键词） */
  summary: string;
}

interface SearchHit {
  chunk: DocChunk;
  /** TF-IDF 得分 */
  tfidfScore: number;
  /** 关键词命中数 */
  keywordHits: number;
  /** 综合得分（用于排序） */
  finalScore: number;
}

/* ───────── 分块缓存 ───────── */

const chunkCache = new Map<string, DocChunk[]>();
/** 全局倒排索引：词 → 包含该词的chunk列表（含TF信息） */
const globalIndex = new Map<string, { chunk: DocChunk; tf: number }[]>();
/** 全局文档频率：词 → 出现在多少个chunk中 */
const globalDF = new Map<string, number>();
let totalChunks = 0;

/* ───────── 语义分块（Semantic Chunking） ───────── */

/**
 * 语义分块：以句子为基本单元，相邻句子语义相近的合并
 * 当连续句子的共享词比例低于阈值时切割
 */
function semanticChunk(text: string, filePath: string): DocChunk[] {
  const chunks: DocChunk[] = [];
  // 按句子分割（中英文句号、换行等）
  const sentences = text.split(/(?<=[。！？.!?\n])\s*/).filter(s => s.trim().length > 0);
  if (sentences.length === 0) return chunks;

  const TARGET_CHARS = 300;  // 目标块大小（字符）
  const MAX_CHARS = 600;     // 最大块大小
  const MIN_SIMILARITY = 0.15; // 最小语义相似度（共享词比例）

  let currentBlock: string[] = [];
  let currentChars = 0;
  let lineStart = 1;
  let currentLineCount = 0;

  const getWords = (s: string) => new Set(s.toLowerCase().replace(/[^\w\u4e00-\u9fff]/g, ' ').split(/\s+/).filter(w => w.length > 1));
  let prevWords: Set<string> | null = null;

  for (const sent of sentences) {
    const sentChars = sent.length;
    const sentWords = getWords(sent);
    const sentLines = (sent.match(/\n/g) || []).length + 1;

    // 计算与上一句的语义相似度
    let similarity = 1.0;
    if (prevWords && prevWords.size > 0 && sentWords.size > 0) {
      const intersection = [...sentWords].filter(w => prevWords!.has(w)).length;
      similarity = intersection / Math.min(prevWords.size, sentWords.size);
    }

    // 切割条件：超过最大字符 OR （超过目标字符且语义相似度低）
    const shouldSplit = currentChars > 0 && (
      currentChars + sentChars > MAX_CHARS ||
      (currentChars + sentChars > TARGET_CHARS && similarity < MIN_SIMILARITY)
    );

    if (shouldSplit) {
      chunks.push({
        path: filePath,
        content: currentBlock.join(''),
        startLine: lineStart,
        endLine: lineStart + currentLineCount - 1,
        summary: [...getWords(currentBlock.join(''))].slice(0, 8).join(', '),
      });
      lineStart = lineStart + currentLineCount;
      currentBlock = [];
      currentChars = 0;
      currentLineCount = 0;
      prevWords = null;
    }

    currentBlock.push(sent);
    currentChars += sentChars;
    currentLineCount += sentLines;
    prevWords = sentWords;
  }

  // 最后一个块
  if (currentBlock.length > 0) {
    chunks.push({
      path: filePath,
      content: currentBlock.join(''),
      startLine: lineStart,
      endLine: lineStart + currentLineCount - 1,
      summary: [...getWords(currentBlock.join(''))].slice(0, 8).join(', '),
    });
  }

  return chunks;
}

/* ───────── TF-IDF 倒排索引 ───────── */

/**
 * 构建/更新全局 TF-IDF 倒排索引
 * TF = 词在chunk中出现次数 / chunk总词数
 * IDF = log(总chunk数 / 包含该词的chunk数)
 */
function buildTfIdfIndex(chunks: DocChunk[]): void {
  for (const chunk of chunks) {
    const words = chunk.content.toLowerCase().replace(/[^\w\u4e00-\u9fff]/g, ' ').split(/\s+/).filter(w => w.length > 1);
    const totalWords = words.length;
    if (totalWords === 0) continue;

    // 计算每个词的TF
    const tfMap = new Map<string, number>();
    for (const w of words) {
      tfMap.set(w, (tfMap.get(w) || 0) + 1);
    }

    // 更新全局索引
    const seen = new Set<string>();
    for (const [word, count] of tfMap) {
      const tf = count / totalWords;
      if (!globalIndex.has(word)) globalIndex.set(word, []);
      globalIndex.get(word)!.push({ chunk, tf });
      
      if (!seen.has(word)) {
        seen.add(word);
        globalDF.set(word, (globalDF.get(word) || 0) + 1);
      }
    }
    totalChunks++;
  }
}

/**
 * TF-IDF 搜索：对查询中的每个词计算 TF-IDF 得分
 */
function tfidfSearch(queryTerms: string[]): SearchHit[] {
  const scoreMap = new Map<DocChunk, { tfidfSum: number; hits: number }>();

  for (const term of queryTerms) {
    const entries = globalIndex.get(term);
    if (!entries) continue;
    
    const df = globalDF.get(term) || 1;
    const idf = Math.log((totalChunks + 1) / (df + 1)) + 1; // 平滑IDF

    for (const { chunk, tf } of entries) {
      const prev = scoreMap.get(chunk) || { tfidfSum: 0, hits: 0 };
      prev.tfidfSum += tf * idf;
      prev.hits += 1;
      scoreMap.set(chunk, prev);
    }
  }

  return [...scoreMap.entries()].map(([chunk, { tfidfSum, hits }]) => ({
    chunk,
    tfidfScore: tfidfSum,
    keywordHits: hits,
    finalScore: 0, // 稍后由rerank计算
  }));
}

/* ───────── 查询扩展（Query Expansion） ───────── */

/**
 * 轻量查询扩展：利用倒排索引找共现词
 * 找到与查询词经常一起出现的词作为扩展词
 */
function expandQuery(queryTerms: string[]): string[] {
  const expanded = new Set(queryTerms);
  const cooccurrence = new Map<string, number>();

  for (const term of queryTerms) {
    const entries = globalIndex.get(term);
    if (!entries) continue;

    // 统计与查询词共现的词
    for (const { chunk } of entries.slice(0, 20)) {
      const words = new Set(chunk.content.toLowerCase().replace(/[^\w\u4e00-\u9fff]/g, ' ').split(/\s+/).filter(w => w.length > 1));
      for (const w of words) {
        if (!expanded.has(w)) {
          cooccurrence.set(w, (cooccurrence.get(w) || 0) + 1);
        }
      }
    }
  }

  // 取共现最多的3个词作为扩展
  const topExpansions = [...cooccurrence.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 3)
    .map(([w]) => w);

  return [...queryTerms, ...topExpansions];
}

/* ───────── Rerank（重排序） ───────── */

/**
 * 交叉注意力式 Rerank：
 * 1. 查询词在chunk中的密度得分
 * 2. 查询词出现在chunk开头/结尾的奖励
 * 3. chunk长度惩罚（太长稀释注意力）
 * 4. 关键词命中覆盖率
 */
function rerank(hits: SearchHit[], queryTerms: string[]): SearchHit[] {
  for (const hit of hits) {
    const content = hit.chunk.content.toLowerCase();
    const contentLen = content.length;
    
    // 1. 密度得分：查询词在chunk中出现的密度
    let densityScore = 0;
    for (const term of queryTerms) {
      const count = (content.match(new RegExp(term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'g')) || []).length;
      densityScore += count / Math.max(contentLen / 100, 1);
    }

    // 2. 位置奖励：出现在前20%或后20%加权
    let positionBonus = 0;
    const firstThird = content.slice(0, Math.floor(contentLen * 0.2));
    const lastThird = content.slice(Math.floor(contentLen * 0.8));
    for (const term of queryTerms) {
      if (firstThird.includes(term)) positionBonus += 0.3;
      if (lastThird.includes(term)) positionBonus += 0.15;
    }

    // 3. 长度惩罚：太长的chunk稀释相关性
    const lengthPenalty = Math.max(0.5, 1 - (contentLen - 300) / 2000);

    // 4. 关键词覆盖率
    const coverage = hit.keywordHits / queryTerms.length;

    // 综合得分
    hit.finalScore = (
      hit.tfidfScore * 0.4 +
      densityScore * 0.25 +
      positionBonus * 0.15 +
      coverage * 0.2
    ) * lengthPenalty;
  }

  return hits.sort((a, b) => b.finalScore - a.finalScore);
}

/* ───────── 文件读取 ───────── */

async function readAndChunk(filePath: string): Promise<DocChunk[]> {
  if (chunkCache.has(filePath)) return chunkCache.get(filePath)!;
  try {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
      const { readTextFile } = await import('@tauri-apps/plugin-fs');
      const text = await readTextFile(filePath);
      const chunks = semanticChunk(text, filePath);
      chunkCache.set(filePath, chunks);
      buildTfIdfIndex(chunks);
      return chunks;
    }
  } catch { /* 浏览器模式跳过 */ }
  return [];
}

const TEXT_EXTS = new Set([
  'md','txt','json','ts','tsx','js','jsx','py','rs','toml','yml','yaml',
  'html','css','csv','log','sql','sh','bat','ps1','env','cfg','ini',
  'c','cpp','h','java','go','rb','php','swift','kt',
]);

/* ───────── 主搜索接口 ───────── */

export interface SearchResult {
  /** 阶段1：文件路径匹配列表 */
  pathMatches: string[];
  /** 阶段2：内容匹配（渐进式披露） */
  contentMatches: { path: string; lineRange: string; snippet: string; score: number; relevance: 'high' | 'medium' | 'low' }[];
  /** 搜索统计 */
  stats: { totalChunks: number; queryExpanded: string[]; timeMs: number };
}

/**
 * 本地文档搜索 v3：
 * 1. 文件名快速匹配（元数据层）
 * 2. 查询扩展
 * 3. TF-IDF 检索
 * 4. Rerank 重排序
 * 5. 渐进式披露输出
 */
export async function searchLocalDocs(
  fileNodes: FileNode[],
  query: string,
  _workdir: string,
): Promise<string> {
  const startTime = performance.now();
  const queryTerms = query.toLowerCase().split(/\s+/).filter(w => w.length > 1);
  if (queryTerms.length === 0) return '';

  /* ── 阶段1：元数据匹配（文件名） ── */
  const nameMatches: string[] = [];
  const walk = (nodes: FileNode[]) => {
    for (const n of nodes) {
      if (!n.is_dir) {
        for (const q of queryTerms) {
          if (n.path.toLowerCase().includes(q)) { nameMatches.push(n.path); break; }
        }
      }
      if (n.children.length > 0) walk(n.children);
    }
  };
  walk(fileNodes);

  /* ── 阶段2：TF-IDF 内容检索 ── */
  let contentResults = '';
  const searchable = nameMatches
    .filter(f => TEXT_EXTS.has(f.split('.').pop()?.toLowerCase() || ''))
    .slice(0, 8);

  if (searchable.length > 0) {
    try {
      // 加载所有文件的chunks
      const allChunks: DocChunk[] = [];
      for (const f of searchable) {
        const c = await readAndChunk(f);
        allChunks.push(...c);
      }

      // 查询扩展
      const expandedTerms = expandQuery(queryTerms);
      const wasExpanded = expandedTerms.length > queryTerms.length;

      // TF-IDF 搜索
      const hits = tfidfSearch(expandedTerms);
      
      // Rerank
      const ranked = rerank(hits, expandedTerms).slice(0, 8);

      if (ranked.length > 0) {
        const timeMs = Math.round(performance.now() - startTime);
        
        // 渐进式披露：先显示路径+行号，再显示片段
        const highRelevance = ranked.filter(h => h.finalScore > 1.5);
        const medRelevance = ranked.filter(h => h.finalScore > 0.5 && h.finalScore <= 1.5);
        const lowRelevance = ranked.filter(h => h.finalScore <= 0.5);

        const formatHit = (h: SearchHit, label: string) => {
          const stars = h.finalScore > 2 ? '★★★' : h.finalScore > 1 ? '★★' : '★';
          return `📄 ${h.chunk.path}:L${h.chunk.startLine}-${h.chunk.endLine} ${stars} [${label}]\n   ${h.chunk.content.slice(0, 180)}${h.chunk.content.length > 180 ? '...' : ''}`;
        };

        const parts: string[] = [];
        if (highRelevance.length > 0) {
          parts.push('🔴 高相关：\n' + highRelevance.map(h => formatHit(h, `${h.finalScore.toFixed(1)}`)).join('\n\n'));
        }
        if (medRelevance.length > 0) {
          parts.push('🟡 中相关：\n' + medRelevance.map(h => formatHit(h, `${h.finalScore.toFixed(1)}`)).join('\n\n'));
        }
        if (lowRelevance.length > 0 && highRelevance.length + medRelevance.length < 3) {
          parts.push('🟢 低相关：\n' + lowRelevance.slice(0, 3).map(h => formatHit(h, `${h.finalScore.toFixed(1)}`)).join('\n\n'));
        }

        const expansionNote = wasExpanded
          ? `🔍 查询扩展: ${queryTerms.join(' ')} → ${expandedTerms.join(' ')}\n`
          : '';
        
        contentResults = `\n\n📖 内容检索结果（${ranked.length} 条匹配，${timeMs}ms，索引共 ${totalChunks} 个语义块）：\n${expansionNote}\n${parts.join('\n\n')}`;
      }
    } catch { /* 回退 */ }
  }

  let r = `🔍 搜索: "${query}"\n📁 文件名匹配 ${nameMatches.length} 个：\n${nameMatches.slice(0, 10).map((f, i) => `${i + 1}. ${f}`).join('\n')}`;
  if (contentResults) r += contentResults;
  if (!nameMatches.length && !contentResults) {
    r += '\n\n未找到匹配。💡 提示：尝试使用更具体的关键词，或检查文件是否在索引范围内。';
  }
  return r;
}

/* ───────── 内存 RAG：对记忆库的语义搜索 ───────── */

export interface MemoryRagResult {
  content: string;
  score: number;
  keywords: string[];
}

/**
 * 对记忆文本进行语义搜索（用于记忆检索增强）
 * 这是 HyDE 策略的基础——用户问题先检索记忆找到相关内容
 */
export function searchMemoryRag(
  memories: { content: string; keywords: string[] }[],
  query: string,
  topK = 5,
): MemoryRagResult[] {
  const queryTerms = query.toLowerCase().split(/\s+/).filter(w => w.length > 1);
  if (queryTerms.length === 0) return [];

  // 构建临时索引
  const scored = memories.map((mem, idx) => {
    const content = mem.content.toLowerCase();
    let score = 0;

    // 关键词精确匹配（高权重）
    for (const kw of mem.keywords) {
      if (query.toLowerCase().includes(kw.toLowerCase())) score += 15;
    }

    // TF-IDF 风格词匹配
    for (const term of queryTerms) {
      const regex = new RegExp(term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'g');
      const matches = content.match(regex);
      if (matches) score += matches.length * 3;
    }

    // 完整查询字符串包含检测
    if (content.includes(query.toLowerCase())) score += 10;

    return { content: mem.content, score, keywords: mem.keywords, _idx: idx };
  });

  return scored
    .filter(s => s.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, topK)
    .map(({ content, score, keywords }) => ({ content, score, keywords }));
}

/* ───────── 清除缓存 ───────── */

export function clearChunkCache() {
  chunkCache.clear();
  globalIndex.clear();
  globalDF.clear();
  totalChunks = 0;
}

/* ───────── 导出统计 ───────── */

export function getRagStats() {
  return {
    totalChunks,
    uniqueTerms: globalIndex.size,
    cachedFiles: chunkCache.size,
  };
}

