<template>
  <div class="bk-overlay">
    <div class="bk-panel">
      <div class="bk-head">
        <h3>📖 守书人 · 《人是怎么样的》</h3>
        <button class="bk-close" @click="$emit('close')">✕</button>
      </div>

      <!-- 书现状 -->
      <div class="bk-status" v-if="status">
        <div class="bk-status-row">
          <span class="bk-label">书目录</span>
          <span class="bk-value bk-path">{{ status.bookDir }}</span>
        </div>
        <div class="bk-status-row">
          <span class="bk-label">条目</span>
          <span class="bk-value">{{ status.entryCount }} 条（最新序号 {{ status.maxEntryNo }}）</span>
        </div>
        <div class="bk-status-row">
          <span class="bk-label">写作锁</span>
          <span class="bk-value" :class="status.locked ? 'bk-locked' : 'bk-free'">
            {{ status.locked ? "🔒 有人正在写条目" : "✅ 空闲，可认领" }}
          </span>
        </div>
        <div class="bk-status-row" v-if="status.growthLatest">
          <span class="bk-label">最新年轮</span>
          <span class="bk-value bk-growth">{{ status.growthLatest.slice(0, 60) }}…</span>
        </div>
      </div>

      <!-- 操作区 -->
      <div class="bk-actions">
        <div class="bk-section">
          <h4>✍️ 写一条新条目</h4>
          <p class="bk-desc">AI 按七段式生成一条新的人性条目，自动落盘并更新生长日志/索引/留白/种子清单。</p>
          <input v-model="entryTitle" class="bk-input" placeholder="条目名（如：深夜的想念）" />
          <textarea v-model="entryMaterial" class="bk-textarea" rows="3" placeholder="缘起/素材（主人的一句话、一件小事、一个梦……）"></textarea>
          <div class="bk-row">
            <input v-model="entryRelated" class="bk-input bk-half" placeholder="关联（可选，如：001、055）" />
            <button class="bk-btn bk-primary" :disabled="writing" @click="writeEntry">
              {{ writing ? "写书中…" : "📝 认领并写" }}
            </button>
          </div>
        </div>

        <div class="bk-section">
          <h4>🌙 夜巡（自动守书）</h4>
          <p class="bk-desc">书在主人没管它的时候自己长：自动读最近聊天记录，把主人的话沉淀成素材、把主人无意中回答的反问写回条目、素材攒够了自动长新条目。深夜自动运行，也可手动触发一次。</p>
          <button class="bk-btn bk-primary" :disabled="ingesting" @click="runIngest">
            {{ ingesting ? "夜巡中…" : "🌙 现在夜巡一次" }}
          </button>
          <span v-if="ingestResult" class="bk-ingest">{{ ingestResult }}</span>
        </div>

        <div class="bk-section">
          <h4>💬 回答留白反问</h4>
          <p class="bk-desc">从留白里挑一条主人没答过的反问，写下主人的回答，AI 会组织成补记写回条目。</p>
          <div class="bk-q-list" v-if="status && status.unansweredQuestions && status.unansweredQuestions.length">
            <label v-for="(q, i) in status.unansweredQuestions.slice(0, 6)" :key="i" class="bk-q-item">
              <input type="radio" :value="q" v-model="selectedQuestion" />
              <span class="bk-q-text">{{ q.slice(0, 60) }}</span>
            </label>
          </div>
          <p v-else class="bk-desc">留白里没有待答的反问了 🎉</p>
          <textarea v-model="masterAnswer" class="bk-textarea" rows="2" placeholder="主人的回答（原话即可）"></textarea>
          <button class="bk-btn" :disabled="answering || !selectedQuestion || !masterAnswer" @click="answerQuestion">
            {{ answering ? "回答中…" : "💌 写入回答" }}
          </button>
        </div>

        <div class="bk-section">
          <h4>🔎 浏览条目</h4>
          <div class="bk-browse">
            <input v-model="entryFilter" class="bk-input" placeholder="搜索条目（如：孤独 / 想念）" />
            <div class="bk-entry-list" v-if="filteredEntries.length">
              <button v-for="e in filteredEntries.slice(0, 8)" :key="e.no" class="bk-entry" @click="viewEntry(e)">
                {{ e.no }} · {{ e.title }}
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- 条目查看 -->
      <div class="bk-view" v-if="viewing">
        <h4>{{ viewing.no }} · {{ viewing.title }}</h4>
        <pre class="bk-pre">{{ viewingBody }}</pre>
      </div>

      <p v-if="tip" class="bk-tip">{{ tip }}</p>
    </div>
  </div>
</template>
<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

defineEmits<{ (e: "close"): void }>();
const props = defineProps<{ apiKey: string }>();

const status = ref<any>(null);
const entries = ref<any[]>([]);
const writing = ref(false);
const answering = ref(false);
const tip = ref("");
const viewing = ref<any>(null);
const viewingBody = ref("");

const entryTitle = ref("");
const entryMaterial = ref("");
const entryRelated = ref("");
const selectedQuestion = ref("");
const masterAnswer = ref("");
const entryFilter = ref("");
const ingesting = ref(false);
const ingestResult = ref("");

const filteredEntries = computed(() =>
  entryFilter.value
    ? entries.value.filter((e) => e.title.includes(entryFilter.value) || String(e.no).includes(entryFilter.value))
    : entries.value,
);

async function refresh() {
  status.value = await invoke<any>("book_status").catch(() => null);
  entries.value = await invoke<any[]>("book_entry_list").catch(() => []);
}

