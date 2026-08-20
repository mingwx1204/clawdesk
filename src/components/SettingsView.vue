<script setup lang="ts">
import { onMounted, ref } from "vue";
import { settingsApi, routerApi } from "../utils/api";

/**
 * 设置面板（极简版）：
 * 模型配置 + 外观（主题/不透明度/字号）—— 填 Key / 选模型 / 保存。
 * Agent、思考、自进化均已默认开启（后端 hardcode），不再在界面展示开关。
 */

defineProps<{ tz?: string }>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "keys", keys: { main?: string; vision?: string; image?: string }): void;
  (e: "tz", v: string): void;
  (e: "appearance", s: AppSettings): void;
}>();

// ── 设置结构（与后端 AppSettings camelCase 逐字段镜像，模型 + 外观）──
interface AppSettings {
  model: string;
  modelEndpoint: string;
  visionModel: string;
  visionEndpoint: string;
  imageModel: string;
  imageEndpoint: string;
  // 外观（后端已持久化，这里补充类型与控件）
  darkTheme: boolean;
  uiOpacity: number;
  fontSize: number;
}

const settings = ref<AppSettings | null>(null);
const error = ref("");
const tip = ref("");
const saving = ref(false);

// API Key（仅内存态，不持久化 —— 与后端安全红线一致）
const mainKey = ref("");
const visionKey = ref("");
const imageKey = ref("");
const keysSavedTip = ref("");

// ── Key 余额追踪（后端 check_balance，根据端点自动识别提供商）──
const mainBalanceText = ref("");
const mainBalanceLoading = ref(false);

// ── 自动检索 Key 支持哪些模型（list_models 命令）──
const detectModelsText = ref("");
const detectModelsLoading = ref(false);
const detectedModels = ref<string[]>([]);

async function detectModels() {
  const key = mainKey.value.trim();
  const endpoint = settings.value?.modelEndpoint ?? "";
  if (!key) {
    detectModelsText.value = "请先填写主模型 Key";
    return;
  }
  detectModelsLoading.value = true;
  detectModelsText.value = "正在检索可用模型…";
  detectedModels.value = [];
  try {
    const r = await routerApi.listModels(key, endpoint);
    const arr: any[] = Array.isArray(r?.models) ? r.models : [];
    detectedModels.value = arr
      .map((m) => (typeof m.id === "string" ? m.id : ""))
      .filter((id) => id);
    detectModelsText.value = `检测到 ${detectedModels.value.length} 个模型（${r?.provider ?? ""}）`;
  } catch (e) {
    detectModelsText.value = `检测失败：${typeof e === "string" ? e : JSON.stringify(e)}`;
  } finally {
    detectModelsLoading.value = false;
  }
}

async function checkBalance() {
  const key = mainKey.value.trim();
  const endpointUrl = settings.value?.modelEndpoint ?? "";
  if (!key) {
    mainBalanceText.value = "请先填写主模型 Key";
    return;
  }
  mainBalanceLoading.value = true;
  mainBalanceText.value = "查询中…";
  try {
    const r = await routerApi.checkBalance(key, endpointUrl);
    const infos: any[] = Array.isArray(r?.balance_infos) ? r.balance_infos : [];
    if (r?.is_available === false) {
      mainBalanceText.value = "账户不可用";
    } else if (infos.length) {
      mainBalanceText.value = infos
        .map(
          (b) =>
            `${b.currency} ${b.total_balance}（赠额 ${b.granted_balance ?? 0} / 充值 ${b.topped_up_balance ?? 0}）`
        )
        .join("；");
    } else {
      mainBalanceText.value = "未获取到余额信息";
    }
  } catch (e) {
    mainBalanceText.value = `查询失败：${typeof e === "string" ? e : JSON.stringify(e)}`;
  } finally {
    mainBalanceLoading.value = false;
  }
}

async function load() {
  try {
    settings.value = await settingsApi.get();
  } catch (e) {
    error.value = `加载设置失败：${String(e)}`;
  }
}

async function loadKeys(): Promise<void> {
  try {
    const k = await settingsApi.getKeys();
    if (k?.main) mainKey.value = k.main;
    if (k?.vision) visionKey.value = k.vision;
    if (k?.image) imageKey.value = k.image;
  } catch { /* 静默 */ }
}

