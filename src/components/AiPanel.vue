<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * 智能 Agent 面板 —— 多轮对话 + 实时进度 + 规划 + 可取消。
 *
 * - 会话记忆：同一 sessionId 的历史由后端保留，多轮连续对话；
 * - 进度流：监听 `agent://progress` 事件实时展示每轮 / 每次工具调用；
 * - 取消：前端生成 runId，运行中可 `agent_cancel` 中断；
 * - 规划：usePlanning 开启 Plan-and-Execute。
 *
 * 安全：API Key 仅存于本组件内存 ref，不落盘。
 */

interface ToolCallRecord {
  toolId: string;
  arguments: unknown;
  status: string;
  output: unknown;
}

interface ToolLoopOutcome {
  rounds: RoundRecord[];
  finalText: string;
  truncated: boolean;
  usedRounds: number;
  /** 单次任务 Token 用量汇总（项目 5） */
  usage: TokenUsage;
}

interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

interface RoundRecord {
  round: number;
  modelText: string;
  toolCalls: ToolCallRecord[];
}

interface ChatMessage {
  role: "user" | "assistant" | "tool";
  content: string;
  toolCalls?: ToolCallRecord[];
}

type ProgressEvent =
  | { type: "roundStarted"; round: number }
  | { type: "modelText"; round: number; text: string }
  | {
      type: "toolCall";
      round: number;
      toolId: string;
      arguments: unknown;
      status: string;
      output: unknown;
    }
  | { type: "compaction"; kept: number; summaryChars: number; compactedCount: number }
  | {
      type: "confirmRequired";
      callId: string;
      toolId: string;
      /** "normal" | "high" —— 高危展示红色警告 + 二次确认 */
      riskLevel: "normal" | "high";
      arguments: unknown;
    }
  | { type: "cancelled" }
  | { type: "finished"; finalText: string; usedRounds: number; truncated: boolean };

/** 待确认的工具调用（confirmRequired 事件载荷）。 */
interface PendingConfirm {
  callId: string;
  toolId: string;
  /** "normal" | "high" —— 高危展示红色警告 + 二次确认 */
  riskLevel: "normal" | "high";
  arguments: unknown;
}

const apiKey = ref("");
const prompt = ref("");
const sessionId = ref("default");
const sessions = ref<string[]>([]);
const usePlanning = ref(false);
const running = ref(false);
const runId = ref("");
const messages = ref<ChatMessage[]>([]);
const liveProgress = ref<string[]>([]);
const error = ref<string | null>(null);
/** 待确认的工具调用（确认模式弹窗）。 */
const pendingConfirm = ref<PendingConfirm | null>(null);
/** 确认弹窗步骤：高危操作需二次确认。 */
const confirmStep = ref<"first" | "second">("first");
/** 最近一次任务的 Token 用量（对话底部状态栏，项目 5）。 */
const lastUsage = ref<TokenUsage | null>(null);
let unlisten: UnlistenFn | null = null;

onMounted(async () => {
  try {
    unlisten = await listen<ProgressEvent>("agent://progress", (e) => {
      handleProgress(e.payload);
    });
    await refreshSessions();
  } catch (e) {
    error.value = `事件监听失败: ${String(e)}`;
  }
});

onUnmounted(() => {
  unlisten?.();
});

async function refreshSessions(): Promise<void> {
  sessions.value = await invoke<string[]>("agent_sessions");
}

function statusIcon(s: string): string {
  if (s === "success") return "✅";
  if (s === "error") return "❌";
  return "⏸";
}

