<script setup lang="ts">
/**
 * 消息卡片 —— 文本 + 图片渲染 + 工具调用（三色状态 + 高危标记）。
 */

interface ToolCallInfo {
  toolId: string;
  arguments: unknown;
  status: "running" | "success" | "error" | "danger";
  output?: unknown;
  error?: string;
}

interface ChatMsg {
  id: string;
  role: "user" | "assistant";
  content: string;
  timestamp: number;
  toolCalls?: ToolCallInfo[];
  images?: string[];
  attachments?: string[]; // 附件文件绝对路径
}

defineProps<{ message: ChatMsg }>();

function formatTime(ts: number) {
  return new Date(ts).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
}

// 工具状态：图标 + 文案 + CSS 类（三色）
const STATUS_META: Record<string, { icon: string; label: string }> = {
  running: { icon: "⏳", label: "运行中" },
  success: { icon: "✅", label: "成功" },
  error: { icon: "❌", label: "失败" },
  danger: { icon: "🚨", label: "高危" },
  interrupted: { icon: "⏹", label: "已中断" },
};

function statusOf(s: string) {
  return STATUS_META[s] ?? { icon: "·", label: s };
}

function argsPreview(args: unknown): string {
  const s = JSON.stringify(args) ?? "";
  return s.length > 200 ? s.slice(0, 200) + "…" : s;
}

function outputText(call: ToolCallInfo): string {
  if (call.error) return call.error;
  if (!call.output) return "";
  return JSON.stringify(call.output, null, 2);
}

function isHighRisk(toolId: string): boolean {
  // 高危工具白名单（与后端 is_high_risk 标记对应）
  return toolId.includes("window_close") || toolId.includes("file_write") || toolId.includes("shell");
}
</script>

<template>
  <div class="msg-row" :class="message.role">
    <div class="msg-avatar">{{ message.role === "user" ? "我" : "AI" }}</div>
    <div class="msg-body">
      <div class="msg-meta">
        <span class="msg-role">{{ message.role === "user" ? "你" : "ClawDesk" }}</span>
        <span class="msg-time">{{ formatTime(message.timestamp) }}</span>
      </div>

      <!-- 文本内容 -->
      <div class="msg-content">{{ message.content }}</div>

      <!-- 图片渲染 -->
      <div v-if="message.images?.length" class="msg-images">
        <img v-for="(img, i) in message.images" :key="i" :src="img" class="msg-img" alt="图片" />
      </div>

      <!-- 附件渲染（任意文件） -->
      <div v-if="message.attachments?.length" class="msg-attachments">
        <div v-for="(p, i) in message.attachments" :key="i" class="msg-attach" :title="p">
          <span class="attach-ico">📎</span>
          <span class="attach-path">{{ p.split(/[\\/]/).pop() }}</span>
        </div>
      </div>

      <!-- 工具调用卡片（三色状态） -->
      <div v-if="message.toolCalls?.length" class="tool-blocks">
        <details v-for="(tc, i) in message.toolCalls" :key="i" class="tool-block" :class="tc.status">
          <summary class="tool-summary">
            <span class="tool-status" :class="tc.status">
              {{ statusOf(tc.status).icon }} {{ statusOf(tc.status).label }}
            </span>
            <span v-if="isHighRisk(tc.toolId)" class="tool-risk">⚠️ 高危</span>
            <code class="tool-id">{{ tc.toolId }}</code>
          </summary>
          <div class="tool-body">
            <div class="tool-section">
              <span class="tool-label">参数</span>
              <pre>{{ argsPreview(tc.arguments) }}</pre>
            </div>
            <div class="tool-section" v-if="outputText(tc)">
              <span class="tool-label">结果</span>
              <pre>{{ outputText(tc) }}</pre>
            </div>
          </div>
        </details>
      </div>
    </div>
  </div>
</template>

<style scoped>
.msg-row { display: flex; gap: 12px; margin-bottom: 20px; max-width: 760px; }
.msg-row.user { margin-left: auto; flex-direction: row-reverse; }
.msg-avatar { width: 32px; height: 32px; border-radius: 50%; background: var(--color-surface); border: 1px solid var(--color-border); display: flex; align-items: center; justify-content: center; font-size: 11px; font-weight: 700; color: var(--color-text-secondary); flex-shrink: 0; }
.msg-body { flex: 1; min-width: 0; }
.msg-meta { display: flex; align-items: center; gap: 8px; margin-bottom: 4px; }
.msg-role { font-size: 12px; font-weight: 600; color: var(--color-text-secondary); }
.msg-time { font-size: 11px; color: var(--color-text-muted); }
.msg-content { background: var(--color-msg-assistant); border: 1px solid var(--color-msg-assistant-border); border-radius: var(--radius-md); padding: 12px 16px; font-size: 13px; line-height: 1.65; white-space: pre-wrap; word-break: break-word; color: var(--color-text); }
.msg-row.user .msg-content { background: var(--color-msg-user); border-color: var(--color-msg-user-border); }

/* 图片 */
.msg-images { display: flex; gap: 8px; margin-top: 8px; flex-wrap: wrap; }
.msg-img { max-width: 180px; max-height: 180px; border-radius: var(--radius-sm); border: 1px solid var(--color-border); object-fit: cover; }

/* 附件（任意文件） */
.msg-attachments { display: flex; gap: 8px; margin-top: 8px; flex-wrap: wrap; }
.msg-attach { display: flex; align-items: center; gap: 6px; max-width: 260px; padding: 4px 10px; border-radius: var(--radius-sm); border: 1px solid var(--color-border); background: var(--color-card); font-size: 11px; color: var(--color-text-secondary); }
.attach-ico { font-size: 12px; }
.attach-path { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

/* 工具调用 */
.tool-blocks { margin-top: 8px; }
.tool-block { border: 1px solid var(--color-border); border-radius: var(--radius-sm); margin-bottom: 4px; background: var(--color-card); overflow: hidden; }
.tool-block.running { border-color: var(--color-accent); }
.tool-block.success { border-color: #1b4d2a; }
.tool-block.error { border-color: var(--color-danger); }
.tool-summary { display: flex; align-items: center; gap: 8px; padding: 7px 12px; cursor: pointer; font-size: 12px; color: var(--color-text-secondary); }
.tool-summary::-webkit-details-marker { display: none; }
.tool-status { font-size: 10px; font-weight: 700; padding: 1px 6px; border-radius: 4px; white-space: nowrap; }
.tool-status.running { background: #1a3042; color: var(--color-accent); }
.tool-status.success { background: #1b3822; color: var(--color-success); }
.tool-status.error { background: #3b1c1c; color: var(--color-danger); }
.tool-status.danger { background: #3b1c1c; color: var(--color-danger); }
.tool-status.interrupted { background: #3b3320; color: var(--color-warning); }
.tool-risk { font-size: 10px; font-weight: 700; color: var(--color-danger); border: 1px solid var(--color-danger); border-radius: 4px; padding: 0 4px; }
.tool-id { font-family: var(--font-mono); font-size: 11px; }
.tool-body { padding: 0 12px 10px; }
.tool-section { margin-top: 8px; }
.tool-label { font-size: 10px; font-weight: 600; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.5px; }
.tool-body pre { margin: 4px 0 0; padding: 8px 10px; background: var(--color-code-bg); border-radius: var(--radius-sm); color: var(--color-text-secondary); font-size: 11px; white-space: pre-wrap; word-break: break-all; max-height: 220px; overflow-y: auto; }
</style>
