<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { sessionsApi, wechatApi } from "../utils/api";

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

/** 微信 Bot 状态（后端 wechat_bot_status.bot）。 */
interface BotStatus {
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
  allowedUsers?: string[];
  voiceId?: string;
  voiceEngine?: string;
  cosyvoiceApiKey?: string;
  indexttsUrl?: string;
  indexttsVoicePath?: string;
}

const emit = defineEmits<{ close: [] }>();

/** 唯一的微信 Bot 状态。 */
const bot = ref<BotStatus | null>(null);
const cur = computed(() => bot.value);

// ── 登录状态 ──
const qrcodeUrl = ref("");
const qrSvg = ref("");
const qrState = ref<"idle" | "loading" | "wait" | "scaned" | "need_verifycode" | "confirmed">("idle");
const verifyCode = ref("");
const pollTimer = ref<number | null>(null);
const statusTimer = ref<number | null>(null);
/** 内置聊天界面：聊天记录轮询同步 */
const chatTimer = ref<number | null>(null);
const messages = ref<WechatMsg[]>([]);
const autoReply = ref(localStorage.getItem("clawdesk_wechat_autoreply") !== "off");
const log = ref<string[]>([]);
/** AI 生活状态（世界线：此刻在做什么，时间与真实时钟同步） */
const livingState = ref("");
const moodState = ref<string>("");
/** 灵魂全景快照（八层状态，供"灵魂面板"展示） */
const soulSnap = ref<any>(null);
const soulOpen = ref(false);

// ── 人设编辑 ──
const personaText = ref("");
const personaSaved = ref(false);
const personaLoading = ref(false);
const memoryClearing = ref(false);
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

// ── 内置微信聊天界面（中栏：会话列表 + 聊天窗 + 输入框）──
/** 原始聊天记录缓存（wechat_history 返回） */
const chatCache = ref<any[]>([]);
/** 会话列表（按联系人分组，按最后消息时间倒序） */
const chats = ref<{ dir: string; last: string; lastTime: number; lastType: string }[]>([]);
/** 当前打开的会话联系人 */
const activeChat = ref("");
/** 当前会话消息（按时间正序） */
const chatMsgs = ref<{ fromBot: boolean; content: string; msgType: string; timestamp: number }[]>([]);
const chatInput = ref("");
const chatSending = ref(false);
const chatTip = ref("");

/** 从后端加载聊天记录，重建会话列表 + 当前会话消息。 */
async function reloadChats(): Promise<void> {
  try {
    const r = await wechatApi.history();
    const records: any[] = r?.records ?? [];
    chatCache.value = records;
    // 按联系人 dir 分组（dir = 对方用户 ID；AI 发出的消息 toUser 同为 dir）
    const map = new Map<string, { last: string; lastTime: number; lastType: string }>();
    for (const rec of records) {
      const dir = rec.dir ?? rec.fromUser ?? "";
      if (!dir) continue;
      const t = Number(rec.timestamp ?? 0);
      const cur = map.get(dir);
      if (!cur || t >= cur.lastTime) {
        map.set(dir, {
          last: rec.content ?? "",
          lastTime: t,
          lastType: rec.msgType ?? "text",
        });
      }
    }
    chats.value = Array.from(map.entries())
      .map(([dir, v]) => ({ dir, ...v }))
      .sort((a, b) => b.lastTime - a.lastTime);
    // 当前会话若已被删除则清空
    if (activeChat.value && !chats.value.some((c) => c.dir === activeChat.value)) {
      activeChat.value = "";
    }
    if (activeChat.value) rebuildChatMsgs();
    chatTip.value = `共 ${chats.value.length} 个会话 · ${records.length} 条消息`;
  } catch { /* 静默 */ }
}

/** 重建当前会话的消息气泡（时间正序，AI 发送=右侧/me，对方=左侧/them）。 */
function rebuildChatMsgs(): void {
  chatMsgs.value = chatCache.value
    .filter((rec) => (rec.dir ?? rec.fromUser ?? "") === activeChat.value)
    .sort((a, b) => Number(a.timestamp ?? 0) - Number(b.timestamp ?? 0))
    .map((rec) => ({
      fromBot: !!rec.fromBot,
      content: rec.content ?? "",
      msgType: rec.msgType ?? "text",
      timestamp: Number(rec.timestamp ?? 0),
    }));
}

/** 打开一个会话。 */
function openChat(dir: string): void {
  activeChat.value = dir;
  rebuildChatMsgs();
}

/** 以该微信身份发送消息（走 iLink 官方协议，与 Bot 共用会话）。 */
async function sendChat(): Promise<void> {
  const text = chatInput.value.trim();
  if (!text || chatSending.value) return;
  if (!activeChat.value) return;
  chatSending.value = true;
  try {
    await wechatApi.sendMessage({
      toUser: activeChat.value,
      content: text,
    });
    chatInput.value = "";
    // 本地立即追加（后端 append_history 已落盘，reloadChats 会再同步一次）
    chatMsgs.value.push({ fromBot: true, content: text, msgType: "text", timestamp: Date.now() });
    pushLog(`📤 已发送到 ${activeChat.value}：${text.slice(0, 40)}`);
    void reloadChats();
  } catch (e) {
    pushLog(`发送失败: ${e}`);
  } finally {
    chatSending.value = false;
  }
}

// ── 使用规则（白名单 / 语音音色）──
const allowedUsers = ref("");
const voiceId = ref("zh-CN-XiaoxiaoNeural");
const voiceEngine = ref("edge");
const cosyvoiceKey = ref("");
const indexttsUrl = ref("http://127.0.0.1:8000");
const indexttsVoicePath = ref("");
/** 规则编辑中标记：用户一旦手动修改（@change），5 秒轮询就不再回填后端旧值，
 *  防止「下拉选了又被弹回」→ 保存后才恢复同步 */
