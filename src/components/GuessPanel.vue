<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** 一轮游戏：AI 提问/猜测 + 用户回答 */
interface GTurn {
  round: number;
  thinking: string; // 思考链（流式累积，deepseek-reasoner 真实输出）
  text: string; // 问题/猜测文本（流式累积）
  answer?: string; // 用户回答
  hint?: string; // 用户补充提示
  isGuess: boolean; // 是否猜测
  elapsed: number; // 思考耗时（秒）
}

const props = defineProps<{ apiKey: string; baseUrl: string }>();
const emit = defineEmits<{ close: [] }>();

const expert = ref(false);
const phase = ref<"idle" | "playing" | "done">("idle");
const busy = ref(false);
const guessed = ref(false);
const gameId = ref("");
const rounds = ref<GTurn[]>([]);
const hint = ref("");

let unlisten: UnlistenFn | null = null;
let curRound = -1;
let thinkingStart = 0;

function newTurn(): GTurn {
  curRound = rounds.value.length;
  thinkingStart = Date.now();
  return { round: rounds.value.length + 1, thinking: "", text: "", isGuess: false, elapsed: 0 };
}

function pushError(msg: string) {
  const t = rounds.value[curRound >= 0 ? curRound : 0];
  if (t && !t.text) t.text = "⚠ " + msg;
  else rounds.value.push({ round: rounds.value.length + 1, thinking: "", text: "⚠ " + msg, isGuess: false, elapsed: 0 });
  busy.value = false;
}

/** 开始一局 */
async function start() {
  phase.value = "playing";
  busy.value = true;
  guessed.value = false;
  rounds.value = [];
  hint.value = "";
  rounds.value.push(newTurn());
  try {
    gameId.value = await invoke<string>("guess_start", {
      apiKey: props.apiKey,
      baseUrl: props.baseUrl,
      expert: expert.value,
    });
  } catch (e) {
    pushError(String(e));
  }
}

/** 用户回答（可附补充提示） */
async function answer(a: string) {
  if (busy.value || !gameId.value) return;
  const t = rounds.value[curRound];
  if (t) {
    t.answer = a;
    t.hint = hint.value.trim();
  }
  hint.value = "";
  guessed.value = false;
  rounds.value.push(newTurn());
  busy.value = true;
  try {
    await invoke("guess_reply", { gameId: gameId.value, answer: a, hint: t?.hint || null });
  } catch (e) {
    pushError(String(e));
  }
}

function onWin() {
  phase.value = "done";
  busy.value = false;
}

function restart() {
  if (gameId.value) void invoke("guess_stop", { gameId: gameId.value }).catch(() => {});
  void start();
}

function fmtAnswer(a?: string): string {
  return a || "";
}

onMounted(async () => {
  try {
    unlisten = await listen<Record<string, unknown>>("guess://progress", (e) => {
      const p = e.payload;
      const type = String(p.type ?? "");
      if (type === "thinking_delta") {
        const t = rounds.value[curRound];
        if (t) t.thinking += String(p.content ?? "");
      } else if (type === "text_delta") {
        const t = rounds.value[curRound];
        if (t) t.text += String(p.content ?? "");
      } else if (type === "done") {
        const t = rounds.value[curRound];
        if (t) {
          t.elapsed = Math.max(1, Math.round((Date.now() - thinkingStart) / 1000));
          t.isGuess = /我猜是/.test(t.text);
          guessed.value = t.isGuess;
        }
        busy.value = false;
      } else if (type === "error") {
        pushError(String(p.message ?? "未知错误"));
      }
    });
  } catch {
    /* 事件监听失败不阻塞 */
  }
});

onUnmounted(() => {
  unlisten?.();
  if (gameId.value) void invoke("guess_stop", { gameId: gameId.value }).catch(() => {});
});
</script>

