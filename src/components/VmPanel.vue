<script setup lang="ts">
/**
 * 虚拟机微信面板 —— 真微信内置（VirtualBox + VNC 屏幕流内嵌）。
 * 虚拟机（AI-WeChat）里跑 Windows 11 + 微信，画面实时内嵌在面板中，
 * 可直接点击/输入/粘贴操作，AI 也可通过 vm_* 工具操作。
 */
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const emit = defineEmits<{ close: [] }>();

const vms = ref<{ name: string; uuid: string; running: boolean }[]>([]);
const vmTip = ref("");
const connected = ref(false);
const connTip = ref("");
const frameUrl = ref("");
const fbW = ref(0);
const fbH = ref(0);
const pasteText = ref("");
const pasteTip = ref("");
const fullscreen = ref(false);
const wlUsers = ref("");
const wlTip = ref("");

/** 读取白名单。 */
async function loadWhitelist(): Promise<void> {
  try {
    const r = await invoke<{ users: string[] }>("vm_whitelist_get");
    wlUsers.value = (r.users ?? []).join("，");
  } catch { /* 静默 */ }
}

/** 保存白名单（AI 只能给名单里的人发消息）。 */
async function saveWhitelist(): Promise<void> {
  wlTip.value = "保存中…";
  try {
    const r = await invoke<{ users: string[] }>("vm_whitelist_set", { users: wlUsers.value });
    wlTip.value = r.users.length ? `✅ 已保存 ${r.users.length} 位可聊天对象` : "✅ 白名单已清空（AI 发送会被拒绝）";
  } catch (e) {
    wlTip.value = `保存失败：${String(e)}`;
  }
}

let unlistenFrame: UnlistenFn | null = null;
let unlistenStatus: UnlistenFn | null = null;

async function refreshVms(): Promise<void> {
  vmTip.value = "扫描中…";
  try {
    const r = await invoke<{ vms: any[] }>("vm_list_vms");
    vms.value = (r.vms ?? []).map((v) => ({
      name: v.name,
      uuid: v.uuid,
      running: !!v.running,
    }));
    vmTip.value = `找到 ${vms.value.length} 台虚拟机`;
  } catch (e) {
    vmTip.value = `扫描失败：${String(e)}`;
  }
}

/** 一键打开：启动虚拟机（如未运行）→ 等系统启动 → 自动连接屏幕。 */
const opening = ref(false);
async function openVm(): Promise<void> {
  if (opening.value) return;
  opening.value = true;
  vmTip.value = "正在启动虚拟机…";
  try {
    await refreshVms();
    const vm = vms.value.find((v) => v.name === "AI-WeChat") ?? vms.value[0];
    if (!vm) {
      vmTip.value = "未找到虚拟机（AI-WeChat），请检查 VirtualBox 是否安装";
      return;
    }
    if (!vm.running) {
      await invoke("vm_power", { name: vm.name, action: "start" });
      vmTip.value = "虚拟机启动中…（Windows 开机约 1~3 分钟，请稍候）";
      // 轮询等待运行
      for (let i = 0; i < 30; i++) {
        await new Promise((r) => setTimeout(r, 6000));
        try {
          const s = await invoke<{ vms: any[] }>("vm_list_vms");
          const v = (s.vms ?? []).find((x) => x.name === vm.name);
          if (v?.running) break;
        } catch { /* 忽略 */ }
      }
    } else {
      vmTip.value = "虚拟机已在运行，连接屏幕中…";
    }
    // 等系统启动 + TightVNC 就绪
    for (let i = 0; i < 20; i++) {
      await new Promise((r) => setTimeout(r, 6000));
      const ok = await invoke<{ connected: boolean }>("vm_connect", {}).catch(() => null);
      if (ok?.connected) break;
    }
    vmTip.value = "✅ 已连接虚拟机屏幕（微信登录窗口可直接扫码）";
    void snapshot();
  } catch (e) {
    vmTip.value = `打开失败：${String(e)}`;
  } finally {
    opening.value = false;
  }
}

