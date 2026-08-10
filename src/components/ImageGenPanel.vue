<script setup lang="ts">
import { ref } from "vue";
import { invokeTool } from "../core/ToolDispatcher";
import type { ToolResult } from "../types/tool";

/**
 * 图像生成面板 —— 阶段 3。
 *
 * 流程：prompt + 尺寸 → 调 `builtin:generate_image`（统一工具通道）→ 预览返回的 dataUrl。
 * 说明：阶段 3 为程序化占位图；真实生成引擎后续阶段接入。
 */

const prompt = ref("");
const width = ref(512);
const height = ref(512);
const calling = ref(false);
const result = ref<ToolResult | null>(null);
const previewUrl = ref<string>("");

async function generate(): Promise<void> {
  if (!prompt.value.trim() || calling.value) return;

  calling.value = true;
  result.value = null;
  previewUrl.value = "";

  const call = {
    id: `gen-${Date.now()}`,
    toolId: "builtin:generate_image",
    arguments: {
      prompt: prompt.value.trim(),
      width: Number(width.value) || 512,
      height: Number(height.value) || 512,
    },
    round: 1,
  };

  try {
    result.value = await invokeTool(call);
    if (result.value.status === "success") {
      const output = result.value.output as { dataUrl?: string; path?: string };
      previewUrl.value = output.dataUrl ?? "";
    }
  } finally {
    calling.value = false;
  }
}

function resultText(): string {
  if (!result.value) return "";
  if (result.value.status === "success") {
    const output = result.value.output as {
      width?: number;
      height?: number;
      path?: string;
      note?: string;
    };
    return `已生成 ${output.width}×${output.height} 图像\n路径: ${output.path ?? ""}\n${output.note ?? ""}`;
  }
  if (result.value.status === "error") return `❌ ${result.value.message}`;
  return `⏸ ${result.value.reason}`;
}
</script>

<template>
  <section class="panel">
    <h3>🎨 图像生成</h3>

    <div class="form-row">
      <textarea
        v-model="prompt"
        class="prompt-input"
        placeholder="描述你想生成的图像，如：一只坐在草地上的蓝色机器猫"
        rows="2"
      />
    </div>

    <div class="size-row">
      <label>
        宽
        <input v-model.number="width" type="number" min="64" max="2048" step="64" class="size-input" />
      </label>
      <label>
        高
        <input v-model.number="height" type="number" min="64" max="2048" step="64" class="size-input" />
      </label>
      <button class="btn-primary" :disabled="calling || !prompt.trim()" @click="generate">
        {{ calling ? "生成中…" : "生成图像" }}
      </button>
    </div>

    <div v-if="previewUrl" class="preview-box">
      <img :src="previewUrl" alt="生成的图像" class="gen-img" />
    </div>

    <pre v-if="result" class="result" :class="{ error: result.status === 'error' }">
      {{ resultText() }}
    </pre>
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

.form-row {
  margin-bottom: 0.5rem;
}

.prompt-input {
  width: 100%;
  padding: 0.5rem 0.6rem;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  font-size: 0.9rem;
  font-family: inherit;
  resize: vertical;
  box-sizing: border-box;
}

.size-row {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  flex-wrap: wrap;
}

.size-row label {
  font-size: 0.85rem;
  color: #555;
  display: flex;
  align-items: center;
  gap: 0.3rem;
}

.size-input {
  width: 80px;
  padding: 0.4rem 0.5rem;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  font-size: 0.85rem;
}

.btn-primary {
  margin-left: auto;
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

.preview-box {
  margin-top: 0.75rem;
  text-align: center;
}

.gen-img {
  max-width: 100%;
  max-height: 400px;
  border: 1px solid #e2e8f0;
  border-radius: 8px;
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
</style>
