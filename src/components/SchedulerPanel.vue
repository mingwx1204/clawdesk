<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** 后端 Schedule（serde tag="type"） */
type Schedule =
  | { type: "daily"; time: string }
  | { type: "weekly"; weekday: number; time: string }
  | { type: "interval"; seconds: number }
  | { type: "once"; atMs: number };

interface SchedTask {
  id: string;
  name: string;
  prompt: string;
  schedule: Schedule;
  enabled: boolean;
  lastRun: number;
  pushWechat: boolean;
  wechatTo?: string | null;
  sessionId?: string | null;
  notify: boolean;
}

interface SchedResult {
  taskId: string;
  name: string;
  ok: boolean;
  result?: string;
  error?: string;
  time: number;
}

const emit = defineEmits<{ close: [] }>();

const tasks = ref<SchedTask[]>([]);
const log = ref<string[]>([]);
const loading = ref(false);

// 添加表单
const formOpen = ref(false);
const name = ref("");
const prompt = ref("");
const schedType = ref<"daily" | "weekly" | "interval" | "once">("daily");
const time = ref("09:00");
const weekday = ref(1);
const seconds = ref(3600);
const onceAt = ref("");
const pushWechat = ref(false);
const wechatTo = ref("");
const notify = ref(true);

let unlisten: UnlistenFn | null = null;

function pushLog(s: string) {
  log.value.unshift(`[${new Date().toLocaleTimeString()}] ${s}`);
  if (log.value.length > 40) log.value.pop();
}
function fmtTs(ts: number): string {
  if (!ts) return "—";
  try { return new Date(ts).toLocaleString(); } catch { return "—"; }
}

function schedDesc(s: Schedule): string {
  switch (s.type) {
    case "daily": return `每天 ${s.time}`;
    case "weekly": return `每周${["一", "二", "三", "四", "五", "六", "日"][(s.weekday - 1) % 7]} ${s.time}`;
    case "interval": return `每 ${s.seconds >= 3600 ? (s.seconds / 3600).toFixed(1) + " 小时" : s.seconds >= 60 ? (s.seconds / 60).toFixed(0) + " 分钟" : s.seconds + " 秒"}`;
    case "once": return `一次性 · ${fmtTs(s.atMs)}`;
  }
}

onMounted(async () => {
  await refresh();
  try {
    unlisten = await listen<SchedResult>("scheduler://result", (e) => {
      const r = e.payload;
      if (r.ok) {
        pushLog(`✅ ${r.name}：${(r.result ?? "").slice(0, 120)}`);
      } else {
        pushLog(`❌ ${r.name}：${r.error ?? "未知错误"}`);
      }
      refresh();
    });
  } catch { /* 静默 */ }
});
onUnmounted(() => unlisten?.());

async function refresh() {
  loading.value = true;
  try {
    tasks.value = await invoke<SchedTask[]>("scheduler_list");
  } catch (e) {
    pushLog(`加载任务失败: ${e}`);
  } finally {
    loading.value = false;
  }
}

async function addTask() {
  if (!name.value.trim() || !prompt.value.trim()) {
    pushLog("请填写任务名称与任务内容");
    return;
  }
  let schedule: Schedule;
  switch (schedType.value) {
    case "daily":
      schedule = { type: "daily", time: time.value };
      break;
    case "weekly":
      schedule = { type: "weekly", weekday: Number(weekday.value), time: time.value };
      break;
    case "interval":
      schedule = { type: "interval", seconds: Number(seconds.value) || 3600 };
      break;
    case "once": {
      const ms = onceAt.value ? new Date(onceAt.value).getTime() : 0;
      if (!ms) { pushLog("请选择一次性触发时间"); return; }
      schedule = { type: "once", atMs: ms };
      break;
    }
  }
  try {
    await invoke("scheduler_add", {
      name: name.value.trim(),
      prompt: prompt.value.trim(),
      schedule,
      pushWechat: pushWechat.value,
      wechatTo: wechatTo.value.trim() || undefined,
      notify: notify.value,
    });
    pushLog(`📌 已添加任务「${name.value.trim()}」`);
    name.value = "";
    prompt.value = "";
    wechatTo.value = "";
    formOpen.value = false;
    await refresh();
  } catch (e) {
    pushLog(`添加失败: ${e}`);
  }
}

async function toggle(t: SchedTask) {
  try {
    await invoke("scheduler_set_enabled", { taskId: t.id, enabled: !t.enabled });
    await refresh();
  } catch (e) {
    pushLog(`操作失败: ${e}`);
  }
}

async function remove(t: SchedTask) {
  if (!window.confirm(`确定删除定时任务「${t.name}」？`)) return;
  try {
    await invoke("scheduler_remove", { taskId: t.id });
    pushLog(`🗑 已删除「${t.name}」`);
    await refresh();
  } catch (e) {
    pushLog(`删除失败: ${e}`);
  }
}

