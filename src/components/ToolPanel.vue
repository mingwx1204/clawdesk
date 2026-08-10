<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { listTools } from "../core/ToolRegistry";
import { invokeTool, MAX_TOOL_ROUNDS } from "../core/ToolDispatcher";
import type { UnifiedToolDef, ToolCall, ToolResult } from "../types/tool";

/**
 * 工具面板 —— 阶段 2。
 *
 * 新增：
 * - round 计数器（1 → MAX_TOOL_ROUNDS），前端显式管理循环轮次；
 * - 高危工具确认弹窗（`window.confirm`），在调用前拦截 isHighRisk 工具；
 * - 熔断 UI：round 超过上限时禁用调用，显示红色提示；
 * - 重置轮次按钮。
 *
 * 契约：
 * - 工具集合完全由后端动态注册表决定，按 source 分组渲染；
 * - `uiPayload.displayHint` 仅驱动卡片样式，绝不参与 LLM 上下文构建；
 * - 参数表单由 `UnifiedToolDef.params` 动态生成。
 */

const tools = ref<UnifiedToolDef[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

const activeTool = ref<UnifiedToolDef | null>(null);
const argInputs = ref<Record<string, string>>({});
const result = ref<ToolResult | null>(null);
const resultRound = ref(0); // 当前结果显示对应的轮次
const calling = ref(false);
const callSeq = ref(1);

// ── 阶段 2：循环轮次管理 ──
const currentRound = ref(1);
const fuseBlown = ref(false); // 后端/前端返回熔断后置为 true

const roundExceeded = computed(() => currentRound.value > MAX_TOOL_ROUNDS);

const sources = computed(() => [...new Set(tools.value.map((t) => t.source))].sort());

function toolsBySource(source: string): UnifiedToolDef[] {
  return tools.value.filter((t) => t.source === source);
}

interface DisplayHint {
  icon?: string;
  tone?: string;
  note?: string;
}

function displayHint(def: UnifiedToolDef): DisplayHint {
  const ui = def.uiPayload as { displayHint?: DisplayHint } | undefined;
  return ui?.displayHint ?? {};
}

onMounted(async () => {
  loading.value = true;
  try {
    tools.value = await listTools();
  } catch (e) {
    error.value = typeof e === "string" ? e : JSON.stringify(e);
  } finally {
    loading.value = false;
  }
});

function selectTool(def: UnifiedToolDef): void {
  activeTool.value = def;
  result.value = null;
  argInputs.value = {};
  resetRound();
  for (const p of def.params) {
    argInputs.value[p.name] = p.default !== undefined ? String(p.default) : "";
  }
}

// ── 阶段 2：高危确认 ──
function confirmHighRisk(): boolean {
  if (!activeTool.value?.isHighRisk) return true;
  return window.confirm(
    `⚠️ 高危工具确认\n\nID: ${activeTool.value.id}\n描述: ${activeTool.value.description}\n\n确定要执行吗？`
  );
}

async function callTool(): Promise<void> {
  if (!activeTool.value || calling.value) return;
  if (roundExceeded.value) {
    fuseBlown.value = true;
    result.value = {
      status: "error",
      message: `工具循环轮次 ${currentRound.value} 超过熔断上限 ${MAX_TOOL_ROUNDS}`,
    };
    resultRound.value = currentRound.value;
    return;
  }

  // 高危确认（阶段 2）
  if (!confirmHighRisk()) {
    result.value = { status: "interrupted", reason: "用户取消高危操作" };
    resultRound.value = currentRound.value;
    return;
  }

  calling.value = true;
  result.value = null;

  const args: Record<string, unknown> = {};
  for (const p of activeTool.value.params) {
    const raw = argInputs.value[p.name] ?? "";
    if (raw === "") continue;
    if (p.type === "number") args[p.name] = Number(raw);
    else if (p.type === "boolean") args[p.name] = raw === "true";
    else args[p.name] = raw;
  }

  const call: ToolCall = {
    id: `call-${callSeq.value++}`,
    toolId: activeTool.value.id,
    arguments: args,
    round: currentRound.value,
  };

  try {
    result.value = await invokeTool(call);
    resultRound.value = currentRound.value;

    // 成功后递增轮次
    if (result.value.status === "success") {
      currentRound.value++;
    }

    // 检测熔断信号（来自前端或后端）
    if (result.value.status === "error") {
      const msg = result.value.message.toLowerCase();
      if (msg.includes("超过熔断") || msg.includes("max_round") || msg.includes("maxrounds")) {
        fuseBlown.value = true;
      }
    }
  } finally {
    calling.value = false;
  }
}

function resetRound(): void {
  currentRound.value = 1;
  fuseBlown.value = false;
  result.value = null;
  resultRound.value = 0;
}

function resultText(): string {
  if (!result.value) return "";
  let prefix = `[轮次 ${resultRound.value}] `;
  if (result.value.status === "success") {
    return prefix + JSON.stringify(result.value.output, null, 2);
  }
  if (result.value.status === "error") return prefix + `❌ ${result.value.message}`;
  return prefix + `⏸ ${result.value.reason}`;
}

const resultIsError = computed(() => result.value?.status === "error");

/** 轮次指示器 CSS 类 */
const roundIndicatorClass = computed(() => {
  if (fuseBlown.value) return "blown";
  if (roundExceeded.value) return "blown";
  if (currentRound.value >= MAX_TOOL_ROUNDS) return "critical";
  return "";
});
</script>

<template>
  <section class="tool-panel">
    <h2>工具面板</h2>

    <p v-if="loading" class="hint">加载工具列表中…</p>
    <p v-if="error" class="error">{{ error }}</p>

    <div v-for="src in sources" :key="src" class="tool-group">
      <h3>来源：{{ src }}</h3>
      <div class="tool-grid">
        <button
          v-for="def in toolsBySource(src)"
          :key="def.id"
          class="tool-card"
          :class="displayHint(def).tone ?? 'neutral'"
          :title="def.description"
          @click="selectTool(def)"
        >
          <span class="tool-icon">{{ displayHint(def).icon ?? "🔧" }}</span>
          <span class="tool-name">{{ def.name }}</span>
          <span class="tool-id">{{ def.id }}</span>
          <span v-if="def.isHighRisk" class="risk-badge">高危</span>
        </button>
      </div>
    </div>

    <div v-if="activeTool" class="tool-detail">
      <div class="detail-header">
        <h3>{{ activeTool.id }}</h3>
        <!-- 阶段 2：轮次指示器 -->
        <span class="round-badge" :class="roundIndicatorClass">
          🔄 {{ roundExceeded ? '⛔ 已熔断' : `${currentRound} / ${MAX_TOOL_ROUNDS}` }}
        </span>
      </div>
      <p class="desc">{{ activeTool.description }}</p>
      <p v-if="displayHint(activeTool).note" class="note">
        ℹ️ {{ displayHint(activeTool).note }}
      </p>

      <!-- 阶段 2：熔断提示 -->
      <p v-if="fuseBlown" class="fuse-warning">
        ⛔ 工具循环已达到最大轮次（{{ MAX_TOOL_ROUNDS }}），已触发熔断。请重置轮次后继续。
      </p>

      <div v-for="p in activeTool.params" :key="p.name" class="param-row">
        <label>
          {{ p.name }}<span v-if="p.required" class="req">*</span>
          <span class="ptype">（{{ p.type }}）</span>
        </label>
        <input v-model="argInputs[p.name]" :placeholder="p.description" />
      </div>
      <p v-if="!activeTool.params.length" class="hint">该工具无参数</p>

      <div class="action-bar">
        <button
          class="call-btn"
          :disabled="calling || roundExceeded || fuseBlown"
          @click="callTool"
        >
          {{ calling ? "调用中…" : "调用" }}
        </button>
        <button class="reset-btn" @click="resetRound">重置轮次</button>
      </div>

      <pre
        v-if="result"
        class="result"
        :class="{ error: resultIsError, interrupted: result.status === 'interrupted' }"
      >{{ resultText() }}</pre>
    </div>

    <p v-if="!loading && !tools.length" class="hint">暂无已注册工具</p>
  </section>
</template>

<style scoped>
.tool-panel {
  font-family: system-ui, sans-serif;
  max-width: 960px;
  margin: 0 auto;
  padding: 1rem 1.5rem 3rem;
}

.tool-group {
  margin-top: 1.25rem;
}

.tool-group h3 {
  font-size: 0.9rem;
  color: #666;
  border-bottom: 1px solid #eee;
  padding-bottom: 0.25rem;
}

.tool-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 0.75rem;
  margin-top: 0.5rem;
}

