/** 统一通知工具（封装 sonner toast） */
import { toast } from 'sonner';

export function notifyError(message?: string) {
  if (message) toast.error(message, { duration: 5000 });
}
export function notifySuccess(message?: string) {
  if (message) toast.success(message, { duration: 3000 });
}
export function notifyInfo(message?: string) {
  if (message) toast(message, { duration: 3000 });
}