const voiceDirty = ref(false);
const voiceReply = ref(localStorage.getItem("clawdesk_wechat_voicereply") === "on");
/** CosyVoice 2 真人级音色（硅基流动预置） */
const cosyvoiceOptions = [
  { id: "anna", name: "Anna · 女声 温暖自然（推荐）" },
  { id: "bella", name: "Bella · 女声 甜美" },
  { id: "lily", name: "Lily · 女声 清新" },
  { id: "maria", name: "Maria · 女声 优雅" },
  { id: "sarah", name: "Sarah · 女声 亲切" },
  { id: "alex", name: "Alex · 男声 沉稳磁性" },
  { id: "eric", name: "Eric · 男声 阳光" },
  { id: "jason", name: "Jason · 男声 成熟" },
  { id: "roger", name: "Roger · 男声 浑厚" },
  { id: "steve", name: "Steve · 男声 低沉" },
];
/** 内置音色（与后端 tts_list_voices 前几名对齐，下拉选择用） */
const voiceOptions = [
  { id: "zh-CN-XiaoxiaoNeural", name: "晓晓 · 温暖亲切（默认）" },
  { id: "zh-CN-XiaoyiNeural", name: "晓伊 · 活泼少女" },
  { id: "zh-CN-YunxiNeural", name: "云希 · 阳光少年" },
  { id: "zh-CN-YunjianNeural", name: "云健 · 沉稳成熟" },
  { id: "zh-CN-XiaochenNeural", name: "晓辰 · 清澈邻家女孩" },
  { id: "zh-CN-XiaohanNeural", name: "晓涵 · 甜美温柔" },
  { id: "zh-CN-XiaomengNeural", name: "晓梦 · 亲切自然" },
  { id: "zh-CN-XiaomoNeural", name: "晓墨 · 成熟知性" },
  { id: "zh-CN-XiaoruiNeural", name: "晓睿 · 温柔睿智" },
  { id: "en-US-EmmaMultilingualNeural", name: "Emma · 多语言女声" },
];

let unlistenMsg: UnlistenFn | null = null;
let unlistenStatus: UnlistenFn | null = null;

function pushLog(s: string) {
  log.value.unshift(`[${new Date().toLocaleTimeString()}] ${s}`);
  if (log.value.length > 50) log.value.pop();
}

function fmtTs(ts: number): string {
  try { return new Date(ts).toLocaleTimeString(); } catch { return ""; }
}

/** ghost 上次「没回」距离现在多久（用于灵魂面板展示） */
function fmtGhostAgo(ms: number): string {
  if (!ms) return "";
  const diff = Date.now() - ms;
  if (diff < 0) return "";
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return min + " 分钟前";
  const h = Math.floor(min / 60);
  if (h < 24) return h + " 小时前";
  return Math.floor(h / 24) + " 天前";
}

/** 消息类型图标（voice=语音转写 / image=图片 / file=文件 / 其余文本） */
function typeIcon(t: string): string {
  switch (t) {
    case "voice": return "🔊";
    case "image": return "🖼️";
    case "file": return "📎";
    default: return "";
  }
}

