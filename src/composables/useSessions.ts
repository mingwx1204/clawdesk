// 会话管理 composable：集中管理会话列表、自定义名、分支从属、断点状态。
// 数据操作与 UI 副作用解耦：本模块只负责会话元数据与增删改查，
// 消息渲染 / 滚动 / 上下文占用等 UI 副作用由调用方（App.vue）通过回调注入。
import { ref } from "vue";
import { sessionsApi } from "../utils/api";

export function useSessions() {
  /** 当前激活会话 id（default = 默认会话）。 */
  const sessionId = ref("default");
  /** 全部会话 id 列表。 */
  const sessions = ref<string[]>([]);
  /** 会话自定义名（id -> name，缺省显示 id）。 */
  const sessionNames = ref<Record<string, string>>({});
  /** 分支从属关系（父会话 id -> 分支会话 id 列表，§十二.2）。 */
  const branches = ref<Record<string, string[]>>({});
  /** 断点状态（会话 id -> 是否有可续跑断点，§十二.1）。 */
  const checkpoints = ref<Record<string, boolean>>({});

  /** 刷新会话列表 + 自定义名 + 分支从属 + 断点状态。 */
  async function refresh() {
    sessions.value = await sessionsApi.list();
    // 会话自定义名（id -> name），用于显示友好名
    try {
      const metas = await sessionsApi.metas();
      const m: Record<string, string> = {};
      for (const x of metas ?? []) if (x?.name) m[x.id] = x.name;
      sessionNames.value = m;
    } catch { /* ignore */ }
    // 分支从属关系（§十二.2）与断点状态（§十二.1）
    const b: Record<string, string[]> = {};
    const cp: Record<string, boolean> = {};
    for (const s of sessions.value) {
      try {
        const br = await sessionsApi.branches(s);
        if (br.length) b[s] = br;
      } catch { /* ignore */ }
      try {
        const ck = await sessionsApi.checkpoint(s);
        cp[s] = ck != null;
      } catch { /* ignore */ }
    }
    branches.value = b;
    checkpoints.value = cp;
  }

  /** 重命名会话（留空恢复默认 id）。返回是否成功。 */
  async function rename(id: string, newName: string): Promise<boolean> {
    const trimmed = newName.trim();
    try {
      await sessionsApi.rename(id, trimmed);
      if (trimmed) sessionNames.value[id] = trimmed;
      else delete sessionNames.value[id];
      return true;
    } catch (e) {
      console.error("重命名失败", e);
      return false;
    }
  }

  /** Fork 分支会话（§十二.2）：完整拷贝记忆，返回新会话 id。 */
  async function fork(id: string): Promise<string | null> {
    const newId = `branch-${Date.now()}`;
    try {
      await sessionsApi.fork(id, newId);
      sessionId.value = newId;
      await refresh();
      return newId;
    } catch (e) {
      console.error("Fork 失败", e);
      return null;
    }
  }

  /** 删除会话；若删除的是当前会话则切回 default。返回是否成功。 */
  async function remove(id: string): Promise<boolean> {
    try {
      await sessionsApi.delete(id);
    } catch (e) {
      console.error("删除会话失败", e);
      return false;
    }
    if (sessionId.value === id) sessionId.value = "default";
    await refresh();
    return true;
  }

  return {
    sessionId,
    sessions,
    sessionNames,
    branches,
    checkpoints,
    refresh,
    rename,
    fork,
    remove,
  };
}
