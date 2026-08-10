<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** 微信消息（与后端 WechatMessage 对应）。 */
interface WechatMsg {
  msgId: string;
  fromUser: string;
  content: string;
  msgType: string;
  timestamp: number;
  botSlot?: number;
  contextToken?: string;
}

/** 单个微信槽位状态（后端 wechat_bot_status.bots[]）。 */
interface BotStatus {
  slot: number;
  name: string;
  running: boolean;
  connected: boolean;
  botName: string;
  lastPoll: number;
  messageCount: number;
  loggedIn: boolean;
  botId: string;
  personaLen: number;
  personaText?: string;
  historyCount: number;
  proactiveEnabled?: boolean;
  proactiveIntervalMin?: number;
  proactiveIntervalMax?: number;
  proactiveLastAt?: number;
  proactiveTarget?: string;
}

const emit = defineEmits<{ close: [] }>();

// ── 账号列表状态 ──
const bots = ref<BotStatus[]>([]);
/** 当前选中槽位（localStorage 记忆，重启后恢复上次查看的微信） */
const curSlot = ref(Number(localStorage.getItem("clawdesk_wechat_slot") || 0));
const cur = computed(() => bots.value.find((b) => b.slot === curSlot.value));

// ── 当前槽位登录状态 ──
const qrcodeUrl = ref("");
const qrSvg = ref("");
const qrState = ref<"idle" | "loading" | "wait" | "scaned" | "need_verifycode" | "confirmed">("idle");
const verifyCode = ref("");
const pollTimer = ref<number | null>(null);
const statusTimer = ref<number | null>(null);
const messages = ref<WechatMsg[]>([]);
const autoReply = ref(localStorage.getItem("clawdesk_wechat_autoreply") !== "off");
const log = ref<string[]>([]);

// ── 人设编辑 ──
const personaText = ref("");
const personaSaved = ref(false);
const personaLoading = ref(false);
/** 人设编辑中标记：用户一旦手动修改（@input），5 秒轮询就不再回填后端旧值，
 *  防止「清空/输入中途被旧人设覆盖」→ 保存后才恢复同步 */
const personaDirty = ref(false);
const historyList = ref<any[]>([]);

// ── 主动聊天 ──
const proactiveEnabled = ref(false);
const proactiveIntervalMin = ref(1);
const proactiveIntervalMax = ref(180);
const proactiveTarget = ref("");
const proactiveLastAt = ref(0);

let unlistenMsg: UnlistenFn | null = null;
let unlistenStatus: UnlistenFn | null = null;

function pushLog(s: string) {
  log.value.unshift(`[${new Date().toLocaleTimeString()}] ${s}`);
  if (log.value.length > 50) log.value.pop();
}

function fmtTs(ts: number): string {
  try { return new Date(ts).toLocaleTimeString(); } catch { return ""; }
}

/** 设置主动聊天（后端 wechat_set_proactive，随机区间） */
async function saveProactive() {
  try {
    // 保证 min <= max
    if (proactiveIntervalMin.value > proactiveIntervalMax.value) {
      proactiveIntervalMax.value = proactiveIntervalMin.value;
    }
    const r = await invoke<any>("wechat_set_proactive", {
      slot: curSlot.value,
      enabled: proactiveEnabled.value,
      intervalMin: proactiveIntervalMin.value,
      intervalMax: proactiveIntervalMax.value,
      target: proactiveTarget.value.trim() || null,
    });
    proactiveEnabled.value = !!r?.enabled;
    proactiveIntervalMin.value = r?.intervalMin ?? proactiveIntervalMin.value;
    proactiveIntervalMax.value = r?.intervalMax ?? proactiveIntervalMax.value;
    proactiveTarget.value = r?.target || "";
    proactiveLastAt.value = r?.lastAt ?? 0;
    pushLog(
      proactiveEnabled.value
        ? `🎯 主动聊天已开启（随机 ${proactiveIntervalMin.value}~${proactiveIntervalMax.value} 分钟一次，仅 08:00~23:00）`
        : "⏸ 主动聊天已关闭",
    );
  } catch (e) {
    pushLog(`主动聊天设置失败: ${e}`);
  }
}

