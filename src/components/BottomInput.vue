<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invokeTool } from "../core/ToolDispatcher";

/**
 * 底部输入栏（v6 壁纸气泡布局）—— 支持任意文件粘贴 / 拖拽 / 上传（全中文）。
 * 图片 → dataURL 预览；其他文件 → builtin:attachment_save 保存到本地，发送时带路径。
 */

interface AttachItem {
  name: string;
  path: string;
  size: number;
}
interface ModelOption { id: string; label: string; desc: string; }

const props = defineProps<{
  running?: boolean;
  disabled?: boolean;
  currentRound?: number;
  modelLabel?: string;
  models?: ModelOption[];
  selectedModel?: string;
  agentOn?: boolean;
  mode?: string;
  thinking?: boolean;
  ctxPct?: number;
  ctxTokens?: string;
  ctxItems?: { sys: number[]; usr: number[] };
}>();
const emit = defineEmits<{
  (e: "send", content: string, images?: string[], attachments?: string[]): void;
  (e: "cancel"): void;
  (e: "select-model", id: string): void;
  (e: "toggle-agent"): void;
  (e: "set-mode", id: string): void;
  (e: "toggle-thinking"): void;
}>();

const prompt = ref("");
const images = ref<string[]>([]);
const attachments = ref<AttachItem[]>([]);
const fileInput = ref<HTMLInputElement | null>(null);
const imageInput = ref<HTMLInputElement | null>(null);
const attachMenuOpen = ref(false);
const modelMenuOpen = ref(false);
const modeMenuOpen = ref(false);
const promptMenuOpen = ref(false);
const roundNum = computed(() => Math.max(1, props.currentRound ?? 1));

/** 快捷指令库（对标大厂提示词模板）。 */
const PROMPT_TEMPLATES = [
  { label: "✍️ 翻译", prompt: "请将以下内容翻译成中文，保持原意和语气：\n\n" },
  { label: "📝 总结", prompt: "请用简洁的中文总结以下内容的要点：\n\n" },
  { label: "🐛 找 Bug", prompt: "请审查以下代码，找出潜在 bug 并给出修复建议：\n\n" },
  { label: "💡 代码解释", prompt: "请逐段解释以下代码的作用与设计思路：\n\n" },
  { label: "📋 写日报", prompt: "请根据我今天的工作内容生成一份简洁日报：\n\n" },
  { label: "🧠 头脑风暴", prompt: "请针对以下主题给出 5 个有创意的想法：\n\n" },
  { label: "✏️ 润色", prompt: "请润色以下文本，使其更通顺专业：\n\n" },
  { label: "📊 数据分析", prompt: "请分析以下数据并给出结论和建议：\n\n" },
];

/** 供父组件（编辑重发）回填输入框。 */
function setPrompt(text: string) {
  prompt.value = text;
}
defineExpose({ setPrompt });

/** 插入快捷指令模板（追加到输入框末尾）。 */
function insertTemplate(t: { label: string; prompt: string }) {
  promptMenuOpen.value = false;
  prompt.value = (prompt.value ? prompt.value + "\n\n" : "") + t.prompt;
}

/** 截屏提问：调用 window_screenshot 工具截取当前屏幕，加入图片预览。 */
async function captureScreen() {
  attachMenuOpen.value = false;
  try {
    const res = await invokeTool({
      id: `cap-${Date.now()}`,
      toolId: "builtin:window_screenshot",
      arguments: {},
      round: 1,
    });
    if (res.status === "success") {
      const out = (res.output ?? {}) as { dataUrl?: string };
      if (out.dataUrl) {
        images.value.push(out.dataUrl);
        if (images.value.length > 4) images.value.shift();
      } else {
        alert("截图成功但未返回图片数据");
      }
    } else {
      alert(`截图失败：${res.status === "error" ? res.message : res.reason}`);
    }
  } catch (e) {
    alert(`截图失败：${String(e)}`);
  }
}

function closeAttachMenu() { attachMenuOpen.value = false; }
onMounted(() => document.addEventListener("click", closeAttachMenu));
onUnmounted(() => document.removeEventListener("click", closeAttachMenu));