async function powerVm(name: string, action: "start" | "stop"): Promise<void> {
  vmTip.value = action === "start" ? "启动中…（Windows 启动约 1~3 分钟）" : "关机中…";
  try {
    await invoke("vm_power", { name, action });
    vmTip.value = action === "start" ? "✅ 已启动，等待系统进入桌面后点「连接」" : "✅ 已发送关机指令";
  } catch (e) {
    vmTip.value = `操作失败：${String(e)}`;
  }
  setTimeout(() => void refreshVms(), 8000);
}

async function connect(): Promise<void> {
  connTip.value = "连接中…";
  try {
    const r = await invoke<{ connected: boolean; desktop: string }>("vm_connect", {});
    connected.value = r.connected;
    connTip.value = r.connected ? `✅ 已连接（${r.desktop}）` : "连接失败";
    void snapshot();
  } catch (e) {
    connTip.value = `连接失败：${String(e)}`;
  }
}

async function disconnect(): Promise<void> {
  try {
    await invoke("vm_disconnect");
    connected.value = false;
    connTip.value = "已断开";
    frameUrl.value = "";
  } catch (e) {
    connTip.value = `断开失败：${String(e)}`;
  }
}

async function snapshot(): Promise<void> {
  if (!connected.value) return;
  try {
    const r = await invoke<{ dataUrl: string; width: number; height: number }>("vm_screenshot");
    if (r.dataUrl) {
      frameUrl.value = r.dataUrl;
      fbW.value = r.width;
      fbH.value = r.height;
    }
  } catch { /* 静默 */ }
}

/** 画面点击 → 鼠标按下+松开。 */
async function onCanvasClick(e: MouseEvent): Promise<void> {
  if (!connected.value) return;
  const el = e.currentTarget as HTMLElement;
  const rect = el.getBoundingClientRect();
  const x = Math.round(((e.clientX - rect.left) / rect.width) * fbW.value);
  const y = Math.round(((e.clientY - rect.top) / rect.height) * fbH.value);
  try {
    await invoke("vm_pointer", { x, y, buttons: 1 });
    await invoke("vm_pointer", { x, y, buttons: 0 });
  } catch (err) {
    connTip.value = `点击失败：${String(err)}`;
  }
}

/** 画布键盘输入（需先聚焦画布）。 */
async function onCanvasKey(e: KeyboardEvent): Promise<void> {
  if (!connected.value) return;
  const special: Record<string, number> = {
    Enter: 0xFF0D,
    Escape: 0xFF1B,
    Tab: 0xFF09,
    Backspace: 0xFF08,
    ArrowUp: 0xFF52,
    ArrowDown: 0xFF54,
    ArrowLeft: 0xFF51,
    ArrowRight: 0xFF53,
    Home: 0xFF50,
    End: 0xFF57,
    Delete: 0xFFFF,
    " ": 0x20,
    Control: 0xFFE3,
    Shift: 0xFFE1,
    Alt: 0xFFE9,
  };
  const mods: { key: string; ks: number }[] = [];
  if (e.ctrlKey) mods.push({ key: "Control", ks: 0xFFE3 });
  if (e.shiftKey) mods.push({ key: "Shift", ks: 0xFFE1 });
  if (e.altKey) mods.push({ key: "Alt", ks: 0xFFE9 });
  let main = special[e.key] ?? null;
  if (main === null && e.key.length === 1) main = e.key.charCodeAt(0);
  if (main === null) return;
  try {
    for (const m of mods) {
      await invoke("vm_key", { keysym: m.ks, down: true });
    }
    await invoke("vm_key", { keysym: main, down: true });
    await invoke("vm_key", { keysym: main, down: false });
    for (const m of mods.slice().reverse()) {
      await invoke("vm_key", { keysym: m.ks, down: false });
    }
  } catch (err) {
    connTip.value = `按键失败：${String(err)}`;
  }
}

