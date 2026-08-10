<script setup lang="ts">
/**
 * 侧边栏 —— 会话列表（项目 10-11 增强）：
 * - 分支会话缩进 + 🔀 标注（§十二.2 从属关系）；
 * - 有断点的会话显示「▶ 续跑」按钮（§十二.1）；
 * - Fork 按钮完整拷贝当前会话记忆生成独立分支。
 */
defineProps<{
  sessions: string[];
  sessionId: string;
  running: boolean;
  branches: Record<string, string[]>;
  checkpoints: Record<string, boolean>;
}>();
const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "new"): void;
  (e: "delete", id: string): void;
  (e: "fork", id: string): void;
  (e: "resume", id: string): void;
}>();
</script>

<template>
  <aside class="sidebar">
    <div class="sb-header">
      <span class="sb-title">会话</span>
      <button class="sb-new" @click="emit('new')" title="新建会话">+</button>
    </div>
    <div class="sb-list">
      <template v-for="s in sessions" :key="s">
        <!-- 主会话 -->
        <button
          v-if="!branches[s]"
          class="sb-item"
          :class="{ active: s === sessionId }"
          @click="emit('select', s)"
        >
          <span class="sb-name">{{ s }}</span>
          <span v-if="checkpoints[s]" class="sb-cp" title="有断点可续跑" @click.stop="emit('resume', s)">▶</span>
          <span class="sb-fork" title="Fork 分支会话" @click.stop="emit('fork', s)">⑂</span>
          <span class="sb-del" @click.stop="() => emit('delete', s)">x</span>
        </button>
        <!-- 分支会话（缩进 + 🔀 标注从属） -->
        <button
          v-for="b in branches[s] || []"
          :key="b"
          class="sb-item sb-branch"
          :class="{ active: b === sessionId }"
          @click="emit('select', b)"
        >
          <span class="sb-name">🔀 {{ b }}</span>
          <span v-if="checkpoints[b]" class="sb-cp" title="有断点可续跑" @click.stop="emit('resume', b)">▶</span>
          <span class="sb-fork" title="Fork 分支会话" @click.stop="emit('fork', b)">⑂</span>
          <span class="sb-del" @click.stop="() => emit('delete', b)">x</span>
        </button>
      </template>
      <p v-if="!sessions.length" class="sb-empty">暂无会话</p>
    </div>
    <div class="sb-footer">
      <span class="dot" :class="{ running }" />
      <span>{{ running ? '运行中' : '就绪' }}</span>
    </div>
  </aside>
</template>

<style scoped>
.sidebar { width: 240px; background: var(--color-sidebar-bg); display: flex; flex-direction: column; border-right: 1px solid #1a1a1a; flex-shrink: 0; }
.sb-header { display: flex; align-items: center; gap: 8px; padding: 12px 14px; border-bottom: 1px solid #1a1a1a; }
.sb-title { flex: 1; font-weight: 600; font-size: 13px; color: var(--color-text-secondary); }
.sb-new { font-size: 16px; color: var(--color-text-secondary); padding: 2px 8px; border-radius: 4px; }
.sb-new:hover { background: var(--color-surface-hover); color: var(--color-text); }
.sb-list { flex: 1; overflow-y: auto; padding: 6px; }
.sb-item { display: flex; align-items: center; justify-content: space-between; width: 100%; padding: 8px 10px; border-radius: var(--radius-sm); color: var(--color-text-secondary); font-size: 12px; text-align: left; margin-bottom: 1px; }
.sb-item:hover { background: var(--color-surface-hover); }
.sb-item.active { background: var(--color-surface-hover); color: var(--color-text); }
.sb-item.sb-branch { padding-left: 20px; }
.sb-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; }
.sb-cp { font-size: 11px; color: var(--color-accent); padding: 2px 4px; visibility: hidden; }
.sb-item:hover .sb-cp { visibility: visible; }
.sb-fork { font-size: 13px; color: var(--color-text-muted); padding: 2px 4px; visibility: hidden; }
.sb-item:hover .sb-fork { visibility: visible; }
.sb-fork:hover { color: var(--color-accent); }
.sb-del { visibility: hidden; font-size: 11px; color: var(--color-danger); padding: 2px 4px; }
.sb-item:hover .sb-del { visibility: visible; }
.sb-empty { text-align: center; color: var(--color-text-muted); margin-top: 16px; font-size: 12px; }
.sb-footer { display: flex; align-items: center; gap: 8px; padding: 10px 16px; border-top: 1px solid #1a1a1a; font-size: 12px; color: var(--color-text-secondary); }
.dot { width: 7px; height: 7px; border-radius: 50%; background: var(--color-text-muted); }
.dot.running { background: var(--color-accent); animation: spin 0.6s linear infinite; }
</style>
