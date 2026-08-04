/**
 * 自我进化状态管理
 */
import { create } from 'zustand';
import type { Experience, Skill, EvolutionEvent } from '../types';
import * as db from '../lib/db';
import { evolve, findRelevantExperiences, injectExperiences, optimizePrompt, logEvolution } from '../lib/evolution';

interface EvolutionState {
  // 数据
  experiences: Experience[];
  skills: Skill[];
  events: EvolutionEvent[];
  loaded: boolean;

  // 进化状态
  evolving: boolean;
  lastEvolutionResult: string | null;

  // 操作
  load: () => Promise<void>;
  runEvolve: (messages: import('../types').ChatMessage[]) => Promise<void>;
  optimizePersonaPrompt: (personaId: string) => Promise<void>;
  getRelevantContext: (userMessage: string) => string;
  deleteExperience: (id: string) => Promise<void>;
  deleteSkill: (id: string) => Promise<void>;
}

export const useEvolutionStore = create<EvolutionState>((set, get) => ({
  experiences: [],
  skills: [],
  events: [],
  loaded: false,
  evolving: false,
  lastEvolutionResult: null,

  load: async () => {
    const [experiences, skills, events] = await Promise.all([
      db.listExperiences(200),
      db.listSkills(),
      db.listEvolutionEvents(100),
    ]);
    set({ experiences, skills, events, loaded: true });
  },

  runEvolve: async (messages) => {
    set({ evolving: true, lastEvolutionResult: null });
    try {
      const { newExperiences, newSkill, reflection } = await evolve(messages);
      const parts: string[] = [];

      if (newExperiences.length > 0) {
        parts.push(`🧠 提取了 ${newExperiences.length} 条新经验`);
        set(state => ({
          experiences: [...newExperiences, ...state.experiences].slice(0, 200),
        }));
      }

      if (newSkill) {
        parts.push(`🔧 生成了新技能: ${newSkill.name}`);
        set(state => ({
          skills: [newSkill, ...state.skills],
        }));
      }

      if (reflection) {
        parts.push(`📊 反思评估: ${reflection.summary}`);
        if (reflection.improvement !== 'none') {
          parts.push(`💡 改进建议: ${reflection.improvement}`);
        }
      }

      // 刷新事件列表
      const events = await db.listEvolutionEvents(50);
      set({
        lastEvolutionResult: parts.join('\n') || '本次对话无可提取经验',
        evolving: false,
        events,
      });
    } catch (err) {
      set({
        evolving: false,
        lastEvolutionResult: `进化评估失败: ${String(err)}`,
      });
    }
  },

  optimizePersonaPrompt: async (personaId) => {
    // 从 useChatStore 获取当前 Persona
    const { useChatStore } = await import('./useChatStore');
    const persona = useChatStore.getState().personas.find(p => p.id === personaId);
    if (!persona) return;

    const experiences = get().experiences;
    if (experiences.length < 3) return;

    const summary = experiences
      .slice(0, 5)
      .map(e => e.content)
      .join('; ');

    const optimized = await optimizePrompt(persona.systemPrompt, summary);
    if (optimized && optimized !== persona.systemPrompt) {
      await useChatStore.getState().savePersona({ ...persona, systemPrompt: optimized });
      await logEvolution({
        type: 'prompt_optimized',
        summary: `优化了 Persona "${persona.name}" 的系统提示词`,
        relatedId: personaId,
        timestamp: Date.now(),
      });
    }
  },

  getRelevantContext: (userMessage) => {
    const { experiences } = get();
    const relevant = findRelevantExperiences(experiences, userMessage, 3);
    if (relevant.length === 0) return '';

    // 更新使用计数
    for (const exp of relevant) {
      exp.useCount++;
      db.upsertExperience(exp).catch(() => {});
    }

    return relevant
      .map(e => `[经验] ${e.content}`)
      .join('\n');
  },

  deleteExperience: async (id) => {
    await db.deleteExperience(id);
    set(state => ({
      experiences: state.experiences.filter(e => e.id !== id),
    }));
  },

  deleteSkill: async (id) => {
    await db.deleteSkill(id);
    set(state => ({
      skills: state.skills.filter(s => s.id !== id),
    }));
  },
}));
