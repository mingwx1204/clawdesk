<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { sessionsApi, chatApi, settingsApi, routerApi, searchApi, systemApi } from "./utils/api";
import BottomInput from "./components/BottomInput.vue";
import SettingsView from "./components/SettingsView.vue";
import WechatPanel from "./components/WechatPanel.vue";
import { useWechatAutoReply } from "./composables/useWechat";
import { useSessions } from "./composables/useSessions";
import { renderMd, escapeHtml } from "./utils/markdown";
import { fmtTs, fmtArgs, fmtOutput, hasArgs, isTerminal, termInfo, hasToolDetail, toolSummary } from "./utils/messageFormat";
import type { ChatMsg, ToolCallInfo } from "./types/message";

// ★ 微信自动回复（独立 composable：监听 wechat-message → AI 回复 → 回发）
const { listenWechatMessages } = useWechatAutoReply(() => apiKey.value);

// ── 会话管理（composable：列表/重命名/分支/删除/断点） ──
const {
  sessionId,
  sessions,
  sessionNames,
  checkpoints,
  refresh: refreshSessions,
  rename: renameSessionApi,
  fork: forkSessionApi,
  remove: removeSession,
} = useSessions();

// Markdown 渲染与代码块复制逻辑已拆至 ./utils/markdown（renderMd/escapeHtml/fence）。
// onDocClick 保留在此（需访问 openImageViewer）。

/** 代码块复制（全局 click 委托）。 */
function onDocClick(e: MouseEvent) {
  const target = e.target as HTMLElement;
  const btn = target.closest?.(".code-copy");
  if (btn) {
    const block = btn.closest?.(".code-block");
    const codeEl = block?.querySelector?.("pre code");
    const text = codeEl?.textContent ?? "";
    navigator.clipboard?.writeText(text).then(() => {
      const old = btn.textContent;
      btn.textContent = "已复制 ✓";
      setTimeout(() => { btn.textContent = old; }, 1500);
    }).catch(() => {});
    return;
  }
  // 图片浏览：点击 Markdown 渲染的消息内图片 → 打开大图查看器
  if (target.tagName === "IMG" && target.closest?.(".msg-content")) {
    const src = target.getAttribute("src");
    if (src) openImageViewer([src], 0);
  }
}

// ── 图片查看器（点击图片放大浏览，左右切换 / Esc 关闭） ──
const imageViewer = ref<{ list: string[]; index: number } | null>(null);
function openImageViewer(list: string[], index: number) {
  imageViewer.value = { list, index };
}
function ivPrev() {
  if (!imageViewer.value) return;
  const n = imageViewer.value.list.length;
  imageViewer.value.index = (imageViewer.value.index - 1 + n) % n;
}
function ivNext() {
  if (!imageViewer.value) return;
  const n = imageViewer.value.list.length;
  imageViewer.value.index = (imageViewer.value.index + 1) % n;
}
function onDocKeydown(e: KeyboardEvent) {
  if (!imageViewer.value) return;
  if (e.key === "Escape") imageViewer.value = null;
  else if (e.key === "ArrowLeft") ivPrev();
  else if (e.key === "ArrowRight") ivNext();
}

/**
 * ClawDesk 桌面客户端 —— 主布局（深色主题 · 全中文）。
 *
 * 顶部工具栏：Agent 总开关 + 权限模式下拉 + 迭代计数 + 设置入口
 * 消息区：消息气泡 + 工具调用卡片（三色状态）
 * 底部：输入栏（支持图片粘贴/拖拽/上传）+ 状态栏（API Key）
 */

// ChatMsg / ToolCallInfo 类型已拆至 ./types/message（供消息区/工具卡复用）

const apiKey = ref("");
// ★ 安全修复：不再硬编码 Key。通过 DPAPI 加密持久化（重启自动恢复），
//   或用户在设置中手动填入。避免真实 Key 泄露到源代码/版本控制。
const messages = ref<ChatMsg[]>([]);
/** 按需渲染历史消息：初始只渲染最近 MSG_PAGE 条，滚动到顶部时增量加载更多。 */
const MSG_PAGE = 50;
const visibleCount = ref(MSG_PAGE);
const displayMessages = computed(() => messages.value.slice(Math.max(0, messages.value.length - visibleCount.value)));
const running = ref(false);
const runId = ref("");
/** 流式输出状态（项目 8：打字机逐字渲染 + 工具进度实时渲染）。 */
const streamingMsgId = ref<string | null>(null);
const pendingText = ref("");

// Agent 配置
const maxRounds = ref(15);
const currentRound = ref(0);

// 设置面板
const showSettings = ref(false);
// 微信 Bot 面板
const showWechat = ref(false);
/** 微信 Bot 在线状态（顶栏圆点显示） */
const wechatOnline = ref(false);
// 消息操作 / 搜索 / 导出
const bottomInputRef = ref<InstanceType<typeof BottomInput> | null>(null);
const searchOpen = ref(false);
const searchKeyword = ref("");
const searchResults = ref<{ sessionId: string; role: string; content: string }[]>([]);
const searching = ref(false);
const exporting = ref(false);

// ── v6 交互状态 ──
// 侧边栏折叠状态（左侧会话列表，可用标题栏按钮切换）
const sidebarCollapsed = ref(false);
const selectedModel = ref("auto"); // auto / deepseek-v4-flash / deepseek-v4-pro
const permRequest = ref<{ toolId: string; args: string; callId?: string } | null>(null);
const clockTime = ref("");
const clockDate = ref("");
// 时区自动保存（localStorage）：设置面板外观页切换后重启不丢失
const tz = ref(localStorage.getItem("clawdesk_tz") || "Asia/Shanghai");
const ctxPct = ref(47);
const ctxTokens = ref("493.4K / 1M 个令牌");
const ctxItems = ref({ sys: [1.4, 4.7], usr: [19.3, 20.2, 2.8] });
let clockTimer: number | null = null;
let glowEl: HTMLElement | null = null;
let artEl: HTMLElement | null = null;

let unlisten: UnlistenFn | null = null;
let unlistenStream: UnlistenFn | null = null;
let unlistenWechat: UnlistenFn | null = null;
let unlistenWechatStatus: UnlistenFn | null = null;
// 缺陷2修复：engine://stream 增量文本累积缓冲（delta → 整段 modelText）
let streamBuf = "";
let thinkingBuf = "";

