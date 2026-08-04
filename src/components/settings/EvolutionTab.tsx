/**
 * 自我进化管理面板 — 查看经验、技能、进化日志
 */
import { useEffect, useState } from 'react';
import { useEvolutionStore } from '@/store/useEvolutionStore';
import type { Experience, Skill, EvolutionEvent } from '@/types';
import { Trash2, Brain, Zap, RefreshCw, TrendingUp } from 'lucide-react';

export function EvolutionTab() {
  const store = useEvolutionStore();
  const [tab, setTab] = useState<'experiences' | 'skills' | 'log'>('experiences');

  useEffect(() => {
    store.load();
  }, []);

  return (
    <div className="flex flex-col gap-4 p-2">
      {/* 统计卡片 */}
      <div className="grid grid-cols-3 gap-3">
        <div className="rounded-xl border bg-card p-3 text-center">
          <div className="text-2xl font-bold text-primary">{store.experiences.length}</div>
          <div className="text-xs text-muted-foreground">经验</div>
        </div>
        <div className="rounded-xl border bg-card p-3 text-center">
          <div className="text-2xl font-bold text-green-500">{store.skills.length}</div>
          <div className="text-xs text-muted-foreground">技能</div>
        </div>
        <div className="rounded-xl border bg-card p-3 text-center">
          <div className="text-2xl font-bold text-amber-500">{store.events.length}</div>
          <div className="text-xs text-muted-foreground">进化事件</div>
        </div>
      </div>

      {/* Tab 切换 */}
      <div className="flex gap-1 rounded-lg bg-muted p-1">
        {(['experiences', 'skills', 'log'] as const).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`flex-1 rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
              tab === t ? 'bg-background shadow-sm' : 'text-muted-foreground hover:text-foreground'
            }`}
          >
            {t === 'experiences' && <><Brain className="mr-1 inline h-3.5 w-3.5" />经验</>}
            {t === 'skills' && <><Zap className="mr-1 inline h-3.5 w-3.5" />技能</>}
            {t === 'log' && <><TrendingUp className="mr-1 inline h-3.5 w-3.5" />进化日志</>}
          </button>
        ))}
      </div>

      {/* 内容区域 */}
      <div className="max-h-[400px] overflow-y-auto">
        {tab === 'experiences' && <ExperienceList experiences={store.experiences} onDelete={store.deleteExperience} />}
        {tab === 'skills' && <SkillList skills={store.skills} onDelete={store.deleteSkill} />}
        {tab === 'log' && <EventList events={store.events} />}
      </div>

      {/* 手动触发进化 */}
      <button
        onClick={async () => {
          const msgs = (async () => {
            try {
              const { useChatStore } = await import('@/store/useChatStore');
              return useChatStore.getState().messages;
            } catch { return []; }
          })();
          const result = await msgs;
          if (result.length > 0) store.runEvolve(result);
        }}
        disabled={store.evolving}
        className="flex items-center justify-center gap-2 rounded-lg border bg-card px-4 py-2 text-sm hover:bg-accent disabled:opacity-50"
      >
        <RefreshCw className={`h-4 w-4 ${store.evolving ? 'animate-spin' : ''}`} />
        {store.evolving ? '进化中...' : '手动触发进化'}
      </button>

      {store.lastEvolutionResult && (
        <div className="rounded-lg border border-primary/20 bg-primary/5 p-3 text-sm whitespace-pre-wrap">
          {store.lastEvolutionResult}
        </div>
      )}
    </div>
  );
}

// ─── 子组件 ───

function ExperienceList({ experiences, onDelete }: { experiences: Experience[]; onDelete: (id: string) => void }) {
  if (experiences.length === 0) {
    return <div className="py-8 text-center text-sm text-muted-foreground">暂无经验。完成几次对话后将自动提取。</div>;
  }

  const catLabel: Record<string, string> = {
    bug_fix: 'Bug 修复', code_pattern: '代码模式', workflow: '工作流',
    knowledge: '领域知识', user_pref: '用户偏好',
  };

  return (
    <div className="flex flex-col gap-2">
      {experiences.map((exp) => (
        <div key={exp.id} className="rounded-lg border bg-card p-3 text-sm">
          <div className="flex items-start justify-between gap-2">
            <div className="flex-1">
              <span className="inline-block rounded bg-primary/10 px-1.5 py-0.5 text-xs text-primary">
                {catLabel[exp.category] || exp.category}
              </span>
              <p className="mt-1">{exp.content}</p>
              {exp.codeSnippet && (
                <pre className="mt-1 overflow-x-auto rounded bg-muted p-1.5 text-xs">
                  {exp.codeSnippet.slice(0, 200)}
                </pre>
              )}
              <div className="mt-1 flex gap-3 text-xs text-muted-foreground">
                <span>使用 {exp.useCount} 次</span>
                <span>成功率 {(exp.successRate * 100).toFixed(0)}%</span>
              </div>
            </div>
            <button
              onClick={() => onDelete(exp.id)}
              className="shrink-0 rounded p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

function SkillList({ skills, onDelete }: { skills: Skill[]; onDelete: (id: string) => void }) {
  if (skills.length === 0) {
    return <div className="py-8 text-center text-sm text-muted-foreground">暂无技能。重复成功模式后将自动生成。</div>;
  }

  return (
    <div className="flex flex-col gap-2">
      {skills.map((skill) => (
        <div key={skill.id} className="rounded-lg border bg-card p-3 text-sm">
          <div className="flex items-start justify-between gap-2">
            <div className="flex-1">
              <div className="flex items-center gap-2">
                <span className="font-medium">{skill.name}</span>
                <span className="rounded bg-muted px-1.5 py-0.5 text-xs">{skill.type}</span>
                <span className="rounded bg-muted px-1.5 py-0.5 text-xs">v{skill.version}</span>
                {skill.autoActivate && (
                  <span className="rounded bg-green-500/10 px-1.5 py-0.5 text-xs text-green-600">自动</span>
                )}
              </div>
              <p className="mt-1 text-muted-foreground">{skill.description}</p>
              <div className="mt-1 flex gap-3 text-xs text-muted-foreground">
                <span>使用 {skill.useCount} 次</span>
                <span>成功率 {(skill.successRate * 100).toFixed(0)}%</span>
              </div>
            </div>
            <button
              onClick={() => onDelete(skill.id)}
              className="shrink-0 rounded p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

function EventList({ events }: { events: EvolutionEvent[] }) {
  if (events.length === 0) {
    return <div className="py-8 text-center text-sm text-muted-foreground">暂无进化事件。</div>;
  }

  const typeIcon: Record<string, string> = {
    experience_created: '🧠', skill_generated: '🔧', prompt_optimized: '⚡',
    reflection_completed: '📊', error_learned: '⚠️',
  };

  return (
    <div className="flex flex-col gap-1">
      {events.map((ev) => (
        <div key={ev.id} className="flex items-start gap-2 rounded px-2 py-1.5 text-sm hover:bg-muted/50">
          <span className="text-base">{typeIcon[ev.type] || '📌'}</span>
          <div className="flex-1">
            <p>{ev.summary}</p>
            <span className="text-xs text-muted-foreground">
              {new Date(ev.timestamp).toLocaleString('zh-CN')}
            </span>
          </div>
        </div>
      ))}
    </div>
  );
}
