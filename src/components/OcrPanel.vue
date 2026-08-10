<script setup lang="ts">
import { ref } from "vue";
import { invokeTool } from "../core/ToolDispatcher";
import type { ToolResult } from "../types/tool";

/**
 * OCR 面板 —— 阶段 3。
 *
 * 流程：文件选择 → FileReader 转 base64 → 调 `builtin:ocr`（统一工具通道）→ 展示识别文本。
 * 依赖：后端 OCR 执行器需要系统安装 Tesseract OCR 引擎。
 */

const selectedName = ref<string>("");
const imagePreview = ref<string>("");
const lang = ref("eng");
const calling = ref(false);
const result = ref<ToolResult | null>(null);

function onFileChange(event: Event): void {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;

  selectedName.value = file.name;

  const reader = new FileReader();
  reader.onload = () => {
    // data:image/png;base64,xxxx
    const dataUrl = String(reader.result ?? "");
    imagePreview.value = dataUrl;
    result.value = null;
  };
  reader.readAsDataURL(file);
}

/** 从 data URL 提取纯 base64（去掉 `data:...;base64,` 前缀）。 */
function stripDataPrefix(dataUrl: string): string {
  const idx = dataUrl.indexOf(",");
  return idx >= 0 ? dataUrl.slice(idx + 1) : dataUrl;
}

async function runOcr(): Promise<void> {
  if (!imagePreview.value || calling.value) return;

  calling.value = true;
  result.value = null;

  const call = {
    id: `ocr-${Date.now()}`,
    toolId: "builtin:ocr",
    arguments: {
      image_base64: stripDataPrefix(imagePreview.value),
      lang: lang.value,
    },
    round: 1,
  };

  try {
    result.value = await invokeTool(call);
  } finally {
    calling.value = false;
  }
}

function resultText(): string {
  if (!result.value) return "";
  if (result.value.status === "success") {
    const output = result.value.output as { text?: string; chars?: number };
    return `识别结果（${output.chars ?? 0} 字符）：\n${output.text ?? ""}`;
  }
  if (result.value.status === "error") return `❌ ${result.value.message}`;
  return `⏸ ${result.value.reason}`;
}
</script>

<template>
  <section class="panel">
    <h3>🔍 OCR 文字识别</h3>

    <div class="upload-row">
      <input type="file" accept="image/*" @change="onFileChange" />
      <input v-model="lang" class="lang-input" placeholder="eng" title="识别语言（ISO 639-1）" />
      <button class="btn-primary" :disabled="calling || !imagePreview" @click="runOcr">
        {{ calling ? "识别中…" : "识别文字" }}
      </button>
    </div>

    <div v-if="imagePreview" class="preview-row">
      <img :src="imagePreview" alt="待识别图片" class="preview-img" />
      <p class="file-name">{{ selectedName }}</p>
    </div>

    <pre v-if="result" class="result" :class="{ error: result.status === 'error' }">
      {{ resultText() }}
    </pre>

    <p class="hint">需要系统安装 Tesseract OCR 并确保 tesseract.exe 在 PATH 中。</p>
  </section>
</template>

<style scoped>
.panel {
  font-family: system-ui, sans-serif;
  max-width: 960px;
  margin: 1rem auto 0;
  padding: 1rem 1.5rem;
  border: 1px solid #e5e7eb;
  border-radius: 10px;
  background: #fff;
}

.upload-row {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  flex-wrap: wrap;
}

.lang-input {
  width: 72px;
  padding: 0.4rem 0.6rem;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  font-size: 0.85rem;
}

.btn-primary {
  padding: 0.45rem 1.25rem;
  border: none;
  border-radius: 8px;
  background: #1d4ed8;
  color: #fff;
  font-size: 0.9rem;
  cursor: pointer;
}

.btn-primary:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.preview-row {
  margin-top: 0.75rem;
  display: flex;
  align-items: center;
  gap: 0.75rem;
}

.preview-img {
  max-width: 160px;
  max-height: 160px;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
}

.file-name {
  color: #666;
  font-size: 0.85rem;
}

.result {
  margin-top: 0.75rem;
  padding: 0.75rem;
  border-radius: 8px;
  background: #f8fafc;
  border: 1px solid #e2e8f0;
  font-size: 0.85rem;
  white-space: pre-wrap;
  word-break: break-all;
}

.result.error {
  background: #fef2f2;
  border-color: #fecaca;
  color: #b91c1c;
}

.hint {
  margin-top: 0.5rem;
  color: #999;
  font-size: 0.78rem;
}
</style>