.tool-card {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.25rem;
  min-width: 120px;
  padding: 0.75rem 1rem;
  border: 1px solid #ddd;
  border-radius: 10px;
  background: #fafafa;
  cursor: pointer;
  transition: transform 0.1s, box-shadow 0.1s;
}

.tool-card:hover {
  transform: translateY(-2px);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.08);
}

.tool-card.info {
  border-color: #93c5fd;
  background: #eff6ff;
}

.tool-card.accent {
  border-color: #c4b5fd;
  background: #f5f3ff;
}

.tool-card.warning {
  border-color: #fcd34d;
  background: #fffbeb;
}

.tool-icon {
  font-size: 1.5rem;
}

.tool-name {
  font-weight: 600;
}

.tool-id {
  font-size: 0.7rem;
  color: #999;
}

.risk-badge {
  font-size: 0.65rem;
  color: #b91c1c;
  border: 1px solid #fca5a5;
  border-radius: 4px;
  padding: 0 4px;
  background: #fef2f2;
}

.tool-detail {
  margin-top: 1.5rem;
  padding: 1rem 1.25rem;
  border: 1px solid #e5e7eb;
  border-radius: 10px;
  background: #fff;
}

.detail-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1rem;
}

.detail-header h3 {
  margin: 0;
}

/* 阶段 2：轮次指示器 */
.round-badge {
  font-size: 0.8rem;
  background: #e0f2fe;
  color: #0369a1;
  border-radius: 12px;
  padding: 0.2rem 0.6rem;
  white-space: nowrap;
}

