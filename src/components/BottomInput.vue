<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
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

defineProps<{
  running?: boolean;
  disabled?: boolean;
  models?: ModelOption[];
  selectedModel?: string;
}>();
const emit = defineEmits<{
  (e: "send", content: string, images?: string[], attachments?: string[]): void;
  (e: "cancel"): void;
  (e: "select-model", id: string): void;
}>();

const prompt = ref("");
const images = ref<string[]>([]);
const attachments = ref<AttachItem[]>([]);
const fileInput = ref<HTMLInputElement | null>(null);
const imageInput = ref<HTMLInputElement | null>(null);
const attachMenuOpen = ref(false);
const moreOpen = ref(false);

/** 供父组件（编辑重发）回填输入框。 */
function setPrompt(text: string) {
  prompt.value = text;
}
defineExpose({ setPrompt });

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

      <!-- ⌘ 更多菜单（收编模型/思考/Agent/模式/上下文/快捷指令） -->
      <div class="more-wrap">
        <button class="more-btn" :class="{ on: moreOpen }" title="更多选项" @click="moreOpen = !moreOpen">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="5" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="12" cy="19" r="1"/></svg>
        </button>
        <div class="more-menu" :class="{ open: moreOpen }" @click.stop>
          <!-- 模型选择 -->
          <div class="more-sec">模型</div>
          <button
            v-for="md in models"
            :key="md.id"
            class="more-item"
            :class="{ active: md.id === selectedModel }"
            @click="moreOpen = false; emit('select-model', md.id)"
          >
            <span>{{ md.label }}</span>
            <span v-if="md.id === selectedModel" class="more-check">✓</span>
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
/* 快捷指令（提示词模板）已移除 */
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