function handleProgress(ev: ProgressEvent): void {
  switch (ev.type) {
    case "roundStarted":
      liveProgress.value.push(ev.round === 0 ? "📋 规划阶段…" : `🔄 第 ${ev.round} 轮`);
      break;
    case "modelText":
      liveProgress.value.push(`💬 ${ev.text}`);
      break;
    case "toolCall": {
      const out = JSON.stringify(ev.output);
      liveProgress.value.push(
        `${statusIcon(ev.status)} 工具 ${ev.toolId} → ${ev.status}${out.length > 100 ? out.slice(0, 100) + "…" : out}`
      );
      break;
    }
    case "confirmRequired": {
      // 确认模式：弹出工具确认弹窗，展示工具名 / 参数 / 风险等级
      pendingConfirm.value = {
        callId: ev.callId,
        toolId: ev.toolId,
        riskLevel: ev.riskLevel,
        arguments: ev.arguments,
      };
      confirmStep.value = "first";
      break;
    }
    case "compaction":
      liveProgress.value.push(`📦 上下文已压缩（保留 ${ev.kept} 条，共压缩 ${ev.compactedCount} 次）`);
      break;
    case "cancelled":
      liveProgress.value.push("⏹ 已取消");
      break;
    case "finished":
      liveProgress.value.push(`✅ 完成（${ev.usedRounds} 轮${ev.truncated ? " · 熔断" : ""}）`);
      break;
  }
}

/** 确认执行：高危操作先进入二次确认，二次确认后才真正放行。 */
async function approveCall(): Promise<void> {
  const p = pendingConfirm.value;
  if (!p) return;
  if (p.riskLevel === "high" && confirmStep.value === "first") {
    // 高危：第一次点击只是进入二次确认
    confirmStep.value = "second";
    return;
  }
  await invoke("agent_confirm_call", { callId: p.callId, approve: true });
  liveProgress.value.push(`🟢 已确认执行 ${p.toolId}`);
  pendingConfirm.value = null;
}

/** 拒绝执行。 */
async function rejectCall(): Promise<void> {
  const p = pendingConfirm.value;
  if (!p) return;
  await invoke("agent_confirm_call", { callId: p.callId, approve: false });
  liveProgress.value.push(`🔴 已拒绝 ${p.toolId}`);
  pendingConfirm.value = null;
}

/** 参数预览（截断防刷屏）。 */
function formatArgs(args: unknown): string {
  const s = JSON.stringify(args);
  if (!s) return "";
  return s.length > 300 ? s.slice(0, 300) + "…" : s;
}

async function send(): Promise<void> {
  if (!apiKey.value.trim() || !prompt.value.trim() || running.value) return;
  running.value = true;
  error.value = null;
  liveProgress.value = [];
  const text = prompt.value;
  prompt.value = "";
  messages.value.push({ role: "user", content: text });
  runId.value = `run-${Date.now()}`;

  try {
    const outcome = await invoke<ToolLoopOutcome>("agent_chat", {
      apiKey: apiKey.value.trim(),
      sessionId: sessionId.value,
      runId: runId.value,
      prompt: text,
      usePlanning: usePlanning.value,
    });
    messages.value.push({ role: "assistant", content: outcome.finalText });
    // Token 用量汇总（底部状态栏，项目 5）
    if (outcome.usage) {
      lastUsage.value = outcome.usage;
    }
  } catch (e) {
    const msg = typeof e === "string" ? e : JSON.stringify(e);
    error.value = msg;
    messages.value.push({ role: "assistant", content: `❌ ${msg}` });
  } finally {
    running.value = false;
    runId.value = "";
    await refreshSessions();
  }
}

async function cancelRun(): Promise<void> {
  if (!running.value || !runId.value) return;
  await invoke("agent_cancel", { runId: runId.value });
}

async function newSession(): Promise<void> {
  sessionId.value = `sess-${Date.now()}`;
  messages.value = [];
  liveProgress.value = [];
  await refreshSessions();
}

async function deleteSession(id: string): Promise<void> {
  await invoke("agent_session_delete", { sessionId: id });
  if (sessionId.value === id) {
    sessionId.value = "default";
    messages.value = [];
  }
  await refreshSessions();
}

function selectSession(id: string): void {
  sessionId.value = id;
  messages.value = [];
  liveProgress.value = [];
}
</script>