async function writeEntry() {
  if (!entryTitle.value.trim() || !entryMaterial.value.trim()) {
    tip.value = "请先填条目名和素材";
    return;
  }
  if (!props.apiKey.trim()) {
    tip.value = "请先在「设置 → 模型 API」填 API Key";
    return;
  }
  writing.value = true;
  tip.value = "";
  try {
    const r = await invoke<any>("book_write_entry", {
      apiKey: props.apiKey,
      title: entryTitle.value.trim(),
      material: entryMaterial.value.trim(),
      related: entryRelated.value.trim() || null,
      who: "ClawDesk 守书人",
    });
    tip.value = `✅ 第 ${r.no} 条《${r.title}》已出生！已更新生长日志/索引/留白。${r.question ? "反问：" + r.question : ""}`;
    entryTitle.value = "";
    entryMaterial.value = "";
    entryRelated.value = "";
    await refresh();
  } catch (e: any) {
    tip.value = `❌ ${String(e)}`;
  } finally {
    writing.value = false;
  }
}

async function answerQuestion() {
  answering.value = true;
  tip.value = "";
  try {
    const m = selectedQuestion.value.match(/第(\d+)条《([^》]+)》/);
    if (!m) {
      tip.value = "无法解析反问格式";
      return;
    }
    const no = parseInt(m[1], 10);
    const title = m[2];
    await invoke("book_answer_question", {
      apiKey: props.apiKey,
      no,
      title,
      question: selectedQuestion.value,
      masterAnswer: masterAnswer.value.trim(),
    });
    tip.value = `✅ 已把回答写回第 ${no} 条《${title}》`;
    masterAnswer.value = "";
    await refresh();
  } catch (e: any) {
    tip.value = `❌ ${String(e)}`;
  } finally {
    answering.value = false;
  }
}

async function runIngest() {
  if (!props.apiKey.trim()) {
    tip.value = "请先在「设置 → 模型 API」填 API Key";
    return;
  }
  ingesting.value = true;
  ingestResult.value = "";
  try {
    const r = await invoke<any>("book_auto_ingest", { apiKey: props.apiKey });
    ingestResult.value = `新消息 ${r.newMessages} 条 → 素材 ${r.materials} 条、回答 ${r.answers} 条、新条目 ${r.entries} 条`;
    await refresh();
  } catch (e: any) {
    ingestResult.value = `❌ ${String(e)}`;
  } finally {
    ingesting.value = false;
  }
}

async function viewEntry(e: any) {
  viewing.value = e;
  viewingBody.value = (await invoke<string | null>("book_read_entry", { no: e.no, title: e.title })) ?? "(读取失败)";
}

onMounted(refresh);
</script>

<style scoped>
.bk-overlay { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.55); z-index: 1000; display: flex; align-items: center; justify-content: center; }
.bk-panel { width: 640px; max-width: 92vw; max-height: 86vh; overflow-y: auto; background: var(--bg, #1c1c1e); border: 1px solid var(--border, #333); border-radius: 12px; padding: 20px; color: var(--fg, #eee); font-size: 13px; }
.bk-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px; }
.bk-head h3 { margin: 0; font-size: 16px; }
.bk-close { background: none; border: none; color: inherit; font-size: 16px; cursor: pointer; }
.bk-status { background: rgba(255,255,255,0.04); border-radius: 8px; padding: 10px 12px; margin-bottom: 14px; }
.bk-status-row { display: flex; gap: 8px; padding: 2px 0; }
.bk-label { color: #888; min-width: 70px; flex-shrink: 0; }
.bk-value { word-break: break-all; }
.bk-path { color: #6cf; font-size: 12px; }
.bk-growth { color: #aaa; font-size: 12px; }
.bk-locked { color: #f88; }
.bk-free { color: #8f8; }
.bk-section { border-top: 1px solid var(--border, #333); padding: 12px 0; }
.bk-section h4 { margin: 0 0 6px; font-size: 14px; }
.bk-desc { color: #999; font-size: 12px; margin: 0 0 8px; }
.bk-input, .bk-textarea { width: 100%; box-sizing: border-box; background: rgba(255,255,255,0.06); border: 1px solid var(--border, #444); border-radius: 6px; color: inherit; padding: 8px 10px; margin-bottom: 8px; font-size: 13px; }
.bk-textarea { resize: vertical; font-family: inherit; }
.bk-row { display: flex; gap: 8px; align-items: center; }
.bk-half { flex: 1; margin-bottom: 0; }
.bk-btn { background: rgba(255,255,255,0.1); border: 1px solid var(--border, #444); color: inherit; border-radius: 6px; padding: 8px 14px; cursor: pointer; font-size: 13px; white-space: nowrap; }
.bk-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.bk-primary { background: #4a6cf7; border-color: #4a6cf7; color: #fff; }
.bk-q-list { max-height: 130px; overflow-y: auto; margin-bottom: 8px; }
.bk-q-item { display: flex; gap: 6px; align-items: flex-start; padding: 3px 0; cursor: pointer; }
.bk-q-text { font-size: 12px; color: #ccc; }
.bk-browse { display: flex; flex-direction: column; gap: 6px; }
.bk-entry-list { display: flex; flex-wrap: wrap; gap: 6px; }
.bk-entry { background: rgba(255,255,255,0.07); border: none; border-radius: 5px; padding: 4px 10px; cursor: pointer; color: #9cf; font-size: 12px; }
.bk-view { border-top: 1px solid var(--border, #333); padding: 10px 0; }
.bk-pre { white-space: pre-wrap; word-break: break-word; background: rgba(0,0,0,0.3); border-radius: 6px; padding: 10px; max-height: 240px; overflow-y: auto; font-family: inherit; font-size: 12px; color: #ccc; }
.bk-ingest { margin-left: 10px; color: #8f8; font-size: 12px; }
.bk-tip { margin: 10px 0 0; color: #8f8; font-size: 12px; white-space: pre-wrap; }
</style>