/** 局部更新：将字段补丁发送后端，成功后用返回的最新设置回填。 */
async function patch(p: Record<string, unknown>): Promise<void> {
  saving.value = true;
  tip.value = "";
  try {
    const updated = await settingsApi.set(p);
    settings.value = updated;
    emit("appearance", updated); // 主题/不透明度/字号即时同步到主界面
    tip.value = "✅ 已保存（即时生效）";
  } catch (e) {
    tip.value = `❌ 保存失败：${String(e)}`;
  } finally {
    saving.value = false;
  }
}

function field(key: keyof AppSettings, value: unknown): void {
  void patch({ [key]: value });
}

async function saveKeys(): Promise<void> {
  const m = mainKey.value.trim();
  const v = visionKey.value.trim();
  const i = imageKey.value.trim();
  if (!m && !v && !i) {
    keysSavedTip.value = "请至少填写一个 Key";
    setTimeout(() => { keysSavedTip.value = ""; }, 2000);
    return;
  }
  try {
    const p: Record<string, unknown> = {};
    if (m) p.mainKey = m;
    if (v) p.visionKey = v;
    if (i) p.imageKey = i;
    settings.value = await settingsApi.set(p);
    emit("keys", { main: m, vision: v, image: i });
    keysSavedTip.value = "✅ Key 已保存";
    setTimeout(() => { keysSavedTip.value = ""; }, 2500);
    mainKey.value = "";
    visionKey.value = "";
    imageKey.value = "";
  } catch (e) {
    keysSavedTip.value = `❌ 保存失败：${String(e)}`;
  }
}

onMounted(async () => {
  await load();
  await loadKeys();
});
</script>