/** 从后端状态同步主动聊天配置 */
function syncProactive(b: BotStatus | undefined) {
  if (!b) return;
  proactiveEnabled.value = !!b.proactiveEnabled;
  proactiveIntervalMin.value = b.proactiveIntervalMin ?? 1;
  proactiveIntervalMax.value = b.proactiveIntervalMax ?? 180;
  proactiveTarget.value = b.proactiveTarget || "";
  proactiveLastAt.value = b.proactiveLastAt ?? 0;
}

/** 调用后端 mobile_qr_svg 把链接文本渲染成二维码 SVG（腾讯返回的 qrcode_img_content 是链接不是图片）。 */
async function genQrSvg(text: string): Promise<string> {
  if (!text) return "";
  try {
    return await invoke<string>("mobile_qr_svg", { text });
  } catch {
    return "";
  }
}

/** 切换当前选中的微信槽位 */
function selectSlot(slot: number) {
  curSlot.value = slot;
  // ★ 记住选中槽位：重启后恢复上次查看的微信
  localStorage.setItem("clawdesk_wechat_slot", String(slot));
  qrState.value = "idle";
  qrcodeUrl.value = "";
  qrSvg.value = "";
  personaText.value = "";
  personaSaved.value = false;
  personaDirty.value = false; // 切换槽位 = 放弃未保存编辑，重新进入同步状态
  historyList.value = [];
  const b = bots.value.find((x) => x.slot === slot);
  if (b?.personaText) personaText.value = b.personaText;
  syncProactive(b);
  void loadHistory(slot);
  pushLog(`已切换到 ${b?.name || `微信${slot + 1}`}`);
}

onMounted(async () => {
  await refreshStatus();
  // 恢复上次选中的槽位（localStorage 记忆）；无效则选第一个已登录的，否则 0
  const saved = curSlot.value;
  if (saved >= 0 && saved < bots.value.length) {
    selectSlot(saved);
  } else {
    const logged = bots.value.find((b) => b.loggedIn);
    selectSlot(logged ? logged.slot : 0);
  }
  statusTimer.value = window.setInterval(refreshStatus, 5000);
  try {
    unlistenMsg = await listen<WechatMsg>("wechat-message", (e) => {
      const m = e.payload;
      // 只显示当前槽位的消息
      const slot = typeof m.botSlot === "number" ? m.botSlot : 0;
      if (slot === curSlot.value) {
        messages.value.unshift(m);
        if (messages.value.length > 30) messages.value.pop();
      }
    });
    unlistenStatus = await listen<any>("wechat-bot-status", (e) => {
      const t = e.payload?.type;
      const slot = typeof e.payload?.slot === "number" ? e.payload.slot : 0;
      if (slot === curSlot.value) {
        if (t === "connected") {
          qrState.value = "confirmed";
          pushLog("✅ 已连接微信");
        } else if (t === "session_expired") {
          pushLog("⚠️ 登录已过期，请重新扫码登录");
          qrState.value = "idle";
        }
      }
      refreshStatus();
    });
  } catch (e) {
    pushLog(`事件监听失败: ${e}`);
  }
});

onUnmounted(() => {
  unlistenMsg?.();
  unlistenStatus?.();
  if (pollTimer.value) window.clearInterval(pollTimer.value);
  if (statusTimer.value) window.clearInterval(statusTimer.value);
});

function setAutoReply(v: boolean) {
  autoReply.value = v;
  localStorage.setItem("clawdesk_wechat_autoreply", v ? "on" : "off");
  pushLog(v ? "🤖 自动回复已开启（所有微信收到消息后由各自 AI 自动回复）" : "⏸ 自动回复已关闭");
}