onMounted(async () => {
  try {
    unlisten = await listen<any>("agent://progress", (e) => handleProgress(e.payload));
    // 缺陷2修复：监听引擎 SSE 流式事件（harness_start_task 新路径）
    unlistenStream = await listen<any>("engine://stream", (e) => handleEngineStream(e.payload));
    // 微信 Bot：收到微信用户消息 → 自动回复 + 更新在线状态
    unlistenWechat = await listenWechatMessages();
    unlistenWechatStatus = await listen<any>("wechat-bot-status", (e) => {
      const t = e.payload?.type;
      wechatOnline.value = t === "connected" || t === "resumed";
      if (t === "session_expired") wechatOnline.value = false;
    });
    // 全局异常兜底（项目 13）：启动轮询最近一次未捕获异常，弹中文报错并自动取消任务
    checkLastError();
  } catch (e) {
    console.error("进度事件监听失败", e);
  }
  await Promise.all([refreshSessions(), loadConfig()]);
  await loadSessionMessages(sessionId.value); // 启动时加载默认会话历史
  await loadSessionUsage(); // 启动时加载真实上下文占用
  // 恢复内存态 API Key（后端 AppState 持有）；未配置时自动填入用户提供的 DeepSeek Key
  try {
    const k = await settingsApi.getKeys();
    if (k?.main) {
      apiKey.value = k.main;
    } else {
      // Key 已通过 DPAPI 加密持久化，重启自动恢复；首次使用请在设置中手动填入
      apiKey.value = "";
    }
  } catch { /* 静默 */ }
  // v6：壁纸时钟 + 鼠标互动
  updateClock();
  clockTimer = window.setInterval(updateClock, 1000);
  glowEl = document.getElementById("mouseGlow");
  artEl = document.querySelector(".wallpaper .art");
  document.addEventListener("mousemove", onMouseMove);
  document.addEventListener("click", onDocClick);
  document.addEventListener("keydown", onDocKeydown);
  // 恢复外观设置（重启后保持）：深色模式 + 界面不透明度
  try {
    const s = await settingsApi.get();
    if (s?.darkTheme) document.documentElement.setAttribute("data-theme", "dark");
    if (typeof s?.uiOpacity === "number") {
      document.documentElement.style.setProperty("--ui-op", String(s.uiOpacity));
    }
    // ★ 字号设置实际生效（12~22px）
    if (typeof s?.fontSize === "number" && s.fontSize >= 12 && s.fontSize <= 22) {
      document.documentElement.style.fontSize = s.fontSize + "px";
    }
  } catch { /* 静默 */ }
  // 预加载 TTS 设置与音色列表（Edge TTS 引擎：提前加载避免首次朗读等待）
  void import("./lib/tts").then(({ loadTtsSettings }) => void loadTtsSettings());
});

onUnmounted(() => {
  unlisten?.();
  unlistenStream?.();
  unlistenWechat?.();
  unlistenWechatStatus?.();
  if (clockTimer) window.clearInterval(clockTimer);
  document.removeEventListener("mousemove", onMouseMove);
  document.removeEventListener("click", onDocClick);
  document.removeEventListener("keydown", onDocKeydown);
});

/** 查询最近一次未捕获异常：弹中文报错 + 自动终止当前任务。 */
async function checkLastError() {
  try {
    const err = await systemApi.lastError();
    if (!err) return;
    // 只弹最近 10 分钟内的异常，避免历史残留记录（如 cargo test 失败）被误报为运行时异常
    if (err.timestamp) {
      const t = Date.parse(err.timestamp);
      if (!isNaN(t) && Date.now() - t > 10 * 60 * 1000) {
        return;
      }
    }
    window.alert(`⚠️ ClawDesk 捕获到未处理异常：\n\n${err.message}\n位置：${err.location}\n\n详细日志：${err.logPath}\n\n已自动终止当前任务。`);
    if (runId.value) await chatApi.cancel(runId.value);
  } catch { /* 查询失败静默 */ }
}

/** 加载指定会话的历史消息（切换会话时恢复显示）。 */
async function loadSessionMessages(id: string) {
  try {
    const msgs = await sessionsApi.messages(id);
    const list = msgs ?? [];
    messages.value = list
      .filter((m: any) => m.role === "user" || m.role === "assistant")
      .map((m: any, i: number) => ({
        id: `m${Date.now()}${i}`,
        role: m.role as "user" | "assistant",
        content: m.content ?? "",
        timestamp: Date.now() - (list.length - i) * 1000,
        // 若后端会话含图片（dataUrl / 本地路径）则保留，切换会话后仍可浏览
        images: Array.isArray(m.images) && m.images.length ? m.images : undefined,
      }));
    visibleCount.value = MSG_PAGE; // 切换会话后只渲染最近 MSG_PAGE 条
  } catch (e) {
    console.error("加载会话消息失败", e);
    messages.value = [];
    visibleCount.value = MSG_PAGE;
  }
  // 加载/切换会话后强制滚到底部（重进软件无需手动下滑）
  await nextTick();
  scrollToBottom(true);
}

/** 切换会话：设置 sessionId 并加载历史消息。 */
async function selectSession(s: string) {
  if (running.value) {
    window.alert("AI 正在运行中，请先停止或等待完成后再切换会话");
    return;
  }
  sessionId.value = s;
  await loadSessionMessages(s);
  await loadSessionUsage(); // 切换会话后刷新上下文占用
}

/** 重命名会话（弹窗输入；留空恢复默认 id）。 */
async function renameSession(id: string) {
  const cur = sessionNames.value[id] || id;
  const name = window.prompt("重命名会话（留空恢复默认名称）", cur);
  if (name === null) return;
  await renameSessionApi(id, name);
}

/** Fork 分支会话（§十二.2）：完整拷贝记忆，新会话独立。 */
async function forkSession(id: string) {
  const newId = await forkSessionApi(id);
  if (newId) {
    await loadSessionMessages(newId); // fork 拷贝了父会话记忆，需恢复显示
  }
}