/** 设置主动聊天（后端 wechat_set_proactive，随机区间） */
async function saveProactive() {
  try {
    // 保证 min <= max
    if (proactiveIntervalMin.value > proactiveIntervalMax.value) {
      proactiveIntervalMax.value = proactiveIntervalMin.value;
    }
    const r = await wechatApi.setProactive({
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
function syncProactive(b: BotStatus | null | undefined) {
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
    return await wechatApi.mobileQrSvg(text);
  } catch {
    return "";
  }
}

/** 初始化微信面板状态。 */
function initCurrentSlot() {
  qrState.value = "idle";
  qrcodeUrl.value = "";
  qrSvg.value = "";
  personaText.value = "";
  personaSaved.value = false;
  personaDirty.value = false;
  voiceDirty.value = false;
  historyList.value = [];
  messages.value = [];
  const b = bot.value;
  if (b?.personaText) personaText.value = b.personaText;
  syncProactive(b);
  void loadHistory();
  reloadChats();
}

onMounted(async () => {
  await refreshStatus();
  initCurrentSlot();
  statusTimer.value = window.setInterval(refreshStatus, 5000);
  // 内置聊天界面：每 5 秒同步一次聊天记录（新消息自动出现）
  chatTimer.value = window.setInterval(() => void reloadChats(), 5000);
  try {
    unlistenMsg = await listen<WechatMsg>("wechat-message", (e) => {
      const m = e.payload;
      messages.value.unshift(m);
      if (messages.value.length > 30) messages.value.pop();
      void reloadChats();
    });
    unlistenStatus = await listen<any>("wechat-bot-status", (e) => {
      const t = e.payload?.type;
      if (t === "connected") {
        qrState.value = "confirmed";
        pushLog("✅ 已连接微信");
      } else if (t === "session_expired") {
        pushLog("⚠️ 登录已过期，请重新扫码登录");
        qrState.value = "idle";
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
  if (pollTimer.value) window.clearTimeout(pollTimer.value);
  if (statusTimer.value) window.clearInterval(statusTimer.value);
  if (chatTimer.value) window.clearInterval(chatTimer.value);
});

function setVoiceReply(v: boolean) {
  voiceReply.value = v;
  localStorage.setItem("clawdesk_wechat_voicereply", v ? "on" : "off");
  pushLog(v ? "🎙 AI 语音回复已开启（文字回复后自动发一条真人音色语音）" : "🔇 AI 语音回复已关闭");
}

/** 保存使用规则（白名单 + 音色 + 引擎，后端持久化） */
async function saveRules() {
  try {
    const r = await wechatApi.setBotRules({
      allowedUsers: allowedUsers.value.trim() || null,
      voiceId: voiceId.value.trim() || null,
      voiceEngine: voiceEngine.value,
      cosyvoiceApiKey: cosyvoiceKey.value.trim() || null,
      indexttsUrl: indexttsUrl.value.trim() || null,
      indexttsVoicePath: indexttsVoicePath.value.trim() || null,
    });
    allowedUsers.value = (r?.allowedUsers || []).join("，");
    voiceId.value = r?.voiceId || "zh-CN-XiaoxiaoNeural";
    voiceEngine.value = r?.voiceEngine || "edge";
    cosyvoiceKey.value = r?.cosyvoiceApiKey || "";
    indexttsUrl.value = r?.indexttsUrl || "http://127.0.0.1:8000";
    indexttsVoicePath.value = r?.indexttsVoicePath || "";
    // 同步到 localStorage：语音回复发送时后端优先用已保存的 voice_id，
    // 此处兜底保证旧路径也能拿到正确音色
    if (voiceId.value) localStorage.setItem("clawdesk_wechat_voice", voiceId.value);
    voiceDirty.value = false; // 保存成功 → 恢复与后端同步
    pushLog(
      r?.allowedUsers?.length
        ? `🔒 白名单已保存：只与 ${r.allowedUsers.length} 位指定用户聊天`
        : "🌐 白名单已清除：不限制聊天对象",
    );
    pushLog(
      voiceEngine.value === "cosyvoice"
        ? "🎙 语音引擎：CosyVoice 2（真人级音色）"
        : voiceEngine.value === "indextts"
          ? "🎙 语音引擎：IndexTTS2 本地声音克隆（诗妍的声音）"
          : "🎙 语音引擎：Edge TTS（免费神经网络音色）",
    );
  } catch (e) {
    pushLog(`保存规则失败: ${e}`);
  }
}

function setAutoReply(v: boolean) {
  autoReply.value = v;
  localStorage.setItem("clawdesk_wechat_autoreply", v ? "on" : "off");
  pushLog(v ? "🤖 自动回复已开启（所有微信收到消息后由各自 AI 自动回复）" : "⏸ 自动回复已关闭");
}

async function refreshStatus() {
  try {
    const r = await wechatApi.botStatus();
    bot.value = r?.bot ?? null;
    // 同步后端设置（重启后自动恢复的主动聊天配置；
    // 主动消息发送后轮询也能刷新"上次主动"时间）
    const b = bot.value;
    if (b) {
      syncProactive(b);
      // 人设回填：仅当本地为空且后端有值时（不打断正在编辑的内容）
      // ★ personaDirty：用户编辑中绝不回填，防止旧人设覆盖新输入
      if (!personaDirty.value && b.personaText && !personaText.value) personaText.value = b.personaText;
      // ★ 规则回填（白名单/音色/引擎）：同步后端保存的配置
      //   用户编辑中（voiceDirty）绝不回填，防止「选了又弹回」
      if (!voiceDirty.value) {
        if (b.allowedUsers) allowedUsers.value = b.allowedUsers.join("，");
        if (b.voiceId) voiceId.value = b.voiceId;
        if (b.voiceEngine) voiceEngine.value = b.voiceEngine;
        if (b.cosyvoiceApiKey) cosyvoiceKey.value = b.cosyvoiceApiKey;
        if (b.indexttsUrl) indexttsUrl.value = b.indexttsUrl;
        if (b.indexttsVoicePath) indexttsVoicePath.value = b.indexttsVoicePath;
      }
    }
  } catch { /* 静默 */ }
  // ★ AI 生活状态（世界线）：与 bot 状态一并刷新，展示 AI 此刻在做什么
  try {
    const s = await wechatApi.livingState();
    if (s) livingState.value = s;
  } catch { /* 静默 */ }
  // ★ AI 当前心情（情绪引擎）：展示 AI 此刻的心情标签
  try {
    const m = await wechatApi.moodState();
    if (m?.label) moodState.value = m.label;
  } catch { /* 静默 */ }
}

/** 灵魂全景按需拉取：面板展开时请求一次，避免 5 秒轮询重复读取八层状态。 */
async function refreshSoul() {
  try {
    soulSnap.value = await wechatApi.soulSnapshot();
  } catch { /* 静默 */ }
}

async function toggleSoul() {
  soulOpen.value = !soulOpen.value;
  if (soulOpen.value) await refreshSoul();
}

async function startQr() {
  qrState.value = "loading";
  try {
    const r = await wechatApi.getQr();
    qrcodeUrl.value = r.qrcodeUrl;
    qrSvg.value = await genQrSvg(r.qrcodeUrl);
    qrState.value = "wait";
    pushLog(`📱 微信 二维码已生成，请用手机微信扫码`);
    startPoll();
  } catch (e) {
    pushLog(`获取二维码失败: ${e}`);
    qrState.value = "idle";
  }
}

async function refreshQr() {
  try {
    const r = await wechatApi.refreshQr();
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
  // ★ 递归 setTimeout（后端长轮询最长 35s，5s interval 会堆积并发请求）
  const tick = () => {
    if (qrState.value === "confirmed") return;
    void pollQr().finally(() => {
      if (qrState.value === "confirmed" || qrState.value === "need_verifycode") return;
      pollTimer.value = window.setTimeout(tick, 5000);
    });
  };
  tick();
}

let pollBusy = false;
async function pollQr() {
  if (pollBusy || qrState.value === "confirmed") return;
  pollBusy = true;
  try {
    const r = await wechatApi.qrStatus();
    const s = r?.status;
    if (s === "confirmed") {
      qrState.value = "confirmed";
      if (pollTimer.value) window.clearTimeout(pollTimer.value);
      pushLog("✅ 扫码成功，正在启动 Bot…");
      await startBot();
    } else if (s === "need_verifycode") {
      qrState.value = "need_verifycode";
      if (pollTimer.value) window.clearTimeout(pollTimer.value);
      pushLog("🔢 手机微信显示配对码，请在下框输入");
    } else if (s === "scaned_but_redirect") {
      qrState.value = "scaned";
    } else if (s === "verify_code_blocked") {
      pushLog("❌ 配对码错误次数过多，请刷新二维码");
      qrState.value = "wait";
    } else if (s === "expired" || s === "invalid") {
      // ★ 二维码过期：明确提示并停止轮询，等用户刷新
      qrState.value = "idle";
      if (pollTimer.value) window.clearTimeout(pollTimer.value);
      pushLog("⏳ 二维码已过期，请点击「刷新二维码」重新获取");
    }
  } catch { /* 网络错误继续轮询 */ } finally {
    pollBusy = false;
  }
}

async function submitVerifyCode() {
  if (!verifyCode.value.trim()) return;
  try {
    await wechatApi.verifyCode(verifyCode.value.trim());
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
    await wechatApi.botStart();
    pushLog(`🚀 微信 Bot 已启动，长轮询接收消息中…`);
  } catch (e) {
    pushLog(`启动失败: ${e}`);
  }
  await refreshStatus();
}

async function stopBot() {
  try {
    await wechatApi.botStop();
    pushLog(`⏹ 微信 Bot 已停止`);
  } catch (e) {
    pushLog(`停止失败: ${e}`);
  }
  await refreshStatus();
}

async function logout() {
  try {
    await wechatApi.logout();
    pushLog(`👋 已登出微信`);
    qrcodeUrl.value = "";
    qrSvg.value = "";
    qrState.value = "idle";
  } catch (e) {
    pushLog(`登出失败: ${e}`);
  }
  await refreshStatus();
}

/** 清除该微信的 AI 会话记忆（删除 wechat-0 会话，下次回复不再参考旧对话）。
 *  不影响聊天记录（history.jsonl）与人设（persona.md）。
 *  后端会等待正在运行的自动回复结束后再删除，这里展示进行中状态。 */
async function clearMemory() {
  if (memoryClearing.value) return;
  const sid = "wechat-0";
  memoryClearing.value = true;
  try {
    const ok = await sessionsApi.delete(sid);
    if (ok) {
      pushLog(`🧹 已清除微信的 AI 记忆（会话 ${sid}）`);
    } else {
      pushLog(`⚠️ 该微信暂无记忆可清除（${sid} 不存在），AI 将从空白开始`);
    }
  } catch (e) {
    pushLog(`清除记忆失败: ${e}`);
  } finally {
    memoryClearing.value = false;
  }
}

/** 保存当前微信的人设（system prompt，存 D 盘 persona.md） */
async function savePersona() {
  personaLoading.value = true;
  personaSaved.value = false;
  try {
    await wechatApi.setPersona(personaText.value);
    personaSaved.value = true;
    personaDirty.value = false; // 保存成功 = 恢复同步，轮询可继续回填
    pushLog(`✅ 微信 人设已保存（${personaText.value.length} 字）`);
    setTimeout(() => { personaSaved.value = false; }, 2000);
    await refreshStatus();
  } catch (e) {
    pushLog(`人设保存失败: ${e}`);
  } finally {
    personaLoading.value = false;
  }
}

/** 读取该微信的聊天记录（D 盘 history.jsonl） */
async function loadHistory() {
  try {
    const r = await wechatApi.history();
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
    await wechatApi.botReply({
      msgId: last.msgId,
      toUser: last.fromUser,
      content: "👋 测试回复成功！ClawDesk 微信 Bot 运行正常。",
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
          <span>内置微信（独立于电脑上的微信）</span>
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

      <div class="wc-body">
        <!-- 左：登录 / 控制 / 人设 -->
        <div class="wc-left">
          <!-- 状态信息 -->
          <div class="wc-info">
            <div class="wc-row"><span>Bot ID</span><b>{{ cur?.botId || "—" }}</b></div>
            <div class="wc-row"><span>消息数</span><b>{{ cur?.messageCount ?? 0 }}</b></div>
            <div class="wc-row"><span>聊天记录</span><b>{{ cur?.historyCount ?? 0 }} 条（D 盘）</b></div>
            <div class="wc-row"><span>AI 生活状态</span><b class="wc-living">{{ livingState || "—" }}</b></div>
            <div class="wc-row"><span>AI 心情</span><b class="wc-living">{{ moodState || "平静" }}</b></div>
            <hr class="wc-split" />
            <!-- ★ 灵魂面板（折叠展开） -->
            <div class="wc-soul" v-if="soulSnap">
              <button class="wc-soul-toggle" @click="toggleSoul">{{ soulOpen ? '▾' : '▸' }} 💗 灵魂面板</button>
              <div v-show="soulOpen" class="wc-soul-body">
                <!-- OCEAN 人格底色 -->
                <div class="wc-soul-card">
                  <div class="wc-soul-title">🧬 人格底色（OCEAN）<span class="wc-anchor-tag" v-if="soulSnap.anchor">锚点 ±{{ Math.round(soulSnap.anchor.range*100) }}%</span></div>
                  <div class="wc-bar-row"><span>开放</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.traits.openness*100)+'%'}"></span><span class="wc-anchor-tick" v-if="soulSnap.anchor" :style="{left: (soulSnap.anchor.baseline.openness*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>尽责</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.traits.conscientiousness*100)+'%'}"></span><span class="wc-anchor-tick" v-if="soulSnap.anchor" :style="{left: (soulSnap.anchor.baseline.conscientiousness*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>外向</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.traits.extraversion*100)+'%'}"></span><span class="wc-anchor-tick" v-if="soulSnap.anchor" :style="{left: (soulSnap.anchor.baseline.extraversion*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>宜人</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-warm" :style="{width: (soulSnap.traits.agreeableness*100)+'%'}"></span><span class="wc-anchor-tick" v-if="soulSnap.anchor" :style="{left: (soulSnap.anchor.baseline.agreeableness*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>神经</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-sens" :style="{width: (soulSnap.traits.neuroticism*100)+'%'}"></span><span class="wc-anchor-tick" v-if="soulSnap.anchor" :style="{left: (soulSnap.anchor.baseline.neuroticism*100)+'%'}"></span></span></div>
                </div>
                <!-- 驱动力 -->
                <div class="wc-soul-card">
                  <div class="wc-soul-title">🔥 此刻内在劲</div>
                  <div class="wc-bar-row"><span>渴望联结</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.drives.connection*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>怕被遗忘</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-sens" :style="{width: (soulSnap.drives.fear_forgotten*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>分享欲</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.drives.share*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>安全感</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.drives.safety*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>顽皮</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-fun" :style="{width: (soulSnap.drives.playful*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>小执拗</span><span class="wc-bar"><span class="wc-bar-text" :style="{width: (soulSnap.drives.stubborn*100)+'%'}">{{ soulSnap.drives.stubborn > 0.2 ? '今天有点小脾气' : '—' }}</span></span></div>
                </div>
                <!-- 好感度（affinity） -->
                <div class="wc-soul-card" v-if="soulSnap.affinity">
                  <div class="wc-soul-title">💞 好感度</div>
                  <div class="wc-bar-row"><span>温暖</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-warm" :style="{width: (soulSnap.affinity.warmth*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>信任</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.affinity.trust*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>好奇</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-fun" :style="{width: (soulSnap.affinity.intrigue*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>亲密</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-warm" :style="{width: (soulSnap.affinity.intimacy*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>耐心</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.affinity.patience*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>张力</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-sens" :style="{width: (soulSnap.affinity.tension*100)+'%'}"></span></span></div>
                </div>
                <!-- 情绪 -->
                <div class="wc-soul-card">
                  <div class="wc-soul-title">💫 情绪（{{ soulSnap.mood.label }}）</div>
                  <div class="wc-bar-row"><span>愉悦</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-fun" :style="{width: (soulSnap.mood.joy*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>想念</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-warm" :style="{width: (soulSnap.mood.longing*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>孤独</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-sens" :style="{width: (soulSnap.mood.loneliness*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>依恋</span><span class="wc-bar"><span class="wc-bar-fill wc-bar-warm" :style="{width: (soulSnap.mood.attachment*100)+'%'}"></span></span></div>
                  <div class="wc-bar-row"><span>强度</span><span class="wc-bar"><span class="wc-bar-fill" :style="{width: (soulSnap.mood.arousal*100)+'%'}"></span></span></div>
                </div>
                <!-- 生活 / 叙事 / 关系 / 记忆 -->
                <div class="wc-soul-card">
                  <div class="wc-soul-title">🌱 生活 · 记忆</div>
                  <div class="wc-soul-text">{{ soulSnap.living }}</div>
                  <div v-if="soulSnap.narrative" class="wc-soul-text wc-soul-sub">📖 {{ soulSnap.narrative }}</div>
                  <div class="wc-soul-text" style="margin-top:4px">💞 关系记忆 {{ soulSnap.relationship }} 条 · 📌 细节记忆 {{ soulSnap.details?.total ?? 0 }} 条（稳定事实 {{ soulSnap.details?.profile ?? 0 }} · 会话时刻 {{ soulSnap.details?.relationship ?? 0 }}）</div>
                  <div v-if="soulSnap.ghost" class="wc-soul-text wc-ghost-text">
                    👻 连续沉默 {{ soulSnap.ghost.streak }} 次 <span v-if="soulSnap.ghost.last_ghost_ms">· 上次 {{ fmtGhostAgo(soulSnap.ghost.last_ghost_ms) }}</span>
                  </div>
                </div>
              </div>
            </div>
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
                <span v-else>用手机微信扫码登录</span>
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
              <button v-if="!cur?.running" class="wc-btn wc-primary" @click="startBot">▶ 启动消息收发</button>
              <button v-else class="wc-btn" @click="stopBot">⏸ 停止消息收发</button>
              <button class="wc-btn" @click="testReply">📨 测试回复</button>
              <button class="wc-btn wc-danger" @click="logout">登出此账号</button>
            </div>
            <p class="wc-tip">
              {{ cur?.running ? "消息收发已启动：好友发来消息时，由该账号的 AI 自动回复（可在右上角开关关闭）。" : "消息收发未运行，仅能在中栏手动聊天。点「启动消息收发」让 AI 自动回复。" }}
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
                <span style="color:var(--color-text-secondary);">~</span>
                <input
                  :value="proactiveIntervalMax"
                  type="number"
                  min="1"
                  max="1440"
                  class="wc-num-input"
                  title="最长间隔（分钟）"
                  @change="(e: any) => { proactiveIntervalMax = Number((e.target as HTMLInputElement).value) || 180; saveProactive(); }"
                />
                <span style="font-size:11px; color:var(--color-text-muted);">分钟</span>
              </b></div>
              <div class="wc-row"><span>目标用户</span><b style="max-width:55%; overflow:hidden; text-overflow:ellipsis;">{{ proactiveTarget || "自动（最近聊过的人）" }}</b></div>
              <div class="wc-row"><span>上次主动</span><b>{{ proactiveLastAt ? fmtTs(proactiveLastAt) : "—" }}</b></div>
            </div>
            <p class="wc-tip wc-tip-warn">
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
            <button class="wc-btn wc-danger" style="margin-left:8px;" @click="clearMemory" :disabled="memoryClearing" title="删除该微信的 AI 会话记忆（wechat-0），不影响聊天记录与人设">
              {{ memoryClearing ? "清空中…" : "🧹 清除 AI 记忆" }}
            </button>
          </div>

          <!-- 使用规则（白名单 / 语音） -->
          <div class="wc-persona">
            <div class="wc-log-title">🔒 使用规则（谁可以聊天 / AI 的声音）</div>
            <div class="wc-info" style="margin: 0 10px;">
              <div class="wc-row"><span>只允许和谁聊</span><b>
                <input v-model="allowedUsers" class="wc-num-input" style="width:200px;" placeholder="留空 = 不限制；填微信用户 ID，逗号分隔" @change="voiceDirty = true" />
              </b></div>
              <div class="wc-row"><span>AI 的声音</span><b>
                <select v-model="voiceEngine" class="wc-num-input" style="width:130px;" @change="voiceDirty = true">
                  <option value="edge">Edge TTS</option>
                  <option value="cosyvoice">CosyVoice 2</option>
                  <option value="indextts">IndexTTS2 克隆</option>
                </select>
                <select v-if="voiceEngine === 'edge'" v-model="voiceId" class="wc-num-input" style="width:150px;" @change="voiceDirty = true">
                  <option v-for="v in voiceOptions" :key="v.id" :value="v.id">{{ v.name }}</option>
                </select>
                <select v-else-if="voiceEngine === 'cosyvoice'" v-model="voiceId" class="wc-num-input" style="width:150px;" @change="voiceDirty = true">
                  <option v-for="v in cosyvoiceOptions" :key="v.id" :value="v.id">{{ v.name }}</option>
                </select>
              </b></div>
              <div v-if="voiceEngine === 'cosyvoice'" class="wc-row"><span>硅基流动 Key</span><b>
                <input v-model="cosyvoiceKey" class="wc-num-input" style="width:200px;" placeholder="sk-...（免费申请，留空回退 Edge）" type="password" @change="voiceDirty = true" />
              </b></div>
              <div v-if="voiceEngine === 'indextts'" class="wc-row"><span>参考音频</span><b>
                <input v-model="indexttsVoicePath" class="wc-num-input" style="width:200px;" placeholder="D:\...\诗妍.wav（10~30秒清晰人声）" @change="voiceDirty = true" />
              </b></div>
              <div class="wc-row"><span>语音回复</span><b>
                <label class="wc-switch">
                  <input type="checkbox" :checked="voiceReply" @change="(e: any) => setVoiceReply((e.target as HTMLInputElement).checked)" />
                  <span class="wc-knob"></span>
                </label>
              </b></div>
            </div>
            <div style="padding: 0 10px 10px;">
              <button class="wc-btn wc-primary" @click="saveRules">保存规则</button>
              <span style="font-size:11px; color:var(--color-text-muted); margin-left:8px;">白名单外的消息不回复、不主动找</span>
            </div>
          </div>

          <!-- 内置微信说明 -->
          <div class="wc-persona">
            <div class="wc-log-title">💎 内置微信（账号跑在软件里，不影响电脑上的微信）</div>
            <div class="wc-info" style="margin: 0 10px;">
              <p style="font-size:11px; color:var(--color-text-secondary); margin:2px 0; line-height:1.6;">
                用手机微信扫码登录后，该账号完全在 ClawDesk 内收发消息（中栏聊天界面），
                <b>不需要多开、不需要在电脑上装第二个微信</b>，也不影响你电脑上正常运行的微信。
              </p>
              <p style="font-size:11px; color:var(--color-text-secondary); margin:2px 0; line-height:1.6;">
                👉 建议用专用小号登录作为 AI 的独立微信；AI 自动回复与你手动聊天（中栏）
                共用同一账号同一会话，互不冲突。
              </p>
            </div>
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

        <!-- 中：内置微信聊天界面（会话列表 + 聊天窗 + 输入框） -->
        <div class="wc-mid">
          <!-- 未登录：先扫码登录该内置微信 -->
          <div v-if="!cur?.loggedIn" class="wc-mid-login">
            <div class="wc-log-title">📱 登录内置微信</div>
            <p class="wc-login-desc">
              用手机微信<b>扫码登录</b>（建议用专用小号），该账号将完全在 ClawDesk 内运行，
              不影响电脑上正常使用的微信。
            </p>
            <div v-if="qrSvg" class="wc-qr-svg" v-html="qrSvg"></div>
            <img v-else-if="qrcodeUrl" class="wc-qr-img" :src="qrcodeUrl" alt="微信登录二维码" />
            <div class="wc-qr-hint">
              <template v-if="qrState === 'loading'">获取二维码中…</template>
              <template v-else-if="qrState === 'wait'">⏳ 等待扫码…</template>
              <template v-else-if="qrState === 'scaned'">📲 已扫码，等待手机确认…</template>
              <template v-else-if="qrState === 'confirmed'">✅ 登录成功，正在进入内置微信…</template>
              <template v-else-if="qrState === 'need_verifycode'">🔢 手机微信显示配对码，请在下框输入</template>
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
            <div class="wc-login-ops">
              <button class="wc-btn wc-primary" @click="startQr" :disabled="qrState === 'loading'">
                {{ qrState === 'loading' ? '获取中…' : '获取登录二维码' }}
              </button>
              <button v-if="qrState === 'wait' || qrState === 'scaned'" class="wc-btn" @click="refreshQr">刷新二维码</button>
            </div>
            <p class="wc-login-tip">提示：用手机微信扫码登录后，该账号即成为 ClawDesk 的内置微信（不占用你电脑上的微信）。</p>
          </div>

          <!-- 已登录：会话列表 + 聊天窗 -->
          <template v-else>
          <!-- 会话列表 -->
          <div class="wc-chat-list">
            <div class="wc-log-title">💬 会话（{{ chats.length }}）</div>
            <div
              v-for="c in chats"
              :key="c.dir"
              class="wc-chat-item"
              :class="{ active: c.dir === activeChat }"
              @click="openChat(c.dir)"
            >
              <span class="wc-chat-name">{{ c.dir }}</span>
              <span class="wc-chat-preview">{{ typeIcon(c.lastType) }} {{ c.last }}</span>
              <span class="wc-chat-time">{{ fmtTs(c.lastTime) }}</span>
            </div>
            <p v-if="!chats.length" class="wc-log-empty">暂无会话 — 好友发来消息后自动出现在这里</p>
          </div>
          <!-- 聊天窗 -->
          <div class="wc-chat-main">
            <template v-if="activeChat">
              <div class="wc-chat-head">{{ activeChat }} <span class="wc-state">· {{ chatTip }}</span></div>
              <div class="wc-bubbles">
                <div v-for="(m, i) in chatMsgs" :key="i" class="wc-bubble-row" :class="m.fromBot ? 'me' : 'them'">
                  <div class="wc-bubble" :title="fmtTs(m.timestamp)">{{ m.content }}</div>
                </div>
                <p v-if="!chatMsgs.length" class="wc-log-empty">暂无消息，发一句开场白吧</p>
              </div>
              <div class="wc-chat-input">
                <textarea
                  v-model="chatInput"
                  rows="2"
                  placeholder="以该微信身份发送消息（Enter 发送，Shift+Enter 换行）"
                  @keydown.enter.exact.prevent="sendChat"
                ></textarea>
                <button class="wc-btn wc-primary" :disabled="chatSending || !chatInput.trim() || !activeChat" @click="sendChat">
                  {{ chatSending ? "发送中…" : "发送" }}
                </button>
              </div>
            </template>
            <p v-else class="wc-log-empty" style="margin-top:40px; text-align:center;">← 选择一个会话开始聊天<br /><br />这是该微信的内置聊天界面，<br />AI 自动回复与本界面共用同一会话</p>
          </div>
          </template>
        </div>

        <!-- 右：最近消息 + 聊天记录 -->
        <div class="wc-right">
          <div class="wc-log-title">最近消息（{{ messages.length }}）</div>
          <div class="wc-msgs">
            <div v-for="m in messages" :key="m.msgId + m.timestamp" class="wc-msg">
              <div class="wc-msg-meta">
                <span class="wc-msg-from">{{ m.fromUser.slice(0, 8) }}</span>
                <span class="wc-msg-time">{{ fmtTs(m.timestamp) }} {{ typeIcon(m.msgType) }}</span>
              </div>
              <div class="wc-msg-content">{{ m.content }}</div>
            </div>
            <p v-if="!messages.length" class="wc-log-empty">暂无消息</p>
          </div>
          <div class="wc-log-title" style="border-top: 1px solid var(--glass-border);">📜 D 盘聊天记录（{{ historyList.length }} 条）</div>
          <div class="wc-msgs">
            <div v-for="(h, i) in historyList.slice(-20).reverse()" :key="i" class="wc-msg">
              <div class="wc-msg-meta">
                <span class="wc-msg-from">{{ h.fromUser === h.toUser ? h.fromUser.slice(0, 8) : "我" }}</span>
                <span class="wc-msg-time">{{ fmtTs(h.timestamp) }} · {{ typeIcon(h.msgType) }}</span>
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
  width: min(1180px, 96vw);
  height: min(640px, 88vh);
  background: linear-gradient(180deg, #12131d 0%, #0d0d14 100%);
  border: 1px solid var(--color-border-light);
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
  border-bottom: 1px solid var(--glass-border);
  flex-shrink: 0;
}
.wc-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 15px;
  font-weight: 600;
  color: var(--color-text);
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
.wc-dot.gray { background: var(--color-text-muted); }
.wc-state {
  font-size: 12px;
  color: var(--color-text-secondary);
  font-weight: 400;
}
.wc-close {
  background: none;
  border: none;
  color: var(--color-text-secondary);
  font-size: 15px;
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 6px;
}
.wc-close:hover { background: var(--glass-border); color: #fff; }
.wc-body {
  display: flex;
  flex: 1;
  min-height: 0;
}
.wc-left {
  width: 32%;
  border-right: 1px solid var(--glass-border);
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
/* ── 中栏：内置微信聊天界面 ── */
.wc-mid {
  width: 38%;
  border-right: 1px solid var(--glass-border);
  display: flex;
  flex-direction: column;
  min-width: 0;
  background: var(--color-surface);
}
.wc-chat-list {
  height: 34%;
  border-bottom: 1px solid var(--glass-border);
  padding: 10px 8px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  flex-shrink: 0;
}
.wc-chat-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  border-radius: 8px;
  cursor: pointer;
  background: var(--color-surface-hover);
  border: 1px solid transparent;
  min-width: 0;
}
.wc-chat-item:hover { border-color: var(--color-border-light); }
.wc-chat-item.active {
  border-color: var(--color-accent);
  background: var(--color-surface-hover);
  box-shadow: inset 3px 0 0 var(--color-accent);
}
.wc-chat-name {
  font-size: 12px;
  color: var(--color-text);
  font-weight: 600;
  flex-shrink: 0;
  max-width: 34%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.wc-chat-preview {
  flex: 1;
  min-width: 0;
  font-size: 11px;
  color: var(--color-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.wc-chat-time { font-size: 10px; color: var(--color-text-muted); flex-shrink: 0; }
.wc-chat-main {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding: 10px 12px;
}
.wc-chat-head {
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text);
  padding-bottom: 8px;
  border-bottom: 1px solid var(--glass-border);
  margin-bottom: 8px;
  flex-shrink: 0;
}
.wc-bubbles {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 4px 2px;
}
.wc-bubble-row {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
}
.wc-bubble-row.me { align-items: flex-end; }
.wc-bubble {
  max-width: 82%;
  padding: 7px 11px;
  border-radius: 10px;
  font-size: 12px;
  line-height: 1.5;
  word-break: break-word;
  white-space: pre-wrap;
}
.wc-bubble-row.them .wc-bubble { background: var(--glass-border); color: var(--color-text); border-bottom-left-radius: 3px; }
.wc-bubble-row.me .wc-bubble { background: var(--color-accent-hover); color: #fff; border-bottom-right-radius: 3px; }
.wc-chat-input {
  display: flex;
  gap: 8px;
  align-items: flex-end;
  padding-top: 8px;
  border-top: 1px solid var(--glass-border);
  flex-shrink: 0;
}
.wc-chat-input textarea {
  flex: 1;
  resize: none;
  background: var(--color-surface-hover);
  border: 1px solid var(--color-border-light);
  border-radius: 8px;
  color: var(--color-text);
  font-size: 12px;
  padding: 8px 10px;
  outline: none;
  font-family: inherit;
  line-height: 1.5;
  min-height: 54px;
  max-height: 120px;
}
.wc-chat-input textarea:focus { border-color: var(--color-accent); }
/* ── 中栏登录面板（未登录时） ── */
.wc-mid-login {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  padding: 24px 20px;
  overflow-y: auto;
}
.wc-login-desc {
  font-size: 12px;
  color: var(--color-text-secondary);
  text-align: center;
  line-height: 1.7;
  margin: 0;
}
.wc-login-ops {
  display: flex;
  gap: 8px;
  align-items: center;
}
.wc-login-tip {
  font-size: 11px;
  color: var(--color-text-muted);
  text-align: center;
  margin: 0;
  line-height: 1.6;
}
.wc-right {
  flex: 1;
  padding: 16px;
  display: flex;
  flex-direction: column;
  min-width: 0;
}
.wc-info {
  background: var(--color-card);
  border: 1px solid var(--glass-border);
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
  color: var(--color-text-secondary);
}
.wc-row b { color: var(--color-text); font-weight: 500; }
.wc-living { color: var(--color-accent); font-weight: 600; }
.wc-switch { position: relative; display: inline-block; width: 34px; height: 19px; vertical-align: middle; }
.wc-switch input { opacity: 0; width: 0; height: 0; }
.wc-knob {
  position: absolute;
  inset: 0;
  background: var(--color-border-light);
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
  background: var(--color-text);
  border-radius: 50%;
  transition: 0.2s;
}
.wc-switch input:checked + .wc-knob { background: #34d399; }
.wc-switch input:checked + .wc-knob::before { transform: translateX(15px); background: #fff; }

.wc-qr {
  background: var(--color-card);
  border: 1px solid var(--glass-border);
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
  color: var(--color-text-muted);
  font-size: 13px;
  width: 100%;
  border: 1px dashed var(--color-border-light);
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
.wc-qr-hint { font-size: 12.5px; color: var(--color-text-secondary); }
.wc-qr-ops { display: flex; gap: 8px; }
.wc-verify { display: flex; gap: 8px; width: 100%; }
.wc-input {
  flex: 1;
  background: var(--color-input-bg);
  border: 1px solid var(--color-border-light);
  border-radius: 8px;
  color: var(--color-text);
  padding: 8px 10px;
  font-size: 13px;
  outline: none;
}
.wc-input:focus { border-color: var(--color-accent); }

.wc-btn {
  background: var(--glass-border);
  color: var(--color-text);
  border: 1px solid var(--color-border-light);
  border-radius: 8px;
  padding: 7px 14px;
  font-size: 13px;
  cursor: pointer;
  transition: 0.15s;
}
.wc-btn:hover { background: var(--color-surface-hover); }
.wc-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.wc-primary { background: var(--color-accent); border-color: var(--color-accent); color: #fff; }
.wc-primary:hover { background: var(--color-accent-hover); }
.wc-danger { background: #7f1d1d; border-color: #991b1b; color: #fecaca; }
.wc-danger:hover { background: #991b1b; }
.wc-ctrl { display: flex; flex-direction: column; gap: 10px; }
.wc-ctrl-btns { display: flex; flex-wrap: wrap; gap: 8px; }
.wc-tip { font-size: 12px; color: var(--color-text-muted); margin: 0; }

.wc-log {
  flex: 1;
  min-height: 90px;
  display: flex;
  flex-direction: column;
  background: var(--color-card);
  border: 1px solid var(--glass-border);
  border-radius: 10px;
  overflow: hidden;
}
.wc-log-title {
  padding: 8px 12px;
  font-size: 12px;
  color: var(--color-text-secondary);
  border-bottom: 1px solid var(--glass-border);
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
  color: var(--color-text-secondary);
  word-break: break-all;
  font-family: Consolas, monospace;
}
.wc-log-empty { color: var(--color-text-muted); font-size: 12px; }

.wc-msgs {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-top: 8px;
}
.wc-msg {
  background: var(--color-card);
  border: 1px solid var(--glass-border);
  border-radius: 9px;
  padding: 8px 12px;
}
.wc-msg-meta {
  display: flex;
  justify-content: space-between;
  font-size: 11px;
  color: var(--color-text-muted);
  margin-bottom: 3px;
}
.wc-msg-from { color: var(--color-accent); }
.wc-msg-time { color: var(--color-text-muted); font-size: 10px; }
.wc-msg-content {
  font-size: 13px;
  color: var(--color-text);
  word-break: break-all;
  white-space: pre-wrap;
}

/* ── 人设编辑区 ── */
.wc-persona {
  display: flex;
  flex-direction: column;
  gap: 8px;
  background: var(--color-card);
  border: 1px solid var(--glass-border);
  border-radius: 10px;
  overflow: hidden;
}
.wc-persona-input {
  width: 100%;
  background: var(--color-input-bg);
  border: 1px solid var(--color-border-light);
  border-radius: 8px;
  color: var(--color-text);
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
.wc-persona-input:focus { border-color: var(--color-accent); }
.wc-persona .wc-btn { margin: 0 10px 10px; }

/* 主动聊天数字输入 */
.wc-num-input {
  width: 64px;
  background: var(--color-input-bg);
  border: 1px solid var(--color-border-light);
  border-radius: 6px;
  color: var(--color-text);
  padding: 3px 6px;
  font-size: 12px;
  text-align: right;
  outline: none;
}
.wc-num-input:focus { border-color: var(--color-accent); }

/* ── 灵魂面板 ── */
.wc-split { border: none; border-top: 1px solid var(--color-border-light); margin: 8px 0; }
.wc-soul { margin: 4px 0; }
.wc-soul-toggle {
  width: 100%;
  background: transparent;
  border: none;
  color: #f5a8c8;
  cursor: pointer;
  font-size: 12px;
  padding: 4px 0;
  text-align: left;
}
.wc-soul-toggle:hover { color: #fab5d6; }
.wc-soul-body {
  margin-top: 6px;
  max-height: 480px;
  overflow-y: auto;
}
.wc-soul-card {
  background: var(--color-card);
  border: 1px solid var(--color-border-light);
  border-radius: 8px;
  padding: 8px 10px;
  margin-bottom: 8px;
}
.wc-soul-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--color-text-secondary);
  margin-bottom: 6px;
}
.wc-soul-text {
  font-size: 11px;
  color: var(--color-text-secondary);
  line-height: 1.5;
  word-break: break-all;
}
.wc-soul-sub { margin-top: 4px; color: var(--color-text-muted); }
.wc-ghost-text { margin-top: 4px; color: var(--color-warning); }
.wc-tip-warn { margin: 0 10px 10px; color: var(--color-warning); }
.wc-bar-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 3px 0;
  font-size: 10px;
  color: var(--color-text-muted);
}
.wc-bar-row > span:first-child {
  flex: 0 0 52px;
  text-align: right;
}
.wc-bar {
  position: relative;
  flex: 1;
  height: 8px;
  background: var(--color-code-bg);
  border-radius: 4px;
  overflow: hidden;
}
.wc-bar-fill {
  display: block;
  height: 100%;
  background: linear-gradient(90deg, var(--color-accent), var(--color-accent-hover));
  border-radius: 4px;
  transition: width 0.4s ease;
}
.wc-bar-text {
  display: block; width: 100%;
  font-size: 80%; color: var(--color-text-muted);
  white-space: nowrap;
}
.wc-bar-fill.wc-bar-warm { background: linear-gradient(90deg, #ff7aa8, #ff9ec0); }
.wc-bar-fill.wc-bar-sens { background: linear-gradient(90deg, #8b7aff, #b3a8ff); }
.wc-bar-fill.wc-bar-fun { background: linear-gradient(90deg, #ffb34f, #ffcf7a); }
.wc-anchor-tag { font-size: 9px; color: var(--color-text-muted); font-weight: normal; margin-left: 4px; }
.wc-anchor-tick { position: absolute; top: 0; bottom: 0; width: 1px; background: #e8eefc; opacity: 0.7; z-index: 2; pointer-events: none; }
</style>