async function refreshStatus() {
  try {
    const r = await invoke<any>("wechat_bot_status");
    bots.value = r?.bots || [];
    // ★ 同步当前槽位的后端设置（重启后自动恢复的主动聊天配置；
    //    主动消息发送后轮询也能刷新"上次主动"时间）
    const b = bots.value.find((x) => x.slot === curSlot.value);
    if (b) {
      syncProactive(b);
      // 人设回填：仅当本地为空且后端有值时（不打断正在编辑的内容）
      // ★ personaDirty：用户编辑中绝不回填，防止旧人设覆盖新输入
      if (!personaDirty.value && b.personaText && !personaText.value) personaText.value = b.personaText;
    }
  } catch { /* 静默 */ }
}

async function startQr() {
  qrState.value = "loading";
  try {
    const r = await invoke<{ qrcode: string; qrcodeUrl: string }>("wechat_get_qr", { slot: curSlot.value });
    qrcodeUrl.value = r.qrcodeUrl;
    qrSvg.value = await genQrSvg(r.qrcodeUrl);
    qrState.value = "wait";
    pushLog(`📱 微信${curSlot.value + 1} 二维码已生成，请用手机微信扫码`);
    startPoll();
  } catch (e) {
    pushLog(`获取二维码失败: ${e}`);
    qrState.value = "idle";
  }
}

async function refreshQr() {
  try {
    const r = await invoke<{ qrcode: string; qrcodeUrl: string }>("wechat_refresh_qr", { slot: curSlot.value });
    qrcodeUrl.value = r.qrcodeUrl;
    qrSvg.value = await genQrSvg(r.qrcodeUrl);
    qrState.value = "wait";
    pushLog("🔄 二维码已刷新");
    startPoll();
  } catch (e) {
    pushLog(`刷新二维码失败: ${e}`);
  }
}

function startPoll() {
  if (pollTimer.value) window.clearInterval(pollTimer.value);
  pollTimer.value = window.setInterval(pollQr, 5000);
}

async function pollQr() {
  if (qrState.value === "confirmed") return;
  try {
    const r = await invoke<any>("wechat_qr_status", { slot: curSlot.value });
    const s = r?.status;
    if (s === "confirmed") {
      qrState.value = "confirmed";
      if (pollTimer.value) window.clearInterval(pollTimer.value);
      pushLog("✅ 扫码成功，正在启动 Bot…");
      await startBot();
    } else if (s === "need_verifycode") {
      qrState.value = "need_verifycode";
      if (pollTimer.value) window.clearInterval(pollTimer.value);
      pushLog("🔢 手机微信显示配对码，请在下框输入");
    } else if (s === "scaned_but_redirect") {
      qrState.value = "scaned";
    } else if (s === "verify_code_blocked") {
      pushLog("❌ 配对码错误次数过多，请刷新二维码");
      qrState.value = "wait";
    }
  } catch { /* 网络错误继续轮询 */ }
}

async function submitVerifyCode() {
  if (!verifyCode.value.trim()) return;
  try {
    await invoke("wechat_verify_code", { code: verifyCode.value.trim(), slot: curSlot.value });
    verifyCode.value = "";
    qrState.value = "wait";
    pushLog("🔢 配对码已提交，等待确认…");
    startPoll();
  } catch (e) {
    pushLog(`提交配对码失败: ${e}`);
  }
}

async function startBot() {
  try {
    await invoke("wechat_bot_start", { config: {}, slot: curSlot.value });
    pushLog(`🚀 微信${curSlot.value + 1} Bot 已启动，长轮询接收消息中…`);
  } catch (e) {
    pushLog(`启动失败: ${e}`);
  }
  await refreshStatus();
}

async function stopBot() {
  try {
    await invoke("wechat_bot_stop", { slot: curSlot.value });
    pushLog(`⏹ 微信${curSlot.value + 1} Bot 已停止`);
  } catch (e) {
    pushLog(`停止失败: ${e}`);
  }
  await refreshStatus();
}

async function logout() {
  try {
    await invoke("wechat_logout", { slot: curSlot.value });
    pushLog(`👋 已登出微信${curSlot.value + 1}`);
    qrcodeUrl.value = "";
    qrSvg.value = "";
    qrState.value = "idle";
  } catch (e) {
    pushLog(`登出失败: ${e}`);
  }
  await refreshStatus();
}

