import { invoke } from "@tauri-apps/api/core";
import type { ToolCall, ToolResult } from "../types/tool";

/**
 * 工具循环最大轮次 —— 与 Rust 侧 ToolDispatcher 默认熔断阈值一致（DEV_SPEC.md §9）。
 * 熔断的权威判定在 Rust 侧执行，此处常量仅用于前端提前提示。
 */
export const MAX_TOOL_ROUNDS = 5;

/**
 * 前端工具调度器 —— 薄转发层。
 *
 * 契约（DEV_SPEC.md §4.4）：参数校验、安全中间件、熔断判定均在 Rust 侧
 * `ToolDispatcher` 执行，前端不重复实现业务规则，仅负责：
 * 1. 构造 `ToolCall`（round 由调用方传入）；
 * 2. 经 Tauri invoke 转发；
 * 3. 将 `ToolResult` / 异常归一为 `ToolResult` 返回。
 */
export async function invokeTool(call: ToolCall): Promise<ToolResult> {
  if (call.round > MAX_TOOL_ROUNDS) {
    return {
      status: "error",
      message: `工具循环轮次 ${call.round} 超过熔断上限 ${MAX_TOOL_ROUNDS}`,
    };
  }

  try {
    return await invoke<ToolResult>("invoke_tool", { call });
  } catch (err) {
    // invoke 拒绝（ToolError 序列化或后端异常）统一归为 error 态
    return {
      status: "error",
      message: typeof err === "string" ? err : JSON.stringify(err),
    };
  }
}
