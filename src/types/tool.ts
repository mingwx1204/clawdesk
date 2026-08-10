/**
 * ClawDesk 统一工具数据结构 —— 与 Rust 侧 `src-tauri/src/core/tool/def.rs` 逐字段镜像。
 *
 * 契约（DEV_SPEC.md §5）：
 * - 字段名使用 camelCase，经 Rust 侧 serde `rename_all = "camelCase"` 对齐；
 * - 修改任一侧必须同步另一侧，否则视为规范违规。
 */

/** 工具参数定义（JSON Schema 风格）。 */
export interface ToolParamDef {
  name: string;
  /** string | number | boolean | object | array */
  type: "string" | "number" | "boolean" | "object" | "array";
  description: string;
  required?: boolean;
  enumValues?: string[];
  default?: unknown;
}

/**
 * 统一工具定义 —— 所有工具（内置 / MCP / SkillHub / 窗口控制等）的唯一数据结构。
 *
 * - `id` 必须等于 `source:name`（后端注册时强制校验）；
 * - `source` 为动态字符串，禁止硬编码枚举；
 * - `uiPayload` 仅用于前端渲染，绝不混入 LLM 上下文。
 */
export interface UnifiedToolDef {
  /** 工具唯一 ID：`source:name` */
  id: string;
  /** 工具来源（builtin / mcp / skillhub / ...，运行时动态） */
  source: string;
  /** 工具名，不得包含 `:` */
  name: string;
  description: string;
  params: ToolParamDef[];
  /** 高危标记：安全中间件（阶段 2）消费 */
  isHighRisk?: boolean;
  version: string;
  /**
   * 前端渲染载荷 —— 仅渲染通道使用，绝不混入 LLM 上下文。
   * 任何 LLM 提示词构建逻辑都不得读取此字段。
   */
  uiPayload?: unknown;
  metadata: Record<string, unknown>;
}

/** 工具调用请求 —— 与 Rust 侧 `ToolCall` 镜像。 */
export interface ToolCall {
  /** 调用唯一 ID（前端生成，用于回执） */
  id: string;
  /** 工具 ID：`source:name` */
  toolId: string;
  /** 调用参数（JSON 对象） */
  arguments: Record<string, unknown>;
  /** 当前工具循环轮次（从 1 开始），超过 5 轮熔断 */
  round: number;
}

/** 工具执行结果 —— 与 Rust 侧 `ToolResult` 镜像（三态）。 */
export type ToolResult =
  | { status: "success"; output: unknown }
  | { status: "error"; message: string }
  | { status: "interrupted"; reason: string };

/** 工具错误 —— 与 Rust 侧 `ToolError` 镜像。 */
export interface ToolErrorPayload {
  kind:
    | "invalid_def"
    | "already_registered"
    | "not_found"
    | "max_rounds_exceeded"
    | "middleware_rejected"
    | "execution_failed"
    | "internal";
  message: string;
}