/** 从断点续跑（§十二.1）：带 resume=true 发起任务。 */
async function resumeSession(id: string) {
  sessionId.value = id;
  await loadSessionMessages(id); // 恢复该会话历史
  await refreshSessions();
  // 前端切换会话后由 send(resume) 发起；这里仅提示用户输入新指令后会自动续跑
}

async function loadConfig() {
  try {
    maxRounds.value = await chatApi.maxRounds();
  } catch (e) {
    console.error("加载最大轮数失败", e);
  }
}


/** engine://stream 事件 → 现有 handleProgress 形态（含 delta 累积）。 */
function handleEngineStream(payload: any) {
  // ★ 运行状态守卫：取消/停止后的迟到事件一律忽略，防止旧任务写入新会话或新消息
  if (!running.value && payload?.type !== "turn_finished") return;
  switch (payload?.type) {
    case "text_delta":
      // 正式回答开始：思考链已流式显示，这里仅兜底（若因故未显示则补上完整思考链）
      if (thinkingBuf) {
        const tm = messages.value.find((m) => m.id === streamingMsgId.value);
        if (tm && (!tm.thinking || tm.thinking === "思考中…")) tm.thinking = thinkingBuf;
        thinkingBuf = "";
      }
      // ★ 只传当前 delta，由 handleProgress modelText 统一累积（避免双重累积/跳字）
      handleProgress({ type: "modelText", round: 0, text: payload.content ?? "" });
      break;
    case "thinking_delta":
      // ★ 思考链按顺序流式追加显示（用户要求：对话区完整、按顺序输出思考过程）
      thinkingBuf += payload.content ?? "";
      {
        const tm = messages.value.find((m) => m.id === streamingMsgId.value);
        if (tm) {
          const cur = !tm.thinking || tm.thinking === "思考中…" ? "" : tm.thinking;
          tm.thinking = cur + (payload.content ?? "");
          tm.thinkingOpen = true; // 默认展开，完整可见
        }
      }
      break;
    case "tool_start": {
      const toolId = String(payload.name ?? "").split("__").join(":");
      handleProgress({ type: "toolCall", round: 0, toolId, status: "running", output: null });
      break;
    }
    case "tool_end": {
      const toolId = String(payload.name ?? "").split("__").join(":");
      handleProgress({ type: "toolCall", round: 0, toolId, status: payload.ok === false ? "error" : "success", output: payload.result });
      break;
    }
    case "confirm":
      handleProgress({ type: "confirmRequired", callId: payload.callId, toolId: payload.toolId, arguments: payload.arguments });
      break;
    case "status":
      // ★ 状态类文案（"正在执行…"）不混入正式回答正文，避免污染消息/朗读
      break;
    case "error":
      handleProgress({ type: "modelText", round: 0, text: "[错误] " + payload.message });
      break;
    case "turn_finished":
      handleProgress({ type: payload.ok ? "finished" : "cancelled" });
      break;
    default:
      break; // 未知 type 忽略
  }
}
function handleProgress(ev: any) {
  switch (ev.type) {
    case "roundStarted":
      currentRound.value = ev.round;
      break;
    case "modelText": {
      const t = ev.text ?? "";
      if (t.startsWith("💭")) {
        // 思考链（后端累积后发送，前缀 💭）：默认展开完整显示，不走正式回答/打字机
        ensureStreamingMessage();
        const msg = messages.value.find((m) => m.id === streamingMsgId.value);
        if (msg) {
          msg.thinking = t.replace(/^💭\s*/, "");
          msg.thinkingOpen = true;
        }
        return;
      }
      // 正式回答：★ 后端 agent://progress 的 modelText 每次只发「当前 delta」，
      // 必须累积（streamBuf）再显示，否则内容只有当前片段 → 界面跳字。
      ensureStreamingMessage();
      streamBuf += t;
      pendingText.value = streamBuf;
      scheduleStreamRender();
      break;
    }
    case "toolCall": {
      // 工具进度实时渲染：更新当前 assistant 消息的 toolCalls
      ensureStreamingMessage();
      const msg = messages.value.find((m) => m.id === streamingMsgId.value);
      if (msg) {
        if (!msg.toolCalls) msg.toolCalls = [];
        const tc: ToolCallInfo = {
          toolId: ev.toolId ?? "",
          arguments: ev.arguments ?? {},
          status: ev.status === "success" ? "success" : ev.status === "error" ? "error" : "running",
          output: ev.output,
          error: ev.error,
          open: true, // 运行中默认展开；更新时保留用户已收起的选择
        };
        const idx = msg.toolCalls.findIndex((t) => t.toolId === tc.toolId && t.status === "running");
        if (idx >= 0) {
          const prev = msg.toolCalls[idx];
          msg.toolCalls[idx] = { ...tc, open: prev.open ?? true };
        } else {
          // ★ 同一工具多次调用：前一次已结束（success/error）则追加新实例，不覆盖
          msg.toolCalls.push(tc);
        }
        // ★ 生图工具成功 → 把完整 dataUrl 提取到消息 images 数组，对话框直接显示图片
        if (tc.toolId === "generate_image" && tc.status === "success" && tc.output) {
          const out = tc.output as Record<string, unknown>;
          if (typeof out.dataUrl === "string" && out.dataUrl.startsWith("data:image/")) {
            if (!msg.images) msg.images = [];
            msg.images.push(out.dataUrl);
          }
        }
      }
      break;
    }
    case "confirmRequired": {
      // 逐步确认模式：v6 权限确认弹窗
      requestPermission(ev.toolId ?? "", ev.arguments ? JSON.stringify(ev.arguments) : "", ev.callId);
      break;
    }
    case "cancelled":
      // 兜底：若思考链未写入（只有思考无回答），任务结束时补上完整思考链
      if (thinkingBuf) {
        const tm = messages.value.find((m) => m.id === streamingMsgId.value);
        if (tm) tm.thinking = thinkingBuf;
        thinkingBuf = "";
      }
      currentRound.value = 0;
      stopTypewriter();
      break;
    case "finished":
      if (thinkingBuf) {
        const tm = messages.value.find((m) => m.id === streamingMsgId.value);
        if (tm) tm.thinking = thinkingBuf;
        thinkingBuf = "";
      }
      currentRound.value = 0;
      stopTypewriter();
      streamingMsgId.value = null;
      // ★ 自动朗读：设置开启时输出完自动朗读最后一条 AI 回复（Edge TTS 拟人音色）
      {
        const last = [...messages.value].reverse().find((m) => m.role === "assistant" && m.content && !m.content.startsWith("[错误]"));
        if (last) void import("./lib/tts").then(({ getTtsSettings, speak }) => {
          void getTtsSettings().then((s) => { if (s.enabled) void speak(last.content); });
        });
      }
      break;
  }
}