async function doPaste(): Promise<void> {
  if (!connected.value || !pasteText.value) return;
  pasteTip.value = "粘贴中…";
  try {
    await invoke("vm_paste", { text: pasteText.value });
    await new Promise((r) => setTimeout(r, 120));
    await invoke("vm_key", { keysym: 0xFFE3, down: true });
    await invoke("vm_key", { keysym: 0x76, down: true });
    await invoke("vm_key", { keysym: 0x76, down: false });
    await invoke("vm_key", { keysym: 0xFFE3, down: false });
    pasteTip.value = `✅ 已粘贴 ${pasteText.value.length} 字（Ctrl+V）`;
  } catch (e) {
    pasteTip.value = `粘贴失败：${String(e)}`;
  }
}

onMounted(async () => {
  // 启动画面流（独立后台线程，与 VNC 连接无关，打开面板即实时画面）
  void invoke("vm_start_frame_stream").catch(() => {});
  await refreshVms();
  void loadWhitelist();
  unlistenFrame = await listen<any>("vm://frame", (e) => {
    const p = e.payload;
    if (p?.dataUrl) {
      frameUrl.value = p.dataUrl;
      fbW.value = p.width;
      fbH.value = p.height;
    }
  });
  unlistenStatus = await listen<any>("vm://status", (e) => {
    if (e.payload?.connected === false) {
      connected.value = false;
      connTip.value = `⚠️ ${e.payload?.reason ?? "连接断开"}`;
    }
  });
  const s = await invoke<{ connected: boolean; width: number; height: number }>("vm_status").catch(() => null);
  connected.value = !!s?.connected;
  if (s?.width) {
    fbW.value = s.width;
    fbH.value = s.height;
    void snapshot();
  }
});

onUnmounted(() => {
  unlistenFrame?.();
  unlistenStatus?.();
  // ★ 关闭面板即停止画面流，避免后台持续截图占用 CPU（重新打开会自动重启）
  void invoke("vm_stop_frame_stream").catch(() => {});
});
</script>