.round-badge.critical {
  background: #fef3c7;
  color: #92400e;
}

.round-badge.blown {
  background: #fecaca;
  color: #991b1b;
  font-weight: 700;
}

.desc {
  color: #555;
  font-size: 0.9rem;
}

.note {
  color: #92400e;
  font-size: 0.85rem;
}

/* 阶段 2：熔断警告 */
.fuse-warning {
  background: #fef2f2;
  border: 1px solid #fecaca;
  border-radius: 8px;
  padding: 0.5rem 0.75rem;
  color: #991b1b;
  font-size: 0.85rem;
  font-weight: 600;
}

.param-row {
  margin: 0.75rem 0;
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.param-row label {
  font-size: 0.85rem;
}

.param-row .req {
  color: #dc2626;
  margin-left: 2px;
}

.param-row .ptype {
  color: #999;
}

.param-row input {
  padding: 0.4rem 0.6rem;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  font-size: 0.9rem;
}

.action-bar {
  display: flex;
  gap: 0.5rem;
  margin-top: 0.5rem;
}

.call-btn {
  padding: 0.45rem 1.25rem;
  border: none;
  border-radius: 8px;
  background: #1d4ed8;
  color: #fff;
  font-size: 0.9rem;
  cursor: pointer;
}

.call-btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.reset-btn {
  padding: 0.45rem 1rem;
  border: 1px solid #d1d5db;
  border-radius: 8px;
  background: #fff;
  color: #444;
  font-size: 0.85rem;
  cursor: pointer;
}

.reset-btn:hover {
  background: #f3f4f6;
}

.result {
  margin-top: 1rem;
  padding: 0.75rem;
  border-radius: 8px;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  font-size: 0.85rem;
  white-space: pre-wrap;
  word-break: break-all;
}

.result.error {
  background: #fef2f2;
  border-color: #fecaca;
  color: #b91c1c;
}

.result.interrupted {
  background: #fffbeb;
  border-color: #fcd34d;
  color: #92400e;
}

.hint {
  color: #888;
  font-size: 0.85rem;
}

.error {
  color: #b91c1c;
}
</style>
