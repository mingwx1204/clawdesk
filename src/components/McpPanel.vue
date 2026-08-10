<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/**
 * MCP 面板 —— 阶段 4。
 *
 * 功能：
 * - 登记外部 MCP server（stdio transport：command + args）；
 * - 连接并注册其工具（source: mcp），注册后可刷新工具列表查看；
 * - 显示已登记 server 列表。
 *
 * 契约：本面板只做配置与调用触发，工具执行仍走统一注册表/调度器。
 */

interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
}

const name = ref("");
const command = ref("");
const argsText = ref("");
const servers = ref<McpServerConfig[]>([]);
const adding = ref(false);
const message = ref<string | null>(null);
const messageIsError = ref(false);

async function refreshServers(): Promise<void> {
  servers.value = await invoke<McpServerConfig[]>("mcp_list_servers");
}

onMounted(async () => {
  try {
    await refreshServers();
  } catch (e) {
    message.value = `加载 server 列表失败: ${String(e)}`;
    messageIsError.value = true;
  }
});

function parseArgs(text: string): string[] {
  return text
    .split(/\s+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

async function addServer(): Promise<void> {
  if (!name.value.trim() || !command.value.trim() || adding.value) return;

  adding.value = true;
  message.value = null;

  const config: McpServerConfig = {
    name: name.value.trim(),
    command: command.value.trim(),
    args: parseArgs(argsText.value),
  };

  try {
    const registered = await invoke<number>("mcp_add_server", { config });
    message.value = `✅ 已连接 ${config.name}，注册 ${registered} 个工具（可在上方工具面板查看）`;
    messageIsError.value = false;
    name.value = "";
    command.value = "";
    argsText.value = "";
    await refreshServers();
  } catch (e) {
    message.value = `添加失败: ${String(e)}`;
    messageIsError.value = true;
  } finally {
    adding.value = false;
  }
}
</script>

<template>
  <section class="panel">
    <h3>🔌 外置 MCP Server</h3>

    <div class="form-grid">
      <input v-model="name" class="field" placeholder="名称（如 fs）" />
      <input v-model="command" class="field" placeholder="命令（如 npx / node / 路径）" />
      <input
        v-model="argsText"
        class="field args"
        placeholder="参数（空格分隔，如 @modelcontextprotocol/server-filesystem ./）"
      />
      <button class="btn-primary" :disabled="adding || !name.trim() || !command.trim()" @click="addServer">
        {{ adding ? "连接中…" : "添加并连接" }}
      </button>
    </div>

    <p v-if="message" class="msg" :class="{ error: messageIsError }">{{ message }}</p>

    <div v-if="servers.length" class="server-list">
      <h4>已登记 Server</h4>
      <ul>
        <li v-for="s in servers" :key="s.name">
          <span class="srv-name">{{ s.name }}</span>
          <code>{{ s.command }} {{ s.args.join(" ") }}</code>
        </li>
      </ul>
    </div>

    <p class="hint">
      连接后工具以 <code>mcp:&lt;server&gt;.&lt;tool&gt;</code> 形式出现在工具面板；调用经
      <code>tools/call</code> 转发。
    </p>
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

.form-grid {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

.field {
  padding: 0.45rem 0.6rem;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  font-size: 0.88rem;
}

.field.args {
  font-family: ui-monospace, monospace;
}

.btn-primary {
  align-self: flex-start;
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

.msg {
  margin-top: 0.6rem;
  font-size: 0.85rem;
  color: #166534;
}

.msg.error {
  color: #b91c1c;
}

.server-list {
  margin-top: 0.75rem;
}

.server-list h4 {
  margin: 0 0 0.35rem;
  font-size: 0.85rem;
  color: #666;
}

.server-list ul {
  margin: 0;
  padding-left: 1.2rem;
}

.server-list li {
  font-size: 0.85rem;
  margin: 0.25rem 0;
}

.srv-name {
  font-weight: 600;
  margin-right: 0.5rem;
}

.hint {
  margin-top: 0.6rem;
  color: #999;
  font-size: 0.78rem;
}

code {
  background: #f3f4f6;
  border-radius: 4px;
  padding: 0 4px;
  font-size: 0.78em;
}
</style>