const MODE_OPTIONS = [
  { id: "off", label: "关闭" },
  { id: "plan_only", label: "计划只读" },
  { id: "step_confirm", label: "逐步确认" },
  { id: "yolo", label: "YOLO 全自动" },
];
const MODE_LABELS: Record<string, string> = { off: "关闭", plan_only: "计划只读", step_confirm: "逐步确认", yolo: "YOLO 全自动" };
const modeLabel = computed(() => MODE_LABELS[props.mode ?? "off"] ?? props.mode ?? "关闭");

const MAX_FILE_MB = 20; // 与后端 attachment_save 上限一致

function onSend() {
  if (prompt.value.trim() || images.value.length || attachments.value.length) {
    emit(
      "send",
      prompt.value.trim(),
      images.value.length ? [...images.value] : undefined,
      attachments.value.length ? attachments.value.map((a) => a.path) : undefined
    );
    prompt.value = "";
    images.value = [];
    attachments.value = [];
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    onSend();
  }
}

// ── 图片处理 ──
function fileToDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error("读取文件失败"));
    reader.readAsDataURL(file);
  });
}

async function addFiles(files: FileList | null) {
  if (!files) return;
  // ★ 并行处理（图片读 dataURL + 附件上传互不阻塞），10 个大文件不再逐个排队
  await Promise.all(
    Array.from(files).map(async (f) => {
      if (f.type.startsWith("image/")) {
        if (images.value.length >= 4) return;
        const url = await fileToDataUrl(f);
        images.value.push(url);
      } else {
        await addAttachment(f);
      }
    }),
  );
}