async function triggerNow(t: SchedTask) {
  pushLog(`⚡ 正在手动触发「${t.name}」…`);
  try {
    const r = await invoke<{ ok: boolean; result: string }>("scheduler_trigger_now", { taskId: t.id });
    pushLog(`✅ 「${t.name}」执行完成：${(r.result ?? "").slice(0, 150)}`);
  } catch (e) {
    pushLog(`❌ 「${t.name}」执行失败: ${e}`);
  }
  await refresh();
}
</script>

<template>
  <div class="sc-overlay">
    <div class="sc-card">
      <div class="sc-head">
        <div class="sc-title">
          <span class="sc-logo">⏰</span>
          <span>定时任务</span>
          <span class="sc-count">共 {{ tasks.length }} 个</span>
        </div>
        <button class="sc-close" @click="emit('close')">✕</button>
      </div>

      <div class="sc-body">
        <!-- 左侧：任务列表 -->
        <div class="sc-left">
          <div class="sc-toolbar">
            <button class="sc-btn sc-primary" @click="formOpen = !formOpen">
              {{ formOpen ? "收起表单" : "＋ 新建任务" }}
            </button>
            <button class="sc-btn" @click="refresh" :disabled="loading">{{ loading ? "加载中…" : "🔄 刷新" }}</button>
          </div>

          <!-- 添加表单 -->
          <div v-if="formOpen" class="sc-form">
            <input v-model="name" class="sc-input" placeholder="任务名称（如：每日新闻摘要）" />
            <textarea v-model="prompt" class="sc-textarea" placeholder="任务内容：到点后让 AI 执行什么？（如：总结今天的重要新闻并列出要点）" rows="3" />
            <div class="sc-form-row">
              <label class="sc-form-label">触发方式</label>
              <select v-model="schedType" class="sc-select">
                <option value="daily">每天定点</option>
                <option value="weekly">每周定点</option>
                <option value="interval">间隔周期</option>
                <option value="once">一次性</option>
              </select>
              <template v-if="schedType === 'daily' || schedType === 'weekly'">
                <input v-if="schedType === 'weekly'" v-model.number="weekday" type="number" min="1" max="7" class="sc-input sc-sm" title="1=周一…7=周日" />
                <input v-model="time" type="time" class="sc-input sc-sm" />
              </template>
              <template v-else-if="schedType === 'interval'">
                <input v-model.number="seconds" type="number" min="10" class="sc-input sc-sm" />
                <span class="sc-unit">秒</span>
              </template>
              <template v-else>
                <input v-model="onceAt" type="datetime-local" class="sc-input sc-lg" />
              </template>
            </div>
            <div class="sc-form-row">
              <label class="sc-check">
                <input type="checkbox" v-model="pushWechat" />
                微信推送
              </label>
              <input v-if="pushWechat" v-model="wechatTo" class="sc-input sc-md" placeholder="微信用户 ID（留空不发，可从消息日志复制）" />
              <label class="sc-check">
                <input type="checkbox" v-model="notify" />
                桌面通知
              </label>
            </div>
            <button class="sc-btn sc-primary sc-add" @click="addTask">添加任务</button>
          </div>

          <!-- 任务列表 -->
          <div v-if="!tasks.length && !loading" class="sc-empty">暂无定时任务，点「＋ 新建任务」添加</div>
          <div v-for="t in tasks" :key="t.id" class="sc-task" :class="{ off: !t.enabled }">
            <div class="sc-task-main">
              <span class="sc-task-name">{{ t.name }}</span>
              <span class="sc-task-desc">{{ schedDesc(t.schedule) }}</span>
              <span class="sc-task-prompt">{{ t.prompt.slice(0, 80) }}{{ t.prompt.length > 80 ? "…" : "" }}</span>
              <span class="sc-task-meta">
                上次 {{ fmtTs(t.lastRun) }}
                <template v-if="t.pushWechat"> · 推微信</template>
              </span>
            </div>
            <div class="sc-task-ops">
              <label class="sc-switch" :title="t.enabled ? '停用' : '启用'">
                <input type="checkbox" :checked="t.enabled" @change="toggle(t)" />
                <span class="sc-knob"></span>
              </label>
              <button class="sc-btn sc-sm" title="立即执行" @click="triggerNow(t)">⚡</button>
              <button class="sc-btn sc-sm sc-danger" title="删除" @click="remove(t)">✕</button>
            </div>
          </div>
        </div>

        <!-- 右侧：日志 -->
        <div class="sc-right">
          <div class="sc-log-title">运行日志</div>
          <div class="sc-log">
            <p v-for="(l, i) in log" :key="i" class="sc-log-line">{{ l }}</p>
            <p v-if="!log.length" class="sc-log-empty">暂无日志</p>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.sc-overlay {
  position: fixed; inset: 0; z-index: 300;
  background: rgba(0, 0, 0, 0.45);
  display: flex; align-items: center; justify-content: center;
  backdrop-filter: blur(4px);
}
.sc-card {
  width: min(900px, 92vw); height: min(620px, 88vh);
  background: linear-gradient(180deg, #1b2233 0%, #141a28 100%);
  border: 1px solid #2c3a55; border-radius: 14px;
  display: flex; flex-direction: column; overflow: hidden;
  box-shadow: 0 18px 60px rgba(0, 0, 0, 0.55);
}
.sc-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 14px 18px; border-bottom: 1px solid #26324a; flex-shrink: 0;
}
.sc-title { display: flex; align-items: center; gap: 8px; font-size: 15px; font-weight: 600; color: #e8edf7; }
.sc-logo { font-size: 16px; }
.sc-count { font-size: 12px; color: #94a3b8; font-weight: 400; }
.sc-close { background: none; border: none; color: #94a3b8; font-size: 15px; cursor: pointer; padding: 4px 8px; border-radius: 6px; }
.sc-close:hover { background: #26324a; color: #fff; }
.sc-body { display: flex; flex: 1; min-height: 0; }
.sc-left { width: 58%; border-right: 1px solid #26324a; padding: 14px; overflow-y: auto; display: flex; flex-direction: column; gap: 10px; }
.sc-right { flex: 1; padding: 14px; display: flex; flex-direction: column; min-width: 0; }
.sc-toolbar { display: flex; gap: 8px; flex-shrink: 0; }
.sc-btn {
  background: #26324a; color: #dbe4f0; border: 1px solid #33415e;
  border-radius: 8px; padding: 7px 14px; font-size: 13px; cursor: pointer; transition: 0.15s;
}
.sc-btn:hover { background: #2f3d59; }
.sc-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.sc-primary { background: #2563eb; border-color: #2563eb; color: #fff; }
.sc-primary:hover { background: #1d4ed8; }
.sc-danger { color: #fecaca; }
.sc-sm { padding: 5px 9px; font-size: 12px; }
.sc-form {
  background: #1e2739; border: 1px solid #2a3752; border-radius: 10px;
  padding: 12px; display: flex; flex-direction: column; gap: 8px;
}
.sc-input, .sc-textarea {
  background: #141a28; border: 1px solid #2c3a55; border-radius: 8px;
  color: #e8edf7; padding: 8px 10px; font-size: 13px; outline: none;
  width: 100%; box-sizing: border-box;
}
.sc-input:focus, .sc-textarea:focus { border-color: #3b82f6; }
.sc-textarea { resize: vertical; font-family: inherit; }
.sc-select { background: #141a28; border: 1px solid #2c3a55; border-radius: 8px; color: #e8edf7; padding: 7px 10px; font-size: 13px; outline: none; }
.sc-form-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.sc-form-label { font-size: 12.5px; color: #94a3b8; }
.sc-sm { width: auto; }
.sc-md { flex: 1; min-width: 140px; }
.sc-lg { flex: 1; min-width: 160px; }
.sc-unit { font-size: 12px; color: #94a3b8; }
.sc-add { align-self: flex-end; }
.sc-check { display: flex; align-items: center; gap: 6px; font-size: 13px; color: #cbd5e1; cursor: pointer; }
.sc-check input { accent-color: #2563eb; }
.sc-empty { color: #64748b; font-size: 13px; text-align: center; padding: 30px 0; }
.sc-task {
  background: #1e2739; border: 1px solid #2a3752; border-radius: 10px;
  padding: 10px 12px; display: flex; justify-content: space-between; gap: 10px; align-items: center;
}
.sc-task.off { opacity: 0.55; }
.sc-task-main { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.sc-task-name { font-size: 13.5px; font-weight: 600; color: #e8edf7; }
.sc-task-desc { font-size: 12px; color: #38bdf8; }
.sc-task-prompt { font-size: 12px; color: #a5b4cb; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sc-task-meta { font-size: 11px; color: #64748b; }
.sc-task-ops { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
.sc-switch { position: relative; display: inline-block; width: 34px; height: 19px; }
.sc-switch input { opacity: 0; width: 0; height: 0; }
.sc-knob { position: absolute; inset: 0; background: #334155; border-radius: 19px; transition: 0.2s; cursor: pointer; }
.sc-knob::before { content: ""; position: absolute; width: 13px; height: 13px; left: 3px; top: 3px; background: #cbd5e1; border-radius: 50%; transition: 0.2s; }
.sc-switch input:checked + .sc-knob { background: #34d399; }
.sc-switch input:checked + .sc-knob::before { transform: translateX(15px); background: #fff; }
.sc-log-title { font-size: 12px; color: #94a3b8; padding-bottom: 8px; border-bottom: 1px solid #26324a; flex-shrink: 0; }
.sc-log { flex: 1; overflow-y: auto; padding-top: 8px; font-size: 12px; }
.sc-log-line { margin: 3px 0; color: #a5b4cb; word-break: break-all; font-family: Consolas, monospace; }
.sc-log-empty { color: #4b5563; }
</style>