/** 确保存在流式 assistant 消息（无则创建）。 */
function ensureStreamingMessage(): void {
  if (!streamingMsgId.value) {
    const id = `m${Date.now()}stream`;
    streamingMsgId.value = id;
    messages.value.push({ id, role: "assistant", content: "", timestamp: Date.now(), thinkingOpen: true });
  }
}

/** 对话区自动滚动：新消息/流式输出自动滚到底；用户上滚查看历史时不打扰。 */
const msgsRef = ref<HTMLElement | null>(null);

function isNearBottom(): boolean {
  const el = msgsRef.value;
  if (!el) return true;
  return el.scrollHeight - el.scrollTop - el.clientHeight < 80;
}
function onMsgsScroll(): void {
  const el = msgsRef.value;
  if (!el) return;
  // 滚动到顶部（含小缓冲）且仍有更旧消息未渲染时，增量加载一批
  if (el.scrollTop < 40) void loadMoreMessages();
}
/** 向上滚动到顶部时，把更旧的一批消息纳入渲染，并补偿滚动位置避免视口跳变。 */
async function loadMoreMessages(): Promise<void> {
  const total = messages.value.length;
  if (visibleCount.value >= total) return; // 已全部渲染
  const el = msgsRef.value;
  const prevHeight = el ? el.scrollHeight : 0;
  visibleCount.value = Math.min(total, visibleCount.value + MSG_PAGE);
  await nextTick();
  if (el) {
    // 新增内容撑高了 scrollHeight，补偿 scrollTop 使原视口内容保持不动
    el.scrollTop = el.scrollHeight - prevHeight;
  }
}
function scrollToBottom(force = false): void {
  const el = msgsRef.value;
  if (!el) return;
  if (force || isNearBottom()) el.scrollTop = el.scrollHeight;
}

// 消息 / 流式文本变化 → 自动滚到底（用户上滚时自动跳过）
watch(
  [() => messages.value.length, pendingText],
  () => {
    void nextTick(() => scrollToBottom());
  },
);

/** 流式渲染节流：text_delta 高频到达时合并渲染帧（~66ms），
 *  避免每个 token 都触发整条消息的 markdown 全量重渲染（长回答时收益显著，
 *  显示体验无差别 —— 66ms 即 ~15fps 更新）。 */
let streamRenderTimer: number | null = null;
function scheduleStreamRender(): void {
  if (streamRenderTimer !== null) return;
  streamRenderTimer = window.setTimeout(() => {
    streamRenderTimer = null;
    const msg = messages.value.find((m) => m.id === streamingMsgId.value);
    if (msg) msg.content = pendingText.value;
  }, 66);
}

/** 停止流式渲染并补齐剩余文本（防漏字）。 */
function stopTypewriter(): void {
  if (streamRenderTimer !== null) {
    window.clearTimeout(streamRenderTimer);
    streamRenderTimer = null;
  }
  if (streamingMsgId.value) {
    const msg = messages.value.find((m) => m.id === streamingMsgId.value);
    if (msg && pendingText.value) msg.content = pendingText.value;
  }
}

/** 加载当前会话的真实上下文占用（后端 agent_session_usage：累计 token + 内容估算细分）。 */
async function loadSessionUsage() {
  try {
    const r = await sessionsApi.usage(sessionId.value);
    if (!r) return;
    ctxPct.value = r.pct ?? 0;
    const win = r.windowTokens ?? 0;
    const limit = r.windowLimit ?? 1_000_000;
    ctxTokens.value = `${(win / 1000).toFixed(1)}K / ${(limit / 1_000_000).toFixed(0)}M 个令牌`;
    ctxItems.value = {
      sys: [r.sys?.[0] ?? 0, r.sys?.[1] ?? 0],
      usr: [r.usr?.[0] ?? 0, r.usr?.[1] ?? 0, r.usr?.[2] ?? 0],
    };
  } catch {
    /* 静默 */
  }
}

// ── 消息操作（对标大厂客户端：复制 / 朗读 / 重新生成 / 编辑重发） ──
function copyMessage(m: ChatMsg) {
  navigator.clipboard?.writeText(m.content || "").catch(() => {});
}