<template>
  <section class="panel">
    <h3>🤖 智能 Agent</h3>

    <div class="key-row">
      <input
        v-model="apiKey"
        type="password"
        class="field key-field"
        placeholder="DeepSeek API Key（仅内存，不落盘）"
      />
      <button class="btn-primary" :disabled="running || !apiKey.trim() || !prompt.trim()" @click="send">
        {{ running ? "运行中…" : "发送" }}
      </button>
      <button v-if="running" class="btn-danger" @click="cancelRun">⏹ 取消</button>
    </div>

    <div class="session-row">
      <select v-model="sessionId" class="field session-select" @change="selectSession(sessionId)">
        <option v-for="s in sessions" :key="s" :value="s">{{ s }}</option>
        <option value="default">default</option>
      </select>
      <button class="btn-secondary" @click="newSession">＋ 新会话</button>
      <button
        class="btn-secondary"
        :disabled="running || sessionId === 'default'"
        @click="deleteSession(sessionId)"
      >
        🗑 删除会话
      </button>
      <label class="plan-toggle">
        <input v-model="usePlanning" type="checkbox" :disabled="running" />
        规划模式（Plan-and-Execute）
      </label>
    </div>

    <textarea
      v-model="prompt"
      class="field prompt-field"
      placeholder="例如：现在几点了？然后帮我计算 (12+8)*3，最后生成一张 128x128 的测试图"
      rows="2"
      @keydown.enter.exact.prevent="send"
    />

    <p v-if="error" class="error-msg">❌ {{ error }}</p>

    <!-- 实时进度流 -->
    <div v-if="liveProgress.length" class="progress-box">
      <h4>实时进度</h4>
      <div v-for="(line, i) in liveProgress" :key="i" class="progress-line">{{ line }}</div>
    </div>

    <!-- 工具调用确认弹窗（确认模式） -->
    <div v-if="pendingConfirm" class="confirm-overlay">
      <div class="confirm-modal" :class="{ 'confirm-high': pendingConfirm.riskLevel === 'high' }">
        <h4 class="confirm-title">
          {{ pendingConfirm.riskLevel === "high" ? "🔴 高危操作确认" : "🟡 工具调用确认" }}
        </h4>
        <div class="confirm-body">
          <p class="confirm-tool"><b>工具：</b>{{ pendingConfirm.toolId }}</p>
          <p class="confirm-args"><b>参数：</b><code>{{ formatArgs(pendingConfirm.arguments) }}</code></p>
          <p v-if="pendingConfirm.riskLevel === 'high' && confirmStep === 'first'" class="confirm-warn">
            ⚠️ 此操作风险等级为【高危】，点击"确认执行"后将再次要求确认。
          </p>
          <p v-if="pendingConfirm.riskLevel === 'high' && confirmStep === 'second'" class="confirm-warn">
            ⚠️ 高危操作二次确认：确定要让 Agent 执行此操作吗？
          </p>
        </div>
        <div class="confirm-actions">
          <button class="btn-secondary" @click="rejectCall">❌ 拒绝</button>
          <button class="btn-primary" @click="approveCall">
            {{ pendingConfirm.riskLevel === "high" && confirmStep === "first" ? "确认执行" : "✅ 确认执行" }}
          </button>
        </div>
      </div>
    </div>

    <!-- 多轮对话历史 -->
    <div v-if="messages.length" class="chat-box">
      <div
        v-for="(m, i) in messages"
        :key="i"
        class="chat-msg"
        :class="m.role"
      >
        <div class="msg-label">
          {{ m.role === "user" ? "🧑 你" : m.role === "assistant" ? "🤖 Agent" : "🔧 工具" }}
        </div>
        <div class="msg-content">{{ m.content }}</div>
      </div>
    </div>

    <!-- 对话底部状态栏：Token 用量汇总（项目 5） -->
    <div v-if="lastUsage" class="usage-bar">
      <span>🪙 本轮 Token：输入 {{ lastUsage.promptTokens }} / 输出 {{ lastUsage.completionTokens }} / 总计 {{ lastUsage.totalTokens }}</span>
    </div>
  </section>
</template>

<style scoped>
.panel {
  font-family: system-ui, sans-serif;
  max-width: 960px;
  margin: 1rem auto 0;
  padding: 1rem 1.5rem;
  border: 1px solid #e5e7eb;
  border-radius: 10px;
  background: #fff;
}

.key-row {
  display: flex;
  gap: 0.5rem;
  margin-bottom: 0.5rem;
}