<template>
  <div class="gp-overlay" @click.self="emit('close')">
    <div class="gp-card">
      <div class="gp-header">
        <div class="gp-title">
          🎯 猜人物
          <span class="gp-sub">AI 提问猜出你心中的人物 · 全程展示思考</span>
        </div>
        <div class="gp-right">
          <label class="gp-expert">
            <input type="checkbox" v-model="expert" :disabled="phase !== 'idle'" />
            <span>专家模式</span>
          </label>
          <span v-if="phase !== 'idle'" class="gp-round">第 {{ rounds.length }} 问</span>
          <button class="gp-close" title="关闭" @click="emit('close')">✕</button>
        </div>
      </div>

      <div class="gp-body">
        <!-- 未开始：规则说明 -->
        <div v-if="phase === 'idle'" class="gp-intro">
          <div class="gp-intro-emoji">🧠</div>
          <h3>规则很简单</h3>
          <ul>
            <li><b>1.</b> 在脑海里想一个「人物」——真实历史人物、动漫/游戏/影视角色、文学角色……都可以</li>
            <li><b>2.</b> AI 会通过「是/否」类问题一步步缩小范围</li>
            <li><b>3.</b> 你只需诚实回答：是 / 否 / 不确定 / 接近了</li>
            <li><b>4.</b> AI 有把握时就会猜，看它多少问能猜中</li>
          </ul>
          <p class="gp-tip">💡 提示：想得越冷门，AI 猜得越久（也越有趣）。AI 的每一步思考都会实时展示出来。</p>
          <button class="gp-start" @click="start">开始游戏</button>
        </div>

        <!-- 游戏中 -->
        <div v-else class="gp-chat">
          <div v-for="(t, i) in rounds" :key="i" class="gp-turn">
            <!-- AI 轮：思考链 + 提问/猜测 -->
            <div class="gp-ai">
              <div v-if="t.thinking || (busy && i === rounds.length - 1)" class="gp-think">
                <div class="gp-think-head">
                  <span class="gp-think-dot"></span>
                  已思考<span v-if="t.elapsed">（用时 {{ t.elapsed }} 秒）</span>
                  <span v-if="i === rounds.length - 1 && busy && !t.elapsed" class="gp-think-live">思考中…</span>
                </div>
                <div class="gp-think-body">{{ t.thinking || "…" }}</div>
              </div>
              <div class="gp-bubble" :class="{ 'gp-guess': t.isGuess }">
                <span class="gp-q-tag" :class="{ 'gp-q-guess': t.isGuess }">{{ t.isGuess ? "🎯 猜测" : "问" }}</span>
                {{ t.text || (busy && i === rounds.length - 1 ? "…" : "") }}
              </div>
            </div>
            <!-- 用户回答 -->
            <div v-if="t.answer" class="gp-user">
              <span class="gp-user-tag">你</span>
              {{ fmtAnswer(t.answer) }}<span v-if="t.hint" class="gp-hint">（{{ t.hint }}）</span>
            </div>
          </div>

          <div v-if="busy" class="gp-typing">AI 正在思考…</div>

          <!-- 回答按钮 -->
          <div v-if="!busy && phase === 'playing'" class="gp-answers">
            <template v-if="!guessed">
              <button class="gp-a yes" @click="answer('是')">✅ 是</button>
              <button class="gp-a no" @click="answer('否')">❌ 否</button>
              <button class="gp-a unk" @click="answer('不确定')">🤔 不确定</button>
              <button class="gp-a near" @click="answer('接近了')">🔥 接近了</button>
            </template>
            <template v-else>
              <button class="gp-a win" @click="onWin">🎉 猜对了！</button>
              <button class="gp-a lose" @click="answer('不对，你猜错了，请根据我之前的回答继续缩小范围')">✗ 没猜对，继续</button>
            </template>
          </div>

          <!-- 补充提示（可选） -->
          <div v-if="!busy && phase === 'playing' && !guessed" class="gp-hint-row">
            <input
              v-model="hint"
              class="gp-hint-input"
              placeholder="补充提示（可选）：如「TA 是女的」「来自日本动漫」，会附在下一次回答里"
            />
            <button v-if="hint" class="gp-hint-clear" @click="hint = ''">清空</button>
          </div>

          <!-- 结束 -->
          <div v-if="phase === 'done'" class="gp-done">
            <div class="gp-done-emoji">🏆</div>
            <h3>猜对了！</h3>
            <p>AI 用了 {{ rounds.length }} 轮猜出你心中的人物。</p>
            <button class="gp-start" @click="restart">再来一局</button>
          </div>

          <div class="gp-restart-row">
            <button v-if="phase === 'playing' || phase === 'done'" class="gp-restart" @click="restart">🔄 重新开始</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