/** 朗读 AI 回复（Edge TTS 神经网络拟人语音，支持多音色/语气/语速）。 */
async function speakMessage(m: ChatMsg) {
  const text = (m.content || "").replace(/[#*`>\[\]\-~|_]/g, " ");
  if (!text.trim()) return;
  const { speak } = await import("./lib/tts");
  void speak(text);
}

/** 重新生成：删除本条及之后的回复，用对应 user 指令重跑（追加新回答）。 */
async function regenerate(m: ChatMsg) {
  if (running.value) return;
  const idx = messages.value.findIndex((x) => x.id === m.id);
  if (idx < 0) return;
  let userIdx = -1;
  for (let i = idx - 1; i >= 0; i--) {
    if (messages.value[i].role === "user") { userIdx = i; break; }
  }
  if (userIdx < 0) return;
  const promptText = messages.value[userIdx].content;
  messages.value.splice(userIdx);
  await handleSend(promptText);
}

/** 编辑重发：删除该 user 及之后消息，内容回填输入框。 */
function editResend(m: ChatMsg) {
  const idx = messages.value.findIndex((x) => x.id === m.id);
  if (idx < 0) return;
  messages.value.splice(idx);
  bottomInputRef.value?.setPrompt(m.content || "");
}

/** 导出当前会话为 Markdown 文件并在资源管理器打开。 */
async function exportSession() {
  if (exporting.value) return;
  exporting.value = true;
  try {
    const path = await searchApi.export(sessionId.value);
    window.alert(`✅ 会话已导出：\n${path}\n\n将打开所在文件夹。`);
    await systemApi.openInExplorer(path).catch(() => {});
  } catch (e) {
    window.alert(`❌ 导出失败：${typeof e === "string" ? e : JSON.stringify(e)}`);
  } finally {
    exporting.value = false;
  }
}

/** 历史对话搜索（跨会话关键词检索）。 */
async function doSearch() {
  const kw = searchKeyword.value.trim();
  if (!kw) return;
  searching.value = true;
  try {
    searchResults.value = await searchApi.search(kw);
  } catch (e) {
    console.error("搜索失败", e);
    searchResults.value = [];
  } finally {
    searching.value = false;
  }
}

/** 跳转到命中会话。 */
async function jumpToResult(sid: string) {
  searchOpen.value = false;
  searchKeyword.value = "";
  searchResults.value = [];
  if (sid !== sessionId.value) {
    await selectSession(sid);
  }
}

/** 微信自动回复：收到用户消息 → 调 AI → 回发（开关存 localStorage）。 */
async function handleSend(
  content: string,
  images?: string[],
  attachments?: string[],
  opts?: { sessionId?: string; persona?: string },
) {
  if (running.value) return;
  if (!apiKey.value.trim()) {
    window.alert("请先在「设置 → 模型 API」中填写 DeepSeek API Key 后再发送");
    return;
  }
  running.value = true;
  runId.value = `run-${Date.now()}`;
  currentRound.value = 0;
  streamBuf = "";
  thinkingBuf = "";
  streamingMsgId.value = null; // 新运行独立消息实例，防止旧流式残留错位
  messages.value.push({ id: `m${Date.now()}`, role: "user", content, timestamp: Date.now(), images, attachments });

  try {
    // 附件路径拼入 prompt（不动 agent_chat 签名）：LLM 看到路径后用 file_read 读取内容
    let promptText = content;
    if (attachments?.length) {
      promptText =
        content +
        "\n\n[用户附件文件]\n" +
        attachments.map((p) => `- ${p}`).join("\n") +
        "\n以上附件已保存到本地磁盘，如需要请调用 file_read 工具读取内容。";
    }
    const outcome = await chatApi.chat({
      apiKey: apiKey.value.trim(),
      sessionId: opts?.sessionId ?? sessionId.value,
      runId: runId.value,
      prompt: promptText,
      resume: false,
      // ★ 图片：dataURL 传给后端保存到本地，模型用 analyze_image 工具查看
      images: images?.length ? [...images] : undefined,
      // ★ 人设（system prompt 追加）：虚拟机微信托管复用 Bot 槽位人设
      persona: opts?.persona ?? null,
    });
    // 流式消息已完成（打字机补齐全文）；此处仅合并工具调用记录，不重复添加消息
    stopTypewriter();
    const streamMsg = messages.value.find((m) => m.id === streamingMsgId.value);
    if (streamMsg) {
      streamMsg.content = outcome.finalText || streamMsg.content;
      const roundsCalls: ToolCallInfo[] = outcome.rounds?.flatMap((r: any) => r.toolCalls ?? []) ?? [];
      if (roundsCalls.length) streamMsg.toolCalls = roundsCalls;
      streamingMsgId.value = null;
    } else {
      messages.value.push({
        id: `m${Date.now()}r`,
        role: "assistant",
        content: outcome.finalText,
        timestamp: Date.now(),
        toolCalls: outcome.rounds?.flatMap((r: any) => r.toolCalls ?? []) ?? [],
      });
    }
    // 更新上下文进度环（真实 token 用量）
    const u = outcome.usage as { prompt_tokens?: number; completion_tokens?: number; total_tokens?: number } | undefined;
    if (u?.total_tokens) {
      ctxTokens.value = `${(u.total_tokens / 1000).toFixed(1)}K / 1M 个令牌`;
      ctxPct.value = Math.min(100, Math.round((u.total_tokens / 1_000_000) * 100));
    }
  } catch (e) {
    messages.value.push({
      id: `m${Date.now()}e`,
      role: "assistant",
      content: `❌ 运行失败：${typeof e === "string" ? e : JSON.stringify(e)}`,
      timestamp: Date.now(),
    });
  } finally {
    running.value = false;
    currentRound.value = 0;
    await refreshSessions();
    await loadSessionUsage(); // 发送完成后刷新真实上下文占用
  }
}

async function handleCancel() {
  // ★ 先向后端发起取消（等待其确认），再重置前端状态，避免旧任务继续跑产生双任务
  const id = runId.value;
  if (id) {
    try { await chatApi.cancel(id); } catch { /* 取消失败不阻塞 UI */ }
  }
  stopTypewriter();
  streamingMsgId.value = null;
  running.value = false;
  currentRound.value = 0;
}

// ── 自定义标题栏窗口控制（decorations:false，前端接管最小化/最大化/关闭）──
// 每次调用都重新 getCurrentWindow()，避免模块初始化时序问题导致实例无效。
async function winMinimize() {
  try { await getCurrentWindow().minimize(); } catch (e) { console.error("minimize 失败", e); }
}
async function winMaximize() {
  try { await getCurrentWindow().toggleMaximize(); } catch (e) { console.error("toggleMaximize 失败", e); }
}
async function winClose() {
  try { await getCurrentWindow().close(); } catch (e) { console.error("close 失败", e); }
}

/** 标题栏拖拽：左键按住非按钮区域 → 显式调用 startDragging()（比 data-tauri-drag-region 更可靠） */
async function onTitlebarMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  const target = e.target as HTMLElement | null;
  if (target?.closest(".tb-btn")) return; // 按钮区域不拖动
  try { await getCurrentWindow().startDragging(); } catch { /* 忽略 */ }
}

// ── v7：会话列表已迁移到侧边栏，不再需要下拉方式
const newSessionOpen = ref(false);
const newSessionName = ref("");
function openNewSession() {
  newSessionName.value = "";
  newSessionOpen.value = true;
}
function confirmNewSession() {
  if (running.value) {
    window.alert("AI 正在运行中，请先停止或等待完成后再新建会话");
    return;
  }
  const name = newSessionName.value.trim();
  sessionId.value = name ? `sess-${name}` : `sess-${Date.now()}`;
  messages.value = []; // 新会话无历史
  newSessionOpen.value = false;
  refreshSessions().catch(() => {});
  loadSessionUsage().catch(() => {});
}
async function deleteSession(id: string) {
  const wasCurrent = sessionId.value === id;
  const ok = await removeSession(id);
  if (!ok) {
    window.alert("删除会话失败，请重试");
    return;
  }
  if (wasCurrent) {
    await loadSessionMessages("default"); // 回到默认会话并恢复其历史
  }
}

// ── v6：模型选择（智能路由 / V4-Flash / V4-Pro） ──
const MODELS = [
  { id: "auto", label: "自动", desc: "智能路由 · 简单任务用 Flash，复杂任务用 Pro" },
  { id: "deepseek-v4-flash", label: "DeepSeek-V4-Flash", desc: "快速响应 · 日常对话 / 简单任务" },
  { id: "deepseek-v4-pro", label: "DeepSeek-V4-Pro", desc: "深度推理 · 复杂任务 / 代码 / 规划" },
];
function selectModel(m: string) {
  selectedModel.value = m;
  if (m === "deepseek-v4-flash" || m === "deepseek-v4-pro") {
    void routerApi.setMainModel(m).catch(() => {});
  }
}
// ── v6：权限确认弹窗 ──
function requestPermission(toolId: string, args: string, callId?: string) {
  permRequest.value = { toolId, args, callId };
}
function approvePermission() {
  if (permRequest.value?.callId) void chatApi.confirmCall(permRequest.value.callId, true).catch(() => {});
  permRequest.value = null;
}
function denyPermission() {
  if (permRequest.value?.callId) void chatApi.confirmCall(permRequest.value.callId, false).catch(() => {});
  permRequest.value = null;
}

// ── v6：壁纸时钟 + 时区 ──
function fmtTime(d: Date, tzs: string): string {
  return d.toLocaleTimeString("zh-CN", { timeZone: tzs, hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
function fmtDate(d: Date, tzs: string): string {
  return d.toLocaleDateString("en-US", { timeZone: tzs, year: "numeric", month: "short", day: "numeric", weekday: "long" }).toUpperCase();
}
function updateClock() {
  const now = new Date();
  clockTime.value = fmtTime(now, tz.value);
  clockDate.value = fmtDate(now, tz.value);
}
/** 时区变更回调（设置面板触发，落盘 localStorage + 刷新时钟）。 */
function onTzChange(v: string) {
  tz.value = v;
  localStorage.setItem("clawdesk_tz", v);
  updateClock();
}
function onMouseMove(e: MouseEvent) {
  if (glowEl) glowEl.style.transform = `translate(${e.clientX - 180}px,${e.clientY - 180}px)`;
  if (artEl) {
    const nx = e.clientX / window.innerWidth - 0.5;
    const ny = e.clientY / window.innerHeight - 0.5;
    artEl.style.transform = `translate(${nx * -16}px,${ny * -12}px) scale(1.06)`;
  }
}

// ── v6：压缩对话 ──
// ★ 后端会话引擎已具备自动压缩（超阈值自动摘要压缩），此处不再提供假的手动压缩按钮。
// 保留 loadSessionUsage 展示真实上下文占用（底部进度环）。

// ── v6：设置密钥回调 ──
function onKeysSaved(keys: { main?: string }) {
  if (keys.main) apiKey.value = keys.main;
}

// 工具卡 / 消息辅助函数已拆至 ./utils/messageFormat（fmtTs/fmtArgs/fmtOutput/isTerminal/termInfo/hasArgs/hasToolDetail/toolSummary）
</script>

<template>
  <div class="root">
    <!-- 动态壁纸 -->
    <div class="wallpaper">
      <div class="art"></div>
      <span class="p p1"></span><span class="p p2"></span><span class="p p3"></span>
      <span class="p p4"></span><span class="p p5"></span><span class="p p6"></span>
      <div class="glow g1"></div><div class="glow g2"></div><div class="glow g3"></div>
      <div class="mouse-glow" id="mouseGlow"></div>
      <div class="wall-clock">
        <div class="t">{{ clockTime }}</div>
        <div class="d">{{ clockDate }}</div>
      </div>
    </div>

    <div class="app">
      <!-- 自定义标题栏（无系统边框，背景透明露出壁纸，与背景融为一体） -->
      <!-- 注意：不要加 data-tauri-drag-region，它会拦截 JS 的 mousedown，导致 startDragging 不触发 -->
      <div class="titlebar" @mousedown="onTitlebarMouseDown">
        <div class="tb-brand">
          <button class="tb-btn sidebar-toggle-btn" title="侧边栏" @mousedown.stop @click="sidebarCollapsed = !sidebarCollapsed">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><line x1="9" y1="3" x2="9" y2="21"/></svg>
          </button>
          <svg class="tb-logo" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="1.6" fill="currentColor"/><ellipse cx="12" cy="12" rx="9.5" ry="3.8"/><ellipse cx="12" cy="12" rx="9.5" ry="3.8" transform="rotate(60 12 12)"/><ellipse cx="12" cy="12" rx="9.5" ry="3.8" transform="rotate(120 12 12)"/></svg>
          <span class="tb-title">ClawDesk</span>
        </div>
        <div class="tb-controls" @mousedown.stop>
          <button class="tb-btn" title="最小化" @click="winMinimize">
            <svg width="12" height="12" viewBox="0 0 12 12"><line x1="1" y1="6" x2="11" y2="6" stroke="currentColor" stroke-width="1.2"/></svg>
          </button>
          <button class="tb-btn" title="最大化 / 还原" @click="winMaximize">
            <svg width="11" height="11" viewBox="0 0 12 12"><rect x="1.5" y="1.5" width="9" height="9" rx="1" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>
          </button>
          <button class="tb-btn tb-close" title="关闭" @click="winClose">
            <svg width="12" height="12" viewBox="0 0 12 12"><line x1="1.5" y1="1.5" x2="10.5" y2="10.5" stroke="currentColor" stroke-width="1.2"/><line x1="10.5" y1="1.5" x2="1.5" y2="10.5" stroke="currentColor" stroke-width="1.2"/></svg>
          </button>
        </div>
      </div>

      <!-- 侧边栏（绝对定位 overlay，不参与文档流） -->
      <aside class="sidebar" :class="{ collapsed: sidebarCollapsed }">
        <div class="sidebar-head">
          <span class="sb-title">💬 会话</span>
        </div>
        <button class="sidebar-new" @click="openNewSession">＋ 新建会话</button>
        <div class="sidebar-body">
          <div
            v-for="s in sessions"
            :key="s"
            class="sp-item"
            :class="{ active: s === sessionId }"
            @click="selectSession(s)"
          >
            <span class="sp-ico">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
            </span>
            <span class="sp-body">
              <span class="sp-name">{{ sessionNames[s] || s }}<span v-if="checkpoints[s]" class="cp-badge">断点</span></span>
              <span class="sp-time">{{ s }}</span>
            </span>
            <span class="sp-ops">
              <span class="sp-op" title="重命名" @click.stop="renameSession(s)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.83 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z"/></svg>
              </span>
              <span class="sp-op" title="Fork 分支" @click.stop="forkSession(s)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>
              </span>
              <span class="sp-op" title="断点续跑" @click.stop="resumeSession(s)">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>
              </span>
              <span class="sp-del" title="删除会话" @click.stop="deleteSession(s)">✕</span>
            </span>
          </div>
          <p v-if="!sessions.length" class="sp-empty">暂无会话</p>
        </div>
        <div class="sidebar-foot">
          <button class="sb-foot-btn" title="搜索" @click="searchOpen = true">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
            搜索
          </button>
          <button class="sb-foot-btn" title="微信" @click="showWechat = true">
            <span class="wx-dot" :class="{ on: wechatOnline }"></span>
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M8.69 4C4.86 4 1.75 6.57 1.75 9.75c0 1.78.9 3.38 2.33 4.47l-.66 2.05a.35.35 0 0 0 .52.4l2.36-1.36c.74.2 1.52.3 2.39.3h.22c-.06-.4-.1-.82-.1-1.24 0-3.22 3.04-5.86 6.87-5.86.2 0 .4.01.6.02C15.45 6.1 12.4 4 8.69 4zm-2.2 3.5a.83.83 0 1 1 0 1.66.83.83 0 0 1 0-1.66zm4.75 0a.83.83 0 1 1 0 1.66.83.83 0 0 1 0-1.66zM18.5 9.5c-3.13 0-5.75 2.28-5.75 5.25S15.37 20 18.5 20c.77 0 1.5-.14 2.16-.38l1.55.89a.28.28 0 0 0 .42-.32l-.53-1.64c1.28-.93 2.15-2.3 2.15-3.8 0-2.97-2.62-5.25-5.75-5.25zm-2 4.5a.68.68 0 1 1 0 1.36.68.68 0 0 1 0-1.36zm4 0a.68.68 0 1 1 0 1.36.68.68 0 0 1 0-1.36z"/></svg>
            微信
          </button>
          <button class="sb-foot-btn" title="设置" @click="showSettings = true">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
            设置
          </button>
        </div>
      </aside>

      <!-- 顶部栏（精简：搜索 + 导出，微信/设置已迁入侧边栏） -->
      <div class="top-bar">
        <div class="top-left">
          <button class="mem-btn top-icon" title="搜索历史对话" @click="searchOpen = true">🔍</button>
          <button class="mem-btn top-icon" title="导出当前会话" :disabled="exporting" @click="exportSession">📤</button>
        </div>
        <div class="status-right">
        </div>
      </div>

      <!-- 消息区 -->
      <main ref="msgsRef" class="msgs" @scroll="onMsgsScroll">
        <div v-for="m in displayMessages" :key="m.id" class="msg" :class="m.role === 'user' ? 'user' : 'ai'">
          <div class="msg-wrap">
            <div class="meta">
              <span>{{ m.role === 'user' ? '你' : 'ClawDesk' }} · [{{ fmtTs(m.timestamp) }}]</span>
              <span class="msg-ops">
                <button class="mo-btn" title="复制消息" @click.stop="copyMessage(m)">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                </button>
                <template v-if="m.role === 'assistant'">
                  <button class="mo-btn" title="朗读" @click.stop="speakMessage(m)">🔊</button>
                  <button class="mo-btn" title="重新生成" @click.stop="regenerate(m)">🔄</button>
                </template>
                <template v-else>
                  <button class="mo-btn" title="编辑重发" @click.stop="editResend(m)">✏️</button>
                </template>
              </span>
            </div>
            <div class="bubble">
              <div v-if="m.thinking !== undefined && m.thinking !== null" class="thinking-block">
                <button class="thinking-toggle" @click="m.thinkingOpen = !m.thinkingOpen">
                  {{ m.thinkingOpen ? '▾' : '▸' }} 💭 思考中
                </button>
                <div v-show="m.thinkingOpen" class="thinking-content">{{ m.thinking }}</div>
              </div>
              <!-- 工具调用：统一为紧凑卡片（默认一行，点击展开详情；运行中自动展开） -->
              <template v-for="(tc, tci) in m.toolCalls ?? []" :key="`${m.id}-${tc.toolId}-${tci}`">
                <div class="tool-card" :class="{ pending: tc.status === 'running' }" :data-idx="tci">
                  <div class="tc-head" @click="tc.open = !tc.open">
                    <span class="tc-fold" :class="{ 'tc-fold-on': tc.open || tc.status === 'running' }">{{ tc.open || tc.status === 'running' ? '▾' : '▸' }}</span>
                    <span class="t" :class="tc.status === 'success' ? 't-ok' : (tc.status === 'error' || tc.status === 'danger') ? 't-err' : 't-run'">
                      {{ tc.status === 'success' ? '✓' : tc.status === 'error' ? '✗' : tc.status === 'danger' ? '⚠' : '⋯' }}
                    </span>
                    <span class="tc-id">{{ tc.toolId }}</span>
                    <span class="tc-sum">{{ toolSummary(tc) }}</span>
                    <span v-if="hasToolDetail(tc)" class="tc-hint">{{ tc.open || tc.status === 'running' ? '收起' : '详情' }}</span>
                  </div>
                  <div v-show="tc.open || tc.status === 'running'" class="tc-body">
                    <!-- terminal 工具：命令 + 输出 + 退出码 -->
                    <template v-if="isTerminal(tc)">
                      <div v-if="termInfo(tc).cmd" class="tc-cmd"><span class="tc-ps">PS&gt;</span>{{ termInfo(tc).cmd }}</div>
                      <pre v-if="termInfo(tc).stdout" class="tc-out">{{ termInfo(tc).stdout }}</pre>
                      <pre v-if="termInfo(tc).stderr" class="tc-out err">{{ termInfo(tc).stderr }}</pre>
                      <pre v-if="tc.error" class="tc-out err">{{ tc.error }}</pre>
                      <div v-if="termInfo(tc).exitCode !== null" class="tc-exit" :class="termInfo(tc).exitCode === 0 ? 'ok' : 'err'">
                        {{ termInfo(tc).exitCode === 0 ? '✓ 退出码 0' : '✗ 退出码 ' + termInfo(tc).exitCode }}
                      </div>
                    </template>
                    <!-- 其他工具：参数 + 输出 -->
                    <template v-else>
                      <div v-if="hasArgs(tc.arguments)" class="tc-cmd">{{ tc.toolId }} {{ fmtArgs(tc.arguments) }}</div>
                      <div v-if="tc.output" class="tc-out">{{ fmtOutput(tc.output) }}</div>
                      <div v-if="tc.error" class="tc-out err">{{ tc.error }}</div>
                    </template>
                    <span v-if="tc.status === 'running'" class="term-cursor"></span>
                  </div>
                </div>
              </template>
              <!-- 用户上传图片 / 消息内图片：缩略图展示，点击放大浏览 -->
              <div v-if="m.images && m.images.length" class="msg-images">
                <img
                  v-for="(img, i) in m.images"
                  :key="i"
                  :src="img"
                  class="msg-img"
                  alt="图片"
                  loading="lazy"
                  @click.stop="openImageViewer(m.images as string[], i)"
                />
              </div>
              <div v-if="m.content" class="msg-content" v-html="m.role === 'assistant' ? renderMd(m.content) : escapeHtml(m.content)"></div>
            </div>
          </div>
        </div>
      </main>

      <!-- 底部输入（v6：模型标签 + Agent + 附件 + 进度环 + 发送） -->
      <BottomInput
        ref="bottomInputRef"
        :running="running"
        :models="MODELS"
        :selected-model="selectedModel"
        @send="handleSend"
        @cancel="handleCancel"
        @select-model="selectModel"
      />
    </div>

    <!-- 设置弹窗（v6 左侧标签列） -->
    <SettingsView
      v-if="showSettings"
      :tz="tz"
      @close="showSettings = false"
      @keys="onKeysSaved"
      @tz="onTzChange"
    />

    <!-- 微信 Bot 面板 -->
    <WechatPanel v-if="showWechat" @close="showWechat = false" />

    <!-- 搜索历史弹窗 -->
    <div class="perm-overlay" :class="{ open: searchOpen }">
      <div class="perm-card" style="width:min(560px,92vw)">
        <div class="pc-title">🔍 搜索历史对话</div>
        <div class="search-bar">
          <input
            v-model="searchKeyword"
            class="ns-input"
            placeholder="输入关键词，跨会话检索历史消息…"
            @keydown.enter="doSearch"
          />
          <button class="pc-yes" @click="doSearch" :disabled="searching">{{ searching ? "搜索中…" : "搜索" }}</button>
        </div>
        <div class="search-list">
          <div v-for="(r, i) in searchResults" :key="i" class="search-item" @click="jumpToResult(r.sessionId)">
            <div class="search-meta">{{ r.sessionId }} · {{ r.role }}</div>
            <div class="search-content">{{ r.content }}</div>
          </div>
          <p v-if="!searching && searchResults.length === 0 && searchKeyword" class="sp-empty">未找到匹配内容</p>
        </div>
        <div class="pc-actions">
          <button class="pc-no" @click="searchOpen = false">关闭</button>
        </div>
      </div>
    </div>

    <!-- 新建会话弹窗 -->
    <div class="perm-overlay" :class="{ open: newSessionOpen }">
      <div class="perm-card">
        <div class="pc-title">新建会话</div>
        <div class="pc-sub">为新的对话输入一个名称（可留空自动命名）</div>
        <input
          v-model="newSessionName"
          class="ns-input"
          placeholder="会话名称"
          maxlength="60"
          @keydown.enter="confirmNewSession"
        />
        <div class="pc-actions">
          <button class="pc-no" @click="newSessionOpen = false">取消</button>
          <button class="pc-yes" @click="confirmNewSession">创建</button>
        </div>
      </div>
    </div>

    <!-- 权限确认弹窗 -->
    <div class="perm-overlay" :class="{ open: !!permRequest }">
      <div class="perm-card">
        <div class="pc-ico">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>
        </div>
        <div class="pc-title">Agent 请求执行工具</div>
        <div class="pc-sub">逐步确认模式 · 请审批以下操作</div>
        <div class="pc-tool">{{ permRequest?.toolId }}</div>
        <div class="pc-args">{{ permRequest?.args }}</div>
        <div class="pc-actions">
          <button class="pc-no" @click="denyPermission">拒绝</button>
          <button class="pc-yes" @click="approvePermission">允许</button>
        </div>
      </div>
    </div>

    <!-- 图片查看器（点击图片放大浏览，←/→ 切换，Esc 关闭） -->
    <div v-if="imageViewer" class="img-overlay" @click.self="imageViewer = null">
      <div class="img-viewer">
        <button class="iv-close" title="关闭 (Esc)" @click="imageViewer = null">✕</button>
        <button v-if="imageViewer.list.length > 1" class="iv-nav iv-prev" title="上一张 (←)" @click="ivPrev">‹</button>
        <img :src="imageViewer.list[imageViewer.index]" class="iv-img" alt="图片预览" />
        <button v-if="imageViewer.list.length > 1" class="iv-nav iv-next" title="下一张 (→)" @click="ivNext">›</button>
        <div v-if="imageViewer.list.length > 1" class="iv-count">{{ imageViewer.index + 1 }} / {{ imageViewer.list.length }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
</style>