.field {
  padding: 0.45rem 0.6rem;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  font-size: 0.88rem;
  font-family: inherit;
  box-sizing: border-box;
  width: 100%;
}

.key-field {
  flex: 1;
}

.prompt-field {
  resize: vertical;
}

.session-row {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  margin-bottom: 0.5rem;
  flex-wrap: wrap;
}

.session-select {
  width: 180px;
}

.plan-toggle {
  font-size: 0.82rem;
  color: #555;
  display: flex;
  align-items: center;
  gap: 0.3rem;
}

.btn-primary {
  padding: 0.45rem 1.25rem;
  border: none;
  border-radius: 8px;
  background: #1d4ed8;
  color: #fff;
  font-size: 0.9rem;
  cursor: pointer;
  white-space: nowrap;
}

.btn-danger {
  padding: 0.45rem 1rem;
  border: none;
  border-radius: 8px;
  background: #dc2626;
  color: #fff;
  font-size: 0.85rem;
  cursor: pointer;
  white-space: nowrap;
}

.btn-secondary {
  padding: 0.45rem 0.9rem;
  border: 1px solid #d1d5db;
  border-radius: 8px;
  background: #fff;
  color: #444;
  font-size: 0.82rem;
  cursor: pointer;
  white-space: nowrap;
}

.btn-primary:disabled,
.btn-secondary:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.error-msg {
  color: #b91c1c;
  font-size: 0.85rem;
}

.progress-box {
  margin-top: 0.75rem;
  border: 1px solid #e0e7ff;
  border-radius: 8px;
  background: #eef2ff;
  padding: 0.5rem 0.75rem;
  font-size: 0.8rem;
}

.confirm-overlay {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}

.confirm-modal {
  background: #fff;
  border-radius: 12px;
  padding: 1.1rem 1.3rem;
  max-width: 480px;
  width: 90%;
  box-shadow: 0 10px 30px rgba(0, 0, 0, 0.25);
  border: 1px solid #e5e7eb;
}

.confirm-modal.confirm-high {
  border-color: #fca5a5;
}

.confirm-title {
  margin: 0 0 0.6rem;
  font-size: 1rem;
}

.confirm-body {
  font-size: 0.85rem;
  margin-bottom: 0.8rem;
}

.confirm-tool {
  margin: 0.3rem 0;
  word-break: break-all;
}

.confirm-args {
  margin: 0.3rem 0;
  word-break: break-all;
}

.confirm-args code {
  background: #f1f5f9;
  padding: 0.15rem 0.4rem;
  border-radius: 4px;
  font-size: 0.78rem;
}

.confirm-warn {
  color: #b91c1c;
  font-weight: 600;
  margin: 0.4rem 0 0;
}

.confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 0.5rem;
}

.progress-box h4 {
  margin: 0 0 0.25rem;
  font-size: 0.8rem;
  color: #3730a3;
}

.progress-line {
  padding: 0.1rem 0;
  color: #3730a3;
  font-family: ui-monospace, monospace;
  word-break: break-all;
}

.chat-box {
  margin-top: 0.75rem;
  border-top: 1px solid #eee;
  padding-top: 0.5rem;
}

.chat-msg {
  margin: 0.5rem 0;
  padding: 0.5rem 0.75rem;
  border-radius: 8px;
}

.chat-msg.user {
  background: #eff6ff;
  border: 1px solid #bfdbfe;
}

.chat-msg.assistant {
  background: #f0fdf4;
  border: 1px solid #bbf7d0;
}

.chat-msg.tool {
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  font-size: 0.82rem;
}

.usage-bar {
  margin-top: 0.75rem;
  padding: 0.4rem 0.75rem;
  border-top: 1px solid #eee;
  font-size: 0.78rem;
  color: #6b7280;
  text-align: right;
}

.msg-label {
  font-size: 0.75rem;
  font-weight: 600;
  color: #666;
  margin-bottom: 0.2rem;
}

.msg-content {
  white-space: pre-wrap;
  word-break: break-word;
  font-size: 0.9rem;
}
</style>