/** 非图片附件：读 base64 → 调用 builtin:attachment_save → 记录路径。 */
async function addAttachment(f: File) {
  if (f.size > MAX_FILE_MB * 1024 * 1024) {
    alert(`附件「${f.name}」超过 ${MAX_FILE_MB}MB 上限，已跳过`);
    return;
  }
  try {
    const dataUrl = await fileToDataUrl(f);
    const b64 = dataUrl.split(",")[1] ?? "";
    const res = await invokeTool({
      id: `att-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      toolId: "builtin:attachment_save",
      arguments: { name: f.name, data: b64 },
      round: 1,
    });
    if (res.status === "success") {
      const out = res.output as { path?: string; name?: string } | undefined;
      const path = out?.path ?? "";
      if (path) attachments.value.push({ name: out?.name ?? f.name, path, size: f.size });
    } else {
      alert(`附件「${f.name}」保存失败：${res.status === "error" ? res.message : res.reason}`);
    }
  } catch (err) {
    alert(`附件「${f.name}」处理失败：${String(err)}`);
  }
}

function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

function onPaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items;
  if (!items) return;
  const files: File[] = [];
  for (const item of Array.from(items)) {
    const f = item.getAsFile();
    if (f) files.push(f);
  }
  if (files.length) {
    e.preventDefault();
    addFiles(files as unknown as FileList);
  }
}

function onDrop(e: DragEvent) {
  e.preventDefault();
  addFiles(e.dataTransfer?.files ?? null);
}

function removeImage(idx: number) {
  images.value.splice(idx, 1);
}

function removeAttachment(idx: number) {
  attachments.value.splice(idx, 1);
}
</script>

<template>
  <div class="input-area">
    <!-- 图片预览 -->
    <div v-if="images.length" class="preview-row">
      <div v-for="(img, i) in images" :key="i" class="preview-item">
        <img :src="img" alt="待发送图片" class="preview-img" />
        <button class="preview-del" @click="removeImage(i)">✕</button>
      </div>
    </div>

    <!-- 附件标签（任意文件） -->
    <div v-if="attachments.length" class="attach-row">
      <div v-for="(a, i) in attachments" :key="i" class="attach-item" :title="a.path">
        <span class="attach-ico">📎</span>
        <span class="attach-name">{{ a.name }}</span>
        <span class="attach-size">{{ fmtSize(a.size) }}</span>
        <button class="attach-del" @click="removeAttachment(i)">✕</button>
      </div>
    </div>

    <div class="input-wrap">
      <!-- 附件（图案按钮）· 最左侧：点击弹出上传图片/附加文件菜单 -->
      <button class="attach" title="添加附件" @click.stop="attachMenuOpen = !attachMenuOpen">
        <svg width="21" height="21" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="4" stroke-linecap="round"><line x1="12" y1="4.5" x2="12" y2="19.5"/><line x1="4.5" y1="12" x2="19.5" y2="12"/></svg>
      </button>
      <input ref="imageInput" type="file" accept="image/*" multiple hidden @change="(e) => { addFiles((e.target as HTMLInputElement).files); attachMenuOpen = false; }" />
      <input ref="fileInput" type="file" multiple hidden @change="(e) => { addFiles((e.target as HTMLInputElement).files); attachMenuOpen = false; }" />
      <div class="attach-menu" :class="{ open: attachMenuOpen }" @click.stop>
        <button class="am-item" @click="imageInput?.click(); attachMenuOpen = false">
          <span class="ico">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>
          </span><span>上传图片<small>PNG / JPG / WebP</small></span>
        </button>
        <button class="am-item" @click="fileInput?.click(); attachMenuOpen = false">
          <span class="ico">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/></svg>
          </span><span>附加文件<small>任意文件，随消息发送</small></span>
        </button>
        <button class="am-item" @click="captureScreen">
          <span class="ico">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z"/><circle cx="12" cy="13" r="4"/></svg>
          </span><span>截屏提问<small>截取当前屏幕发给 AI</small></span>
        </button>
      </div>

      <!-- 模型选择（智能路由） + 思考模式开关 -->
      <div class="model-wrap">
        <button class="model-tag" @click="modelMenuOpen = !modelMenuOpen">
          <span class="l"></span><span>{{ modelLabel }}</span>
        </button>
        <button
          class="think-tag"
          :class="{ on: thinking }"
          :title="thinking ? '思考模式已开启：deepseek-reasoner 真实思考链' : '思考模式：用 deepseek-reasoner 展示真实思考链'"
          @click="emit('toggle-thinking')"
        >💭</button>
        <div class="model-menu" :class="{ open: modelMenuOpen }">
          <button
            v-for="md in models"
            :key="md.id"
            class="mm-item"
            :class="{ active: md.id === selectedModel }"
            @click="modelMenuOpen = false; emit('select-model', md.id)"
          >
            <span class="mm-ico">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/></svg>
            </span>
            <span class="mm-body">
              <span class="mm-name">{{ md.label }}</span>
              <span class="mm-desc">{{ md.desc }}</span>
            </span>
            <span class="mm-check">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
          </button>
        </div>
      </div>

      <!-- Agent 控制中心 -->
      <div class="agent-inline">
        <label class="agent-toggle" @click="emit('toggle-agent')">
          <span class="at-label">Agent</span>
          <span class="at-switch" :class="{ on: agentOn }"></span>
        </label>
        <div class="mode-wrap">
          <button class="mode-select" @click="modeMenuOpen = !modeMenuOpen">
            <span>{{ modeLabel }}</span>
            <svg class="caret" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
          </button>
          <div class="mode-menu" :class="{ open: modeMenuOpen }">
            <button
              v-for="mo in MODE_OPTIONS"
              :key="mo.id"
              class="mm-item2"
              :class="{ active: mo.id === mode }"
              @click="modeMenuOpen = false; emit('set-mode', mo.id)"
            >
              <span class="dot"></span>{{ mo.label }}
            </button>
          </div>
        </div>
        <span v-if="agentOn && running" class="round-status run">
          <span class="spinner"></span>思考中 · 第 {{ roundNum }} 轮 / 最大 15 轮
        </span>
      </div>

      <!-- 快捷指令（提示词模板） -->
      <div class="prompt-wrap">
        <button class="prompt-tag" title="快捷指令" @click="promptMenuOpen = !promptMenuOpen">📚</button>
        <div class="prompt-menu" :class="{ open: promptMenuOpen }" @click.stop>
          <button v-for="t in PROMPT_TEMPLATES" :key="t.label" class="pm-item" @click="insertTemplate(t)">
            {{ t.label }}
          </button>
        </div>
      </div>

      <textarea
        v-model="prompt"
        :disabled="running || disabled"
        rows="1"
        placeholder="输入问题…（Enter 发送 · Shift+Enter 换行 · 可粘贴/拖拽图片）"
        @keydown="onKeydown"
        @paste="onPaste"
        @drop="onDrop"
        @dragover.prevent
      />

      <!-- 上下文进度环（悬停面板 + 压缩对话） -->
      <div class="progress-wrap">
        <span class="pct">{{ ctxPct ?? 0 }}%</span>
        <div class="ctx-panel">
          <div class="cp-title">会话信息</div>
          <div class="cp-sub">上下文窗口</div>
          <div class="cp-token">
            <span class="cp-bar"><span class="cp-fill" :style="{ width: ctxPct + '%' }"></span></span>
            <span class="cp-val">{{ ctxPct }}%</span>
          </div>
          <div class="cp-line">{{ ctxTokens }}</div>
          <div class="cp-sec">系统</div>
          <div class="cp-item"><span>系统指令</span><span>{{ ctxItems?.sys[0] }}%</span></div>
          <div class="cp-item"><span>工具定义</span><span>{{ ctxItems?.sys[1] }}%</span></div>
          <div class="cp-sec">用户上下文</div>
          <div class="cp-item"><span>消息</span><span>{{ ctxItems?.usr[0] }}%</span></div>
          <div class="cp-item"><span>工具操控</span><span>{{ ctxItems?.usr[1] }}%</span></div>
          <div class="cp-item"><span>文件</span><span>{{ ctxItems?.usr[2] }}%</span></div>
        </div>
        <div class="ring">
          <svg width="26" height="26" viewBox="0 0 24 24">
            <circle class="ring-track" cx="12" cy="12" r="9"/>
            <circle class="ring-fill" cx="12" cy="12" r="9"/>
          </svg>
        </div>
      </div>

      <!-- 发送 / 停止 -->
      <button
        v-if="!running"
        class="send"
        :disabled="(!prompt.trim() && !images.length && !attachments.length) || disabled"
        @click="onSend"
      >发送</button>
      <button v-else class="send" style="background:linear-gradient(180deg,#ff6b5e,#ff453a); animation:none" @click="emit('cancel')">停止</button>
    </div>
  </div>
</template>

<style scoped>
.input-area { position: relative; }
/* 快捷指令（提示词模板） */
.prompt-wrap { position: relative; display: flex; align-items: center; }
.prompt-tag {
  width: 30px; height: 30px; border-radius: 50%;
  border: 1px solid rgba(255,255,255,.35); background: var(--bar);
  color: #5a4a76; font-size: 14px; cursor: pointer; transition: .15s;
  display: flex; align-items: center; justify-content: center;
}
.prompt-tag:hover { border-color: rgba(232,122,92,.5); transform: translateY(-1px); }
.prompt-menu {
  position: absolute; bottom: 36px; left: 0; z-index: 60;
  min-width: 180px; background: rgba(28, 32, 48, 0.97);
  border: 1px solid rgba(255,255,255,.12); border-radius: 12px;
  padding: 6px; display: none; box-shadow: 0 10px 34px rgba(0,0,0,.4);
  backdrop-filter: blur(12px);
}
.prompt-menu.open { display: block; }
.pm-item {
  display: block; width: 100%; text-align: left;
  background: none; border: none; color: #dbe4f0;
  padding: 7px 10px; border-radius: 8px; font-size: 13px; cursor: pointer;
}
.pm-item:hover { background: rgba(232,122,92,.15); color: #fff; }
.preview-row { display: flex; gap: 8px; padding: 0 20px 8px; flex-wrap: wrap; }
.preview-item { position: relative; width: 60px; height: 60px; }
.preview-img { width: 100%; height: 100%; object-fit: cover; border-radius: 12px; border: 1px solid rgba(255,255,255,.5); }
.preview-del { position: absolute; top: -6px; right: -6px; width: 18px; height: 18px; border-radius: 50%; background: #ff453a; color: #fff; font-size: 10px; line-height: 1; display: flex; align-items: center; justify-content: center; cursor: pointer; border: none; }
.attach-row { display: flex; gap: 8px; padding: 0 20px 8px; flex-wrap: wrap; }
.attach-item { display: flex; align-items: center; gap: 6px; max-width: 280px; padding: 4px 8px 4px 6px; border-radius: 10px; border: 1px solid rgba(255,255,255,.5); background: rgba(255,255,255,.4); font-size: 11px; color: var(--txt); }
.attach-ico { font-size: 12px; }
.attach-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.attach-size { color: var(--txt-3); font-size: 10px; flex-shrink: 0; }
.attach-del { width: 16px; height: 16px; border-radius: 50%; background: #ff453a; color: #fff; font-size: 9px; line-height: 1; display: flex; align-items: center; justify-content: center; cursor: pointer; border: none; flex-shrink: 0; }
</style>
