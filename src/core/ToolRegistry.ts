import { invoke } from "@tauri-apps/api/core";
import type { UnifiedToolDef } from "../types/tool";

/**
 * 前端工具注册表镜像 —— 动态拉取后端注册表，仅用于 UI 渲染。
 *
 * 契约（DEV_SPEC.md §7）：前端不做任何硬编码，工具集合完全由后端
 * 运行时状态决定；本模块只提供查询/枚举能力，不含业务逻辑。
 */

/** 拉取全部工具定义（按 id 排序）。 */
export async function listTools(): Promise<UnifiedToolDef[]> {
  return invoke<UnifiedToolDef[]>("list_tools");
}

/** 按来源拉取工具定义（来源动态，不硬编码）。 */
export async function listToolsBySource(source: string): Promise<UnifiedToolDef[]> {
  const all = await listTools();
  return all.filter((t) => t.source === source);
}

/** 按 ID 查询单个工具定义。 */
export async function getTool(id: string): Promise<UnifiedToolDef | undefined> {
  const all = await listTools();
  return all.find((t) => t.id === id);
}

/** 动态枚举全部已注册来源。 */
export async function listSources(): Promise<string[]> {
  const all = await listTools();
  const sources = new Set(all.map((t) => t.source));
  return [...sources].sort();
}
