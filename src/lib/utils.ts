/**
 * 通用工具函数。
 * cn: Tailwind CSS 类名合并（clsx + tailwind-merge），解决样式冲突。
 */

import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

/** 合并 Tailwind CSS 类名，自动去重和优先级处理 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
