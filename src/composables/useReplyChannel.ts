import { ref } from "vue";

/**
 * 回复通道 —— 二选一：AI 用哪条微信路线回消息。
 *   - "bot"：iLink Bot（wechat.rs，扫码登录的长轮询自动回复）
 *   - "vm" ：虚拟机里的独立微信（vm_vnc.rs，AI 托管模式）
 *
 * 只有这两种，没有关闭状态（关闭 = 两条都关才叫关，用户用面板开关控制）。
 * 与旧开关双向同步（向后兼容）：
 *   - 旧键 `clawdesk_wechat_autoreply`（Bot 开关，useWechat.ts 检查）
 *   - 旧键 `clawdesk_vm_guard`（VM 托管开关，App.vue vm://activity 监听检查）
 * 监听条件仍读 localStorage 键；本 composable 只负责「快捷切换 + 状态指示」。
 */

const LS_BOT = "clawdesk_wechat_autoreply";
const LS_VM = "clawdesk_vm_guard";

export type ReplyChannel = "bot" | "vm";

function derive(): ReplyChannel {
  const bot = localStorage.getItem(LS_BOT) !== "off";
  const vm = localStorage.getItem(LS_VM) !== "off";
  if (vm && !bot) return "vm";
  return "bot"; // 含 both / bot-only：bot 优先
}

/** 全局唯一的通道状态（module 级单例，面板与工具栏共享）。 */
const channel = ref<ReplyChannel>(derive());

/** 当前通道（响应式）。 */
export function useReplyChannel() {
  /** 切换通道：严格二选一。选 Bot → 走 Bot（VM 托管关）；选虚拟机 → 走虚拟机（Bot 关）。 */
  function setChannel(c: ReplyChannel): void {
    if (c === "bot") {
      localStorage.setItem(LS_BOT, "on");
      localStorage.setItem(LS_VM, "off");
    } else {
      localStorage.setItem(LS_BOT, "off");
      localStorage.setItem(LS_VM, "on");
    }
    channel.value = c;
  }

  /** 面板内旧开关被手动改过 → 重新派生（保持指示器与真实状态一致）。 */
  function resync(): void {
    channel.value = derive();
  }

  return { channel, setChannel, resync };
}