/** 清除该微信的 AI 会话记忆（删除 wechat-{槽位} 会话，下次回复不再参考旧对话）。
 *  不影响聊天记录（history.jsonl）与人设（persona.md）。 */
async function clearMemory() {
  const sid = `wechat-${curSlot.value}`;
  try {
    const ok = await invoke<boolean>("agent_session_delete", { sessionId: sid });
    if (ok) {
      pushLog(`🧹 已清除 微信${curSlot.value + 1} 的 AI 记忆（会话 ${sid}）`);
    } else {
      pushLog(`⚠️ 该微信暂无记忆可清除（${sid} 不存在），AI 将从空白开始`);
    }
  } catch (e) {
    pushLog(`清除记忆失败: ${e}`);
  }
}

/** 保存当前微信的人设（system prompt，存 D 盘 persona.md） */
async function savePersona() {
  personaLoading.value = true;
  personaSaved.value = false;
  try {
    await invoke("wechat_set_persona", { slot: curSlot.value, persona: personaText.value });
    personaSaved.value = true;
    personaDirty.value = false; // 保存成功 = 恢复同步，轮询可继续回填
    pushLog(`✅ 微信${curSlot.value + 1} 人设已保存（${personaText.value.length} 字）`);
    setTimeout(() => { personaSaved.value = false; }, 2000);
    await refreshStatus();
  } catch (e) {
    pushLog(`人设保存失败: ${e}`);
  } finally {
    personaLoading.value = false;
  }
}

/** 读取该微信的聊天记录（D 盘 history.jsonl） */
async function loadHistory(slot: number) {
  try {
    const r = await invoke<any>("wechat_history", { slot });
    historyList.value = r?.records || [];
  } catch { historyList.value = []; }
}

async function testReply() {
  const last = messages.value[0];
  if (!last) {
    pushLog("暂无可回复的消息，请先在微信上给 Bot 发一条消息");
    return;
  }
  try {
    await invoke("wechat_bot_reply", {
      msgId: last.msgId,
      toUser: last.fromUser,
      content: "👋 测试回复成功！ClawDesk 微信 Bot 运行正常。",
      slot: curSlot.value,
    });
    pushLog("✅ 测试回复已发送");
  } catch (e) {
    pushLog(`测试回复失败: ${e}`);
  }
}
</script>

