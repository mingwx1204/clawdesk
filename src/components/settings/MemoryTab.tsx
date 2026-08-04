/**
 * 永久记忆管理 v2 — 重要性评分 + 记忆图谱 + 合并建议 + HyDE
 */
import { useEffect, useMemo, useState } from 'react';
import { Trash2, Search, RefreshCw, Clock, BarChart3, Calendar, GitMerge, AlertTriangle, Sparkles, Network } from 'lucide-react';
import { useMemoryStore } from '@/store/useMemoryStore';
import { scoreImportance } from '@/lib/memory';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import type { Memory } from '@/types';

function MemoryCard({ m, onDelete }: { m: Memory; onDelete: () => void }) {
  const catColors: Record<string, string> = {
    tech: 'bg-blue-500/10 text-blue-400 border-blue-500/30',
    personal: 'bg-purple-500/10 text-purple-400 border-purple-500/30',
    preference: 'bg-amber-500/10 text-amber-400 border-amber-500/30',
    task: 'bg-green-500/10 text-green-400 border-green-500/30',
    knowledge: 'bg-cyan-500/10 text-cyan-400 border-cyan-500/30',
  };

  const imp = scoreImportance(m);
  const impStars = imp > 6 ? '★★★★★' : imp > 4 ? '★★★★' : imp > 2.5 ? '★★★' : imp > 1.5 ? '★★' : '★';
  const impColor = imp > 4 ? 'text-yellow-400' : imp > 2.5 ? 'text-green-400' : 'text-muted-foreground';

  return (
    <div className={`rounded-lg border px-3 py-2.5 ${catColors[m.category] || catColors.knowledge}`}>
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5 mb-0.5">
            <span className={`text-[10px] ${impColor}`}>{impStars}</span>
            <span className="text-[10px] text-muted-foreground">{imp.toFixed(1)}</span>
          </div>
          <p className="text-sm leading-relaxed">{m.content}</p>
          <div className="mt-1.5 flex items-center gap-2 text-[10px] opacity-60">
            <Clock className="h-3 w-3" />
            <span>{new Date(m.createdAt).toLocaleString('zh-CN', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span>
            <span>·</span>
            <span>使用 {m.useCount} 次</span>
            {m.keywords.length > 0 && <span className="text-[9px] opacity-50">🏷 {m.keywords.slice(0, 3).join(', ')}</span>}
          </div>
        </div>
        <Button variant="ghost" size="icon" className="h-7 w-7 shrink-0 text-red-400" onClick={onDelete}>
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  );
}

function groupByDate(memories: Memory[]): Map<string, Memory[]> {
  const groups = new Map<string, Memory[]>();
  for (const m of memories) {
    const date = new Date(m.createdAt).toLocaleDateString('zh-CN');
    if (!groups.has(date)) groups.set(date, []);
    groups.get(date)!.push(m);
  }
  return groups;
}

function dailyDigest(memories: Memory[]): string {
  if (memories.length === 0) return '今日暂无新记忆';
  const cats: Record<string, number> = {};
  for (const m of memories) cats[m.category] = (cats[m.category] || 0) + 1;
  const labels: Record<string, string> = { tech: '技术', personal: '个人', preference: '偏好', task: '任务', knowledge: '知识' };
  return `新增 ${memories.length} 条：${Object.entries(cats).map(([k, v]) => `${labels[k] || k} ${v}`).join(' · ')}`;
}

export function MemoryTab() {
  const { memories, loaded, load, remove, getTopMemories, getGraph, getConsolidationCandidates, consolidate, getDailySummary, getStale, cleanStale } = useMemoryStore();
  const [keyword, setKeyword] = useState('');
  const [viewMode, setViewMode] = useState<'timeline' | 'categories' | 'top' | 'graph'>('timeline');
  const [showConsolidation, setShowConsolidation] = useState(false);
  const [showStale, setShowStale] = useState(false);

  useEffect(() => { if (!loaded) void load(); }, [loaded, load]);

  const filtered = keyword.trim()
    ? memories.filter((m) => m.content.toLowerCase().includes(keyword.toLowerCase()) || m.keywords.some((k) => k.includes(keyword)))
    : memories;

  const dateGroups = useMemo(() => groupByDate(filtered), [filtered]);
  const today = new Date().toLocaleDateString('zh-CN');
  const todayMs = memories.filter((m) => new Date(m.createdAt).toLocaleDateString('zh-CN') === today);
  const digest = useMemo(() => dailyDigest(todayMs), [todayMs]);
  const topMemories = useMemo(() => getTopMemories(8), [memories, getTopMemories]);
  const graph = useMemo(() => getGraph(), [memories, getGraph]);
  const consolidationCandidates = useMemo(() => getConsolidationCandidates(), [memories, getConsolidationCandidates]);
  const staleMemories = useMemo(() => getStale(30), [memories, getStale]);
  const dailySummary = useMemo(() => getDailySummary(), [memories, getDailySummary]);

  const cats = { tech: 0, personal: 0, preference: 0, task: 0, knowledge: 0 };
  for (const m of memories) cats[m.category] = (cats[m.category] || 0) + 1;
  const catIcons: Record<string, string> = { tech: '🔧', personal: '👤', preference: '⭐', task: '📋', knowledge: '📚' };
  const catLabels: Record<string, string> = { tech: '技术', personal: '个人', preference: '偏好', task: '任务', knowledge: '知识' };

  return (
    <div className="space-y-4">
      {/* 统计卡片 */}
      <div className="grid grid-cols-4 gap-2">
        <div className="rounded-lg border border-primary/30 bg-primary/5 p-2.5 text-center">
          <p className="text-xl font-bold text-primary">{memories.length}</p>
          <p className="text-[10px] text-muted-foreground">总记忆</p>
        </div>
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-2.5 text-center">
          <p className="text-xl font-bold text-amber-400">{todayMs.length}</p>
          <p className="text-[10px] text-muted-foreground">今日新增</p>
        </div>
        <div className="rounded-lg border border-green-500/30 bg-green-500/5 p-2.5 text-center">
          <p className="text-xl font-bold text-green-400">{memories.reduce((s, m) => s + m.useCount, 0)}</p>
          <p className="text-[10px] text-muted-foreground">总引用</p>
        </div>
        <div className="rounded-lg border border-yellow-500/30 bg-yellow-500/5 p-2.5 text-center">
          <p className="text-xl font-bold text-yellow-400">{graph.edges.length}</p>
          <p className="text-[10px] text-muted-foreground">关联边</p>
        </div>
      </div>

      {/* 今日摘要 */}
      {todayMs.length > 0 && (
        <div className="rounded-lg border border-border/50 bg-card p-3">
          <div className="flex items-center gap-2 mb-1">
            <Sparkles className="h-3.5 w-3.5 text-yellow-400" />
            <span className="text-xs font-medium">今日摘要</span>
          </div>
          <p className="text-xs text-muted-foreground">{digest}</p>
          {dailySummary.topKeywords.length > 0 && (
            <div className="mt-1.5 flex flex-wrap gap-1">
              {dailySummary.topKeywords.map(kw => (
                <span key={kw} className="rounded-full bg-primary/10 px-2 py-0.5 text-[10px] text-primary">#{kw}</span>
              ))}
            </div>
          )}
        </div>
      )}

      {/* 合并建议 */}
      {consolidationCandidates.length > 0 && (
        <div className="rounded-lg border border-amber-500/30 bg-amber-500/5 p-3">
          <div className="flex items-center justify-between mb-2">
            <div className="flex items-center gap-2">
              <GitMerge className="h-3.5 w-3.5 text-amber-400" />
              <span className="text-xs font-medium text-amber-400">记忆合并建议</span>
            </div>
            <Button variant="ghost" size="sm" className="h-6 text-[10px]" onClick={() => setShowConsolidation(!showConsolidation)}>
              {showConsolidation ? '收起' : `展开 (${consolidationCandidates.length})`}
            </Button>
          </div>
          {showConsolidation && (
            <div className="space-y-2 max-h-[200px] overflow-y-auto">
              {consolidationCandidates.map((c, i) => (
                <div key={i} className="rounded border border-border/40 p-2 text-[11px]">
                  <p className="text-muted-foreground mb-1">
                    合并 {c.memories.length} 条相似记忆（相似度 {(c.score / (c.memories[0]?.useCount + c.memories[1]?.useCount || 1)).toFixed(0)}%）
                  </p>
                  <div className="space-y-0.5 mb-1.5">
                    {c.memories.map(m => <p key={m.id} className="text-[10px] opacity-60 line-clamp-1">· {m.content.slice(0, 60)}</p>)}
                  </div>
                  <Button variant="outline" size="sm" className="h-6 text-[10px]" onClick={() => void consolidate(c)}>
                    <GitMerge className="mr-1 h-3 w-3" /> 合并为: {c.mergedContent.slice(0, 40)}...
                  </Button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* 过期记忆提醒 */}
      {staleMemories.length > 0 && (
        <div className="rounded-lg border border-red-500/30 bg-red-500/5 p-3">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <AlertTriangle className="h-3.5 w-3.5 text-red-400" />
              <span className="text-xs text-red-400">{staleMemories.length} 条记忆超过30天未使用</span>
            </div>
            <Button variant="ghost" size="sm" className="h-6 text-[10px] text-red-400" onClick={() => { void cleanStale(30).then(n => { if (n > 0) void load(); }); }}>
              一键清理
            </Button>
          </div>
        </div>
      )}

      {/* 搜索 + 视图切换 */}
      <div className="flex items-center gap-1.5 flex-wrap">
        <div className="relative flex-1 min-w-[120px]">
          <Search className="absolute left-2.5 top-2 h-3.5 w-3.5 text-muted-foreground" />
          <Input className="h-7 pl-8 text-sm" placeholder="搜索记忆..." value={keyword} onChange={(e) => setKeyword(e.target.value)} />
        </div>
        <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => void load()}><RefreshCw className="h-3.5 w-3.5" /></Button>
        <Button variant={viewMode === 'timeline' ? 'secondary' : 'ghost'} size="sm" className="h-7 text-[10px] px-2" onClick={() => setViewMode('timeline')}><Clock className="mr-1 h-3 w-3" />时间线</Button>
        <Button variant={viewMode === 'categories' ? 'secondary' : 'ghost'} size="sm" className="h-7 text-[10px] px-2" onClick={() => setViewMode('categories')}><BarChart3 className="mr-1 h-3 w-3" />分类</Button>
        <Button variant={viewMode === 'top' ? 'secondary' : 'ghost'} size="sm" className="h-7 text-[10px] px-2" onClick={() => setViewMode('top')}><Sparkles className="mr-1 h-3 w-3" />重要</Button>
        <Button variant={viewMode === 'graph' ? 'secondary' : 'ghost'} size="sm" className="h-7 text-[10px] px-2" onClick={() => setViewMode('graph')}><Network className="mr-1 h-3 w-3" />图谱</Button>
      </div>

      {/* 内容 */}
      <div className="max-h-[400px] overflow-y-auto">
        {viewMode === 'timeline' && (
          <div className="space-y-4">
            {Array.from(dateGroups.entries()).map(([date, items]) => (
              <div key={date}>
                <p className="mb-2 text-[11px] font-medium text-muted-foreground sticky top-0 bg-background py-1">{date}</p>
                <div className="space-y-2">{items.map((m) => <MemoryCard key={m.id} m={m} onDelete={() => void remove(m.id)} />)}</div>
              </div>
            ))}
            {filtered.length === 0 && <p className="py-8 text-center text-sm text-muted-foreground">{memories.length === 0 ? 'AI 会在对话中自动提取记忆 ✨' : '无匹配'}</p>}
          </div>
        )}

        {viewMode === 'categories' && (
          <div className="space-y-2">
            {Object.entries(cats).map(([cat, count]) => count > 0 && (
              <div key={cat} className="flex items-center gap-2 rounded-lg border border-border/40 p-3">
                <span className="text-lg">{catIcons[cat]}</span>
                <span className="flex-1 text-sm">{catLabels[cat]}</span>
                <span className="text-sm font-bold">{count}</span>
                <span className="text-[10px] text-muted-foreground">
                  {Math.round(count / Math.max(memories.length, 1) * 100)}%
                </span>
              </div>
            ))}
            {memories.length === 0 && <p className="py-8 text-center text-sm text-muted-foreground">暂无记忆</p>}
          </div>
        )}

        {viewMode === 'top' && (
          <div className="space-y-2">
            <p className="text-[11px] text-muted-foreground mb-1">📊 按重要性排序（频率×时效×关键词×分类权重）</p>
            {topMemories.map((m) => <MemoryCard key={m.id} m={m} onDelete={() => void remove(m.id)} />)}
            {topMemories.length === 0 && <p className="py-8 text-center text-sm text-muted-foreground">暂无高优先级记忆</p>}
          </div>
        )}

        {viewMode === 'graph' && (
          <div className="space-y-2">
            <p className="text-[11px] text-muted-foreground mb-1">🔗 记忆关联图谱（{graph.nodes.length}节点 · {graph.edges.length}条边）</p>
            {graph.edges.length === 0 ? (
              <p className="py-8 text-center text-sm text-muted-foreground">记忆之间暂未形成强关联，继续对话会自动建立</p>
            ) : (
              <div className="space-y-1.5">
                {graph.edges.slice(0, 15).map((edge, i) => {
                  const fromNode = graph.nodes.find(n => n.id === edge.from);
                  const toNode = graph.nodes.find(n => n.id === edge.to);
                  return (
                    <div key={i} className="rounded border border-border/40 p-2 text-[10px]">
                      <div className="flex items-center gap-1.5">
                        <span className="text-primary truncate max-w-[120px]">{fromNode?.label || edge.from}</span>
                        <span className="text-muted-foreground">←{edge.weight.toFixed(0)}→</span>
                        <span className="text-amber-400 truncate max-w-[120px]">{toNode?.label || edge.to}</span>
                      </div>
                      {edge.sharedKeywords.length > 0 && (
                        <p className="mt-0.5 text-[9px] text-muted-foreground">
                          共享: {edge.sharedKeywords.slice(0, 4).join(', ')}
                        </p>
                      )}
                    </div>
                  );
                })}
                {graph.edges.length > 15 && (
                  <p className="text-[10px] text-muted-foreground text-center py-1">... 还有 {graph.edges.length - 15} 条关联</p>
                )}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