<template>
  <div class="vm-overlay" :class="{ fullscreen }">
    <div class="vm-card">
      <div class="vm-head">
        <div class="vm-title">
          <span>🖥️</span>
          <span>虚拟机内置微信（真微信跑在虚拟机里，与本机隔离）</span>
        </div>
        <div style="display:flex; gap:6px;">
          <button class="vm-close" title="全屏 / 退出全屏（随时返回）" @click="fullscreen = !fullscreen">{{ fullscreen ? "⤢ 退出全屏" : "⤡ 全屏" }}</button>
          <button class="vm-close" title="关闭面板（虚拟机继续后台运行）" @click="emit('close')">✕</button>
        </div>
      </div>

      <div class="vm-toolbar">
        <button class="vm-btn big" :disabled="opening" @click="openVm">
          {{ opening ? "⏳ 正在打开虚拟机…" : "🚀 一键打开虚拟机微信" }}
        </button>
        <button v-for="v in vms" :key="v.uuid" class="vm-btn" :class="{ primary: v.running }" @click="powerVm(v.name, v.running ? 'stop' : 'start')">
          {{ v.name }}：{{ v.running ? "运行中·点此关机" : "已关机·点此启动" }}
        </button>
        <button v-if="!vms.length" class="vm-btn" @click="refreshVms">🔄 刷新虚拟机列表</button>
        <span class="vm-tip">{{ vmTip }}</span>
        <span class="vm-spacer"></span>
        <button v-if="!connected" class="vm-btn primary" @click="connect">🔗 连接屏幕</button>
        <template v-else>
          <button class="vm-btn" @click="disconnect">⏹ 断开</button>
          <button class="vm-btn" @click="snapshot">📸 截图</button>
        </template>
        <span class="vm-tip">{{ connTip }}</span>
      </div>

      <div class="vm-screen-wrap">
        <img
          v-if="frameUrl"
          :src="frameUrl"
          class="vm-canvas"
          :class="{ connected }"
          tabindex="0"
          @click="onCanvasClick"
          @keydown="onCanvasKey"
        />
        <div v-else class="vm-empty">
          <p v-if="!connected">尚未连接虚拟机屏幕。</p>
          <p v-else>等待画面…（虚拟机启动中请稍候，或在虚拟机内确认 TightVNC 已运行）</p>
          <p class="vm-hint">步骤：启动虚拟机 → 等 Windows 进入桌面 → 「连接屏幕」→ 即可看到真微信界面</p>
        </div>
      </div>

      <div class="vm-foot">
        <div class="vm-paste">
          <input
            v-model="pasteText"
            class="vm-input"
            placeholder="输入要粘贴到虚拟机的文本（支持中文，自动 Ctrl+V）…"
            @keydown.enter.prevent="doPaste"
          />
          <button class="vm-btn primary" :disabled="!connected || !pasteText" @click="doPaste">📋 粘贴</button>
          <span class="vm-tip">{{ pasteTip }}</span>
        </div>
        <div class="vm-paste" style="margin-top:6px;">
          <input
            v-model="wlUsers"
            class="vm-input"
            placeholder="可聊天对象白名单（AI 只能给这些人发消息，逗号分隔；留空 = AI 不能发）"
            @keydown.enter.prevent="saveWhitelist"
          />
          <button class="vm-btn primary" @click="saveWhitelist">🔒 保存白名单</button>
          <span class="vm-tip">{{ wlTip }}</span>
        </div>
        <p class="vm-hint">
          操作说明：点击画面 = 鼠标点击；先点画面获得焦点后可直接键盘输入；AI 也可通过对话（vm_screenshot / vm_click / vm_type / vm_key / vm_send）操作虚拟机里的微信。全屏模式像独立系统一样使用，点「退出全屏」随时返回。
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.vm-overlay {
  position: fixed;
  inset: 0;
  z-index: 300;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  backdrop-filter: blur(4px);
}
.vm-overlay.fullscreen {
  background: #000;
  backdrop-filter: none;
  padding: 0;
}
.vm-overlay.fullscreen .vm-card {
  width: 100vw;
  height: 100vh;
  border-radius: 0;
  border: none;
}
.vm-overlay.fullscreen .vm-canvas {
  max-width: 100vw;
  max-height: calc(100vh - 140px);
}
.vm-card {
  width: min(1080px, 94vw);
  height: min(700px, 90vh);
  background: linear-gradient(180deg, #1b2233 0%, #141a28 100%);
  border: 1px solid #2c3a55;
  border-radius: 14px;
  display: flex;
  flex-direction: column;
  box-shadow: 0 18px 60px rgba(0, 0, 0, 0.55);
  overflow: hidden;
}
.vm-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 13px 18px;
  border-bottom: 1px solid #26324a;
  flex-shrink: 0;
}
.vm-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 600;
  color: #e8edf7;
}
.vm-close {
  background: none;
  border: none;
  color: #94a3b8;
  font-size: 15px;
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 6px;
}
.vm-close:hover { background: #26324a; color: #fff; }
.vm-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 14px;
  border-bottom: 1px solid #26324a;
  flex-shrink: 0;
  flex-wrap: wrap;
}
.vm-btn {
  background: #26324a;
  border: 1px solid #2c3a55;
  color: #e8edf7;
  border-radius: 8px;
  padding: 6px 12px;
  font-size: 12px;
  cursor: pointer;
}
.vm-btn:hover { border-color: #3b82f6; }
.vm-btn.primary { background: #1d4ed8; border-color: #2563eb; }
.vm-btn:disabled { opacity: 0.5; cursor: default; }
.vm-spacer { flex: 1; }
.vm-tip { font-size: 11px; color: #94a3b8; }
.vm-screen-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #0d1117;
  overflow: hidden;
}
.vm-canvas {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  image-rendering: auto;
}
.vm-canvas.connected { cursor: crosshair; }
.vm-empty {
  color: #64748b;
  text-align: center;
  font-size: 13px;
}
.vm-hint { color: #64748b; font-size: 11px; }
.vm-foot {
  padding: 10px 14px;
  border-top: 1px solid #26324a;
  flex-shrink: 0;
}
.vm-paste { display: flex; align-items: center; gap: 8px; }
.vm-input {
  flex: 1;
  background: #1b2436;
  border: 1px solid #2c3a55;
  border-radius: 8px;
  color: #e8edf7;
  font-size: 12px;
  padding: 8px 10px;
  outline: none;
}
.vm-input:focus { border-color: #3b82f6; }
.vm-foot .vm-hint { margin: 8px 0 0; line-height: 1.6; }
</style>