<template>
  <div class="wc-overlay">
    <div class="wc-card">
      <div class="wc-head">
        <div class="wc-title">
          <span class="wc-logo">💬</span>
          <span>微信 Bot（多账号）</span>
          <span
            class="wc-dot"
            :class="{
              green: cur?.connected,
              red: cur?.loggedIn && !cur?.connected,
              gray: !cur?.loggedIn,
            }"
          ></span>
          <span class="wc-state">
            {{
              !cur?.loggedIn
                ? "未登录"
                : cur?.connected
                  ? "已连接"
                  : cur?.running
                    ? "运行中"
                    : "已登录"
            }}
          </span>
        </div>
        <button class="wc-close" @click="emit('close')">✕</button>
      </div>

      <!-- 账号列表（微信1 ~ 微信10，每个独立登录/人设/会话） -->
      <div class="wc-slots">
        <button
          v-for="b in bots"
          :key="b.slot"
          class="wc-slot"
          :class="{ active: curSlot === b.slot }"
          @click="selectSlot(b.slot)"
        >
          <span class="wc-slot-dot" :class="b.connected ? 'on' : b.loggedIn ? 'idle' : ''"></span>
          {{ b.name }}
          <span v-if="b.personaLen > 0" class="wc-slot-persona" title="已设置人设">🧬</span>
        </button>
      </div>

      <div class="wc-body">
        <!-- 左：登录 / 控制 / 人设 -->
        <div class="wc-left">
          <!-- 状态信息 -->
          <div class="wc-info">
            <div class="wc-row"><span>Bot ID</span><b>{{ cur?.botId || "—" }}</b></div>
            <div class="wc-row"><span>消息数</span><b>{{ cur?.messageCount ?? 0 }}</b></div>
            <div class="wc-row"><span>聊天记录</span><b>{{ cur?.historyCount ?? 0 }} 条（D 盘）</b></div>
            <div class="wc-row"><span>自动回复</span><b>
              <label class="wc-switch">
                <input type="checkbox" :checked="autoReply" @change="(e: any) => setAutoReply((e.target as HTMLInputElement).checked)" />
                <span class="wc-knob"></span>
              </label>
            </b></div>
          </div>

          <!-- 扫码区 -->
          <div v-if="!cur?.loggedIn" class="wc-qr">
            <template v-if="qrState === 'idle' || qrState === 'loading'">
              <div class="wc-qr-placeholder">
                <span v-if="qrState === 'loading'">获取二维码中…</span>
                <span v-else>用手机微信扫码登录（微信{{ curSlot + 1 }}）</span>
              </div>
              <button class="wc-btn wc-primary" @click="startQr" :disabled="qrState === 'loading'">获取登录二维码</button>
            </template>
            <template v-else>
              <div v-if="qrSvg" class="wc-qr-svg" v-html="qrSvg"></div>
              <img v-else-if="qrcodeUrl" class="wc-qr-img" :src="qrcodeUrl" alt="微信登录二维码" />
              <div class="wc-qr-hint">
                <template v-if="qrState === 'wait'">⏳ 等待扫码…</template>
                <template v-else-if="qrState === 'scaned'">📲 已扫码，等待确认…</template>
                <template v-else-if="qrState === 'confirmed'">✅ 登录成功</template>
              </div>
              <div v-if="qrState === 'need_verifycode'" class="wc-verify">
                <input
                  v-model="verifyCode"
                  class="wc-input"
                  placeholder="输入手机微信显示的配对码"
                  @keydown.enter="submitVerifyCode"
                />
                <button class="wc-btn wc-primary" @click="submitVerifyCode">提交</button>
              </div>
              <div class="wc-qr-ops">
                <button class="wc-btn" @click="refreshQr">刷新二维码</button>
              </div>
            </template>
          </div>

          <!-- 已登录控制 -->
          <div v-else class="wc-ctrl">
            <div class="wc-ctrl-btns">
              <button v-if="!cur?.running" class="wc-btn wc-primary" @click="startBot">▶ 启动 Bot</button>
              <button v-else class="wc-btn" @click="stopBot">⏸ 停止 Bot</button>
              <button class="wc-btn" @click="testReply">📨 测试回复</button>
              <button class="wc-btn wc-danger" @click="logout">登出</button>
            </div>
            <p class="wc-tip">
              {{ cur?.running ? "Bot 正在长轮询接收微信消息，收到后由该微信的 AI 自动回复。" : "Bot 未运行。点「启动 Bot」恢复接收。" }}
            </p>
          </div>

          <!-- 主动聊天（Bot 主动找用户聊天）★ 优先显示 -->
          <div class="wc-persona">
            <div class="wc-log-title">🎯 主动聊天（Bot 主动找用户）</div>
            <div class="wc-info" style="margin: 0 10px;">
              <div class="wc-row"><span>主动聊天</span><b>
                <label class="wc-switch">
                  <input type="checkbox" :checked="proactiveEnabled" @change="(e: any) => { proactiveEnabled = (e.target as HTMLInputElement).checked; saveProactive(); }" />
                  <span class="wc-knob"></span>
                </label>
              </b></div>
              <div class="wc-row"><span>随机间隔</span><b>
                <input
                  :value="proactiveIntervalMin"
                  type="number"
                  min="1"
                  max="1440"
                  class="wc-num-input"
                  title="最短间隔（分钟）"
                  @change="(e: any) => { proactiveIntervalMin = Number((e.target as HTMLInputElement).value) || 1; saveProactive(); }"
                />
                <span style="color:#94a3b8;">~</span>
                <input
                  :value="proactiveIntervalMax"
                  type="number"
                  min="1"
                  max="1440"
                  class="wc-num-input"
                  title="最长间隔（分钟）"
                  @change="(e: any) => { proactiveIntervalMax = Number((e.target as HTMLInputElement).value) || 180; saveProactive(); }"
                />
                <span style="font-size:11px; color:#64748b;">分钟</span>
              </b></div>
              <div class="wc-row"><span>目标用户</span><b style="max-width:55%; overflow:hidden; text-overflow:ellipsis;">{{ proactiveTarget || "自动（最近聊过的人）" }}</b></div>
              <div class="wc-row"><span>上次主动</span><b>{{ proactiveLastAt ? fmtTs(proactiveLastAt) : "—" }}</b></div>
            </div>
            <p class="wc-tip" style="margin: 0 10px 10px; color: #f59e0b;">
              🎲 每次发送后随机等 最短~最长 分钟（真随机，不固定）；⏰ 仅 08:00~23:00 主动找（晚上11点后不打扰）；用户深夜发消息时 AI 会带"这么晚找我"的关心语气。
            </p>
          </div>

          <!-- 人设编辑（每个微信独立） -->
          <div class="wc-persona">
            <div class="wc-log-title">🧬 人设（该微信 AI 的角色设定，保存到 D 盘）</div>
            <textarea
              v-model="personaText"
              class="wc-persona-input"
              rows="3"
              placeholder="粘贴人设文本（如小肥鱼人设.md）… 留空并保存 = 清除人设"
              @input="personaDirty = true"
            ></textarea>
            <button class="wc-btn wc-primary" @click="savePersona" :disabled="personaLoading">
              {{ personaLoading ? "保存中…" : personaSaved ? "✅ 已保存" : "保存人设" }}
            </button>
            <button class="wc-btn wc-danger" style="margin-left:8px;" @click="clearMemory" title="删除该微信的 AI 会话记忆（wechat-{槽位}），不影响聊天记录与人设">
              🧹 清除 AI 记忆
            </button>
          </div>

          <!-- 日志 -->
          <div class="wc-log">
            <div class="wc-log-title">运行日志</div>
            <div class="wc-log-body">
              <p v-for="(l, i) in log" :key="i" class="wc-log-line">{{ l }}</p>
              <p v-if="!log.length" class="wc-log-empty">暂无日志</p>
            </div>
          </div>
        </div>

        <!-- 右：最近消息 + 聊天记录 -->
        <div class="wc-right">
          <div class="wc-log-title">最近消息（{{ messages.length }}）</div>
          <div class="wc-msgs">
            <div v-for="m in messages" :key="m.msgId + m.timestamp" class="wc-msg">
              <div class="wc-msg-meta">
                <span class="wc-msg-from">{{ m.fromUser.slice(0, 8) }}</span>
                <span class="wc-msg-time">{{ fmtTs(m.timestamp) }}</span>
              </div>
              <div class="wc-msg-content">{{ m.content }}</div>
            </div>
            <p v-if="!messages.length" class="wc-log-empty">暂无消息</p>
          </div>
          <div class="wc-log-title" style="border-top: 1px solid #2a3752;">📜 D 盘聊天记录（{{ historyList.length }} 条）</div>
          <div class="wc-msgs">
            <div v-for="(h, i) in historyList.slice(-20).reverse()" :key="i" class="wc-msg">
              <div class="wc-msg-meta">
                <span class="wc-msg-from">{{ h.fromUser === h.toUser ? h.fromUser.slice(0, 8) : "我" }}</span>
                <span class="wc-msg-time">{{ fmtTs(h.timestamp) }} · {{ h.msgType }}</span>
              </div>
              <div class="wc-msg-content">{{ String(h.content || "").slice(0, 120) }}</div>
            </div>
            <p v-if="!historyList.length" class="wc-log-empty">暂无聊天记录</p>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.wc-overlay {
  position: fixed;
  inset: 0;
  z-index: 300;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  backdrop-filter: blur(4px);
}
.wc-card {
  width: min(880px, 92vw);
  height: min(600px, 86vh);
  background: linear-gradient(180deg, #1b2233 0%, #141a28 100%);
  border: 1px solid #2c3a55;
  border-radius: 14px;
  display: flex;
  flex-direction: column;
  box-shadow: 0 18px 60px rgba(0, 0, 0, 0.55);
  overflow: hidden;
}
.wc-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid #26324a;
  flex-shrink: 0;
}
.wc-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 15px;
  font-weight: 600;
  color: #e8edf7;
}
.wc-logo { font-size: 16px; }
.wc-dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  display: inline-block;
}
.wc-dot.green { background: #34d399; box-shadow: 0 0 6px #34d399; }
.wc-dot.red { background: #f87171; box-shadow: 0 0 6px #f87171; }
.wc-dot.gray { background: #64748b; }
.wc-state {
  font-size: 12px;
  color: #94a3b8;
  font-weight: 400;
}
.wc-close {
  background: none;
  border: none;
  color: #94a3b8;
  font-size: 15px;
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 6px;
}
.wc-close:hover { background: #26324a; color: #fff; }
.wc-body {
  display: flex;
  flex: 1;
  min-height: 0;
}
.wc-left {
  width: 46%;
  border-right: 1px solid #26324a;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  overflow-y: auto;
  /* ★ 修复：除日志外的子面板不允许被 flex 压缩（否则窗口矮时
     人设/按钮/日志被裁剪且不出现滚动条，底部设置"消失"）。
     内容超高时由 .wc-left 滚动，保证所有设置都能看到。 */
}
.wc-left > *:not(.wc-log) {
  flex-shrink: 0;
}
.wc-right {
  flex: 1;
  padding: 16px;
  display: flex;
  flex-direction: column;
  min-width: 0;
}
.wc-info {
  background: #1e2739;
  border: 1px solid #2a3752;
  border-radius: 10px;
  padding: 10px 14px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.wc-row {
  display: flex;
  justify-content: space-between;
  font-size: 12.5px;
  color: #94a3b8;
}
.wc-row b { color: #e8edf7; font-weight: 500; }
.wc-switch { position: relative; display: inline-block; width: 34px; height: 19px; vertical-align: middle; }
.wc-switch input { opacity: 0; width: 0; height: 0; }
.wc-knob {
  position: absolute;
  inset: 0;
  background: #334155;
  border-radius: 19px;
  transition: 0.2s;
  cursor: pointer;
}
.wc-knob::before {
  content: "";
  position: absolute;
  width: 13px;
  height: 13px;
  left: 3px;
  top: 3px;
  background: #cbd5e1;
  border-radius: 50%;
  transition: 0.2s;
}
.wc-switch input:checked + .wc-knob { background: #34d399; }
.wc-switch input:checked + .wc-knob::before { transform: translateX(15px); background: #fff; }

.wc-qr {
  background: #1e2739;
  border: 1px solid #2a3752;
  border-radius: 10px;
  padding: 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.wc-qr-placeholder {
  height: 180px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #64748b;
  font-size: 13px;
  width: 100%;
  border: 1px dashed #334155;
  border-radius: 8px;
}
.wc-qr-img {
  width: 180px;
  height: 180px;
  object-fit: contain;
  border-radius: 8px;
  background: #fff;
  padding: 6px;
}
.wc-qr-svg {
  width: 180px;
  height: 180px;
  background: #fff;
  border-radius: 8px;
  padding: 6px;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}
.wc-qr-svg :deep(svg) {
  width: 100%;
  height: 100%;
  display: block;
}
.wc-qr-hint { font-size: 12.5px; color: #94a3b8; }
.wc-qr-ops { display: flex; gap: 8px; }
.wc-verify { display: flex; gap: 8px; width: 100%; }
.wc-input {
  flex: 1;
  background: #141a28;
  border: 1px solid #2c3a55;
  border-radius: 8px;
  color: #e8edf7;
  padding: 8px 10px;
  font-size: 13px;
  outline: none;
}
.wc-input:focus { border-color: #3b82f6; }

.wc-btn {
  background: #26324a;
  color: #dbe4f0;
  border: 1px solid #33415e;
  border-radius: 8px;
  padding: 7px 14px;
  font-size: 13px;
  cursor: pointer;
  transition: 0.15s;
}
.wc-btn:hover { background: #2f3d59; }
.wc-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.wc-primary { background: #2563eb; border-color: #2563eb; color: #fff; }
.wc-primary:hover { background: #1d4ed8; }
.wc-danger { background: #7f1d1d; border-color: #991b1b; color: #fecaca; }
.wc-danger:hover { background: #991b1b; }
.wc-ctrl { display: flex; flex-direction: column; gap: 10px; }
.wc-ctrl-btns { display: flex; flex-wrap: wrap; gap: 8px; }
.wc-tip { font-size: 12px; color: #64748b; margin: 0; }

.wc-log {
  flex: 1;
  min-height: 90px;
  display: flex;
  flex-direction: column;
  background: #1e2739;
  border: 1px solid #2a3752;
  border-radius: 10px;
  overflow: hidden;
}
.wc-log-title {
  padding: 8px 12px;
  font-size: 12px;
  color: #94a3b8;
  border-bottom: 1px solid #2a3752;
  flex-shrink: 0;
}
.wc-log-body {
  flex: 1;
  overflow-y: auto;
  padding: 8px 12px;
  font-size: 12px;
}
.wc-log-line {
  margin: 2px 0;
  color: #a5b4cb;
  word-break: break-all;
  font-family: Consolas, monospace;
}
.wc-log-empty { color: #4b5563; font-size: 12px; }

.wc-msgs {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-top: 8px;
}
.wc-msg {
  background: #1e2739;
  border: 1px solid #2a3752;
  border-radius: 9px;
  padding: 8px 12px;
}
.wc-msg-meta {
  display: flex;
  justify-content: space-between;
  font-size: 11px;
  color: #64748b;
  margin-bottom: 3px;
}
.wc-msg-from { color: #38bdf8; }
.wc-msg-content {
  font-size: 13px;
  color: #e8edf7;
  word-break: break-all;
  white-space: pre-wrap;
}

/* ── 多账号：槽位列表 ── */
.wc-slots {
  display: flex;
  gap: 6px;
  padding: 10px 16px 0;
  flex-wrap: wrap;
  flex-shrink: 0;
}
.wc-slot {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: #1e2739;
  border: 1px solid #2a3752;
  color: #a5b4cb;
  border-radius: 20px;
  padding: 5px 12px;
  font-size: 12.5px;
  cursor: pointer;
  transition: 0.15s;
}
.wc-slot:hover { background: #26324a; }
.wc-slot.active { background: #2563eb; border-color: #2563eb; color: #fff; }
.wc-slot-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #64748b;
}
.wc-slot-dot.idle { background: #f59e0b; }
.wc-slot-dot.on { background: #34d399; box-shadow: 0 0 5px #34d399; }
.wc-slot-persona { font-size: 11px; }

/* ── 人设编辑区 ── */
.wc-persona {
  display: flex;
  flex-direction: column;
  gap: 8px;
  background: #1e2739;
  border: 1px solid #2a3752;
  border-radius: 10px;
  overflow: hidden;
}
.wc-persona-input {
  width: 100%;
  background: #141a28;
  border: 1px solid #2c3a55;
  border-radius: 8px;
  color: #e8edf7;
  padding: 8px 10px;
  font-size: 12.5px;
  outline: none;
  resize: vertical;
  min-height: 80px;
  font-family: inherit;
  margin: 0 10px;
  width: calc(100% - 20px);
  flex-shrink: 0; /* ★ 防止在 .wc-persona 内被压缩成一行 */
}
.wc-persona-input:focus { border-color: #3b82f6; }
.wc-persona .wc-btn { margin: 0 10px 10px; }

/* 主动聊天数字输入 */
.wc-num-input {
  width: 64px;
  background: #141a28;
  border: 1px solid #2c3a55;
  border-radius: 6px;
  color: #e8edf7;
  padding: 3px 6px;
  font-size: 12px;
  text-align: right;
  outline: none;
}
.wc-num-input:focus { border-color: #3b82f6; }
</style>