<template>
  <div class="settings-overlay open" @click.self="emit('close')">
    <div class="settings-card">
      <header class="sc-header">
        <h3>设置</h3>
        <button class="sc-close" @click="emit('close')">✕</button>
      </header>

      <div class="sc-main">
        <div class="sc-body">
          <p v-if="error" class="sc-error">{{ error }}</p>
          <p v-if="!settings && !error" class="sc-loading">加载中…</p>

          <div v-if="settings" class="sc-group active">
            <h4>主模型</h4>
            <p class="sc-desc">文本推理 / 规划 / 工具选择走主模型（思考 + Agent 已默认开启）</p>
            <label class="sc-label">模型</label>
            <select :value="settings.model" class="sc-select" @change="field('model', ($event.target as HTMLSelectElement).value)">
              <optgroup v-if="detectedModels.length" label="✅ 该 Key 可用（自动检测）">
                <option v-for="m in detectedModels" :key="m" :value="m">{{ m }}</option>
              </optgroup>
              <optgroup label="常用模型（未检测时手动选）">
                <option value="deepseek-v4-pro">deepseek-v4-pro（DeepSeek V4 Pro）</option>
                <option value="deepseek-v4-flash">deepseek-v4-flash（DeepSeek V4 Flash）</option>
                <option value="deepseek-chat">deepseek-chat（DeepSeek-V3 对话）</option>
                <option value="deepseek-reasoner">deepseek-reasoner（DeepSeek-R1 思考）</option>
              </optgroup>
            </select>
            <label class="sc-label">API 地址</label>
            <input :value="settings.modelEndpoint" class="sc-input" @change="field('modelEndpoint', ($event.target as HTMLInputElement).value)" />

            <h4>API Key</h4>
            <input v-model="mainKey" type="password" class="sc-input" placeholder="主模型 Key（如 sk-…）" />
            <div class="sc-balance-row">
              <button class="sc-btn" :disabled="detectModelsLoading" @click="detectModels">
                {{ detectModelsLoading ? "检测中…" : "🔍 检测支持的模型" }}
              </button>
              <button class="sc-btn" :disabled="mainBalanceLoading" @click="checkBalance">
                {{ mainBalanceLoading ? "查询中…" : "查询余额" }}
              </button>
            </div>
            <span v-if="detectModelsText" class="sc-balance" :class="{ err: detectModelsText.startsWith('检测失败') }" style="display:block;margin-top:4px">{{ detectModelsText }}</span>
            <span v-if="mainBalanceText" class="sc-balance" :class="{ err: mainBalanceText.startsWith('查询失败') || mainBalanceText.startsWith('账户不可用') }" style="display:block;margin-top:4px">{{ mainBalanceText }}</span>

            <div class="sc-balance-row" style="margin-top:6px">
              <button class="sc-btn" style="background:#4caf50;border-color:#4caf50" @click="saveKeys">💾 保存 Key</button>
              <span v-if="keysSavedTip" style="color:#4caf50;font-size:12px">{{ keysSavedTip }}</span>
              <span v-if="tip" style="color:var(--accent);font-size:12px">{{ tip }}</span>
            </div>

            <h4 style="margin-top:14px">视觉模型（识图 · 可忽略）</h4>
            <p class="sc-desc">analyze_image 路由至视觉专用模型，未配置时模型会尝试直接用图片输入</p>
            <label class="sc-label">模型</label>
            <input :value="settings.visionModel" class="sc-input" @change="field('visionModel', ($event.target as HTMLInputElement).value)" />
            <label class="sc-label">API 地址</label>
            <input :value="settings.visionEndpoint" class="sc-input" @change="field('visionEndpoint', ($event.target as HTMLInputElement).value)" />
            <input v-model="visionKey" type="password" class="sc-input" placeholder="视觉模型 Key（可留空）" style="margin-top:6px" />

            <h4 style="margin-top:14px">绘图模型（生图 · 可忽略）</h4>
            <p class="sc-desc">generate_image 路由至绘图 API，未配置时自动降级</p>
            <label class="sc-label">模型</label>
            <input :value="settings.imageModel" class="sc-input" @change="field('imageModel', ($event.target as HTMLInputElement).value)" />
            <label class="sc-label">API 地址</label>
            <input :value="settings.imageEndpoint" class="sc-input" @change="field('imageEndpoint', ($event.target as HTMLInputElement).value)" />
            <input v-model="imageKey" type="password" class="sc-input" placeholder="绘图 API Key（可留空）" style="margin-top:6px" />

            <h4 style="margin-top:14px">外观</h4>
            <p class="sc-desc">主题 / 界面不透明度 / 字号，保存后立即生效并自动持久化</p>
            <div class="sc-row">
              <span class="sc-row-label">主题</span>
              <div class="sc-theme-switch">
                <button
                  class="sc-theme-btn"
                  :class="{ active: settings.darkTheme }"
                  type="button"
                  @click="field('darkTheme', true)"
                >🌙 深色</button>
                <button
                  class="sc-theme-btn"
                  :class="{ active: !settings.darkTheme }"
                  type="button"
                  @click="field('darkTheme', false)"
                >☀️ 亮色</button>
              </div>
            </div>
            <div class="sc-row">
              <span class="sc-row-label">不透明度</span>
              <input
                type="range"
                min="0.6"
                max="1"
                step="0.05"
                :value="settings.uiOpacity"
                @change="field('uiOpacity', Number(($event.target as HTMLInputElement).value))"
              />
              <span class="sc-range-val">{{ Math.round(settings.uiOpacity * 100) }}%</span>
            </div>
            <div class="sc-row">
              <span class="sc-row-label">字号</span>
              <input
                type="range"
                min="12"
                max="22"
                step="1"
                :value="settings.fontSize"
                @change="field('fontSize', Number(($event.target as HTMLInputElement).value))"
              />
              <span class="sc-range-val">{{ settings.fontSize }}px</span>
            </div>

            <h4 style="margin-top:14px">时区</h4>
            <select :value="tz || 'Asia/Shanghai'" class="sc-select" @change="emit('tz', ($event.target as HTMLSelectElement).value)">
              <option value="Asia/Shanghai">北京（UTC+8）</option>
              <option value="Asia/Tokyo">东京（UTC+9）</option>
              <option value="America/New_York">纽约（UTC-5）</option>
              <option value="America/Los_Angeles">洛杉矶（UTC-8）</option>
              <option value="Europe/London">伦敦（UTC+0）</option>
              <option value="UTC">UTC</option>
            </select>
          </div>

          <div v-if="settings" class="sc-foot">
            <span class="sc-note">✨ 思考模式、Agent（YOLO 全自动）、自进化均已默认开启，无需配置。</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
