import { create } from 'zustand';
import { useSettingsStore } from './useSettingsStore';

/**
 * 工具/插件权限闸门。
 * permissionMode = allow_all：直接放行；
 * permissionMode = confirm_each：弹出确认框，等待用户选择。
 * 支持「本次会话内记住同类操作」以减少重复打扰。
 */

export interface PermissionRequest {
  /** 操作类别，如 terminal / fs-delete / screenshot / opener */
  action: string;
  /** 展示给用户的操作名称 */
  title: string;
  /** 详细信息（命令、路径等） */
  detail: string;
}

interface PendingItem extends PermissionRequest {
  resolve: (allow: boolean) => void;
}

interface PermissionState {
  pending: PendingItem | null;
  /** 本次会话已记住放行的操作类别（Set 类型） */
  sessionAllowed: Set<string>;
  request: (req: PermissionRequest) => Promise<boolean>;
  answer: (allow: boolean, remember: boolean) => void;
  /** 移除某项的会话级信任 */
  removeSessionAllow: (action: string) => void;
}

/** 队列：多个请求排队，一次只弹一个 */
let queue: PendingItem[] = [];
let showing = false;

export const usePermissionStore = create<PermissionState>((set, get) => ({
  pending: null,
  sessionAllowed: new Set<string>(),

  request: (req) => {
    const mode = useSettingsStore.getState().settings.permissionMode;
    if (mode === 'allow_all') return Promise.resolve(true);
    if (get().sessionAllowed.has(req.action)) return Promise.resolve(true);
    return new Promise<boolean>((resolve) => {
      queue.push({ ...req, resolve });
      pump(set, get);
    });
  },

  answer: (allow, remember) => {
    const { pending, sessionAllowed } = get();
    if (!pending) return;
    if (allow && remember) {
      const next = new Set(sessionAllowed);
      next.add(pending.action);
      set({ sessionAllowed: next });
    }
    pending.resolve(allow);
    showing = false;
    set({ pending: null });
    pump(set, get);
  },

  removeSessionAllow: (action) => {
    const next = new Set(get().sessionAllowed);
    next.delete(action);
    set({ sessionAllowed: next });
  },
}));

type SetFn = (partial: Partial<PermissionState>) => void;
type GetFn = () => PermissionState;

function pump(set: SetFn, get: GetFn) {
  if (showing || queue.length === 0) return;
  showing = true;
  const item = queue.shift() as PendingItem;
  // 排队期间可能已被记住放行
  if (get().sessionAllowed.has(item.action)) {
    showing = false;
    item.resolve(true);
    pump(set, get);
    return;
  }
  set({ pending: item });
}
