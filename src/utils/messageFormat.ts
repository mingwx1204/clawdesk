// 消息格式与工具卡辅助（从 App.vue 拆出）：纯函数、无副作用
import type { ToolCallInfo } from "../types/message";

/** 时间戳 → HH:mm:ss */
export function fmtTs(ts: number): string {
  const d = new Date(ts);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  return hh + ":" + mm + ":" + ss;
}

/** 工具参数 → 短字符串（超长截断防刷屏） */
export function fmtArgs(a: unknown): string {
  let s: string;
  try {
    s = typeof a === "string" ? a : JSON.stringify(a);
  } catch {
    s = String(a);
  }
  if (s.length > 300) {
    return s.slice(0, 300) + " …";
  }
  return s;
}

/** 工具输出 → 短字符串（超长截断防刷屏） */
export function fmtOutput(o: unknown): string {
  let s: string;
  try {
    s = typeof o === "string" ? o : JSON.stringify(o);
  } catch {
    s = String(o);
  }
  if (s.length > 1000) {
    return s.slice(0, 1000) + " …（内容过长，已截断显示）";
  }
  return s;
}

export function hasArgs(a: unknown): boolean {
  if (a == null) return false;
  if (typeof a === "string") return a.trim().length > 0;
  if (typeof a === "object") return Object.keys(a as object).length > 0;
  return true;
}

// ── 终端卡片（builtin:terminal 在对话区以真实终端窗口渲染）──

export function isTerminal(tc: ToolCallInfo): boolean {
  return tc.toolId.toLowerCase().includes("terminal");
}

export function termInfo(tc: ToolCallInfo): { exitCode: number | null; stdout: string; stderr: string; cmd: string } {
  let exitCode: number | null = null;
  let stdout = "";
  let stderr = "";
  const o = tc.output;
  if (o && typeof o === "object") {
    const obj = o as Record<string, unknown>;
    if (typeof obj.exitCode === "number") exitCode = obj.exitCode;
    if (typeof obj.stdout === "string") stdout = obj.stdout;
    if (typeof obj.stderr === "string") stderr = obj.stderr;
  } else if (typeof o === "string") {
    stdout = o;
  }
  let cmd = "";
  const a = tc.arguments;
  if (a && typeof a === "object") {
    const c = (a as Record<string, unknown>).command;
    if (typeof c === "string") cmd = c;
  } else if (typeof a === "string") {
    try {
      const p = JSON.parse(a);
      if (p && typeof p.command === "string") cmd = p.command;
    } catch {
      cmd = a;
    }
  }
  return { exitCode, stdout, stderr, cmd };
}

export function hasToolDetail(tc: ToolCallInfo): boolean {
  if (tc.output || tc.error) return true;
  if (isTerminal(tc)) {
    const ti = termInfo(tc);
    return !!(ti.cmd || ti.stdout || ti.stderr || ti.exitCode !== null);
  }
  return hasArgs(tc.arguments);
}

export function toolSummary(tc: ToolCallInfo): string {
  if (isTerminal(tc)) return termInfo(tc).cmd || "(无命令)";
  return fmtArgs(tc.arguments);
}
