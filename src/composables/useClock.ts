// 壁纸时钟 composable（时区感知的桌面时钟 + 日期，localStorage 持久化）。
import { ref } from "vue";

export function useClock() {
  const clockTime = ref("");
  const clockDate = ref("");
  // 时区自动保存（localStorage）：设置面板外观页切换后重启不丢失
  const tz = ref(localStorage.getItem("clawdesk_tz") || "Asia/Shanghai");

  function fmtTime(d: Date, tzs: string): string {
    return d.toLocaleTimeString("zh-CN", { timeZone: tzs, hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit" });
  }
  function fmtDate(d: Date, tzs: string): string {
    return d.toLocaleDateString("en-US", { timeZone: tzs, year: "numeric", month: "short", day: "numeric", weekday: "long" }).toUpperCase();
  }
  function updateClock() {
    const now = new Date();
    clockTime.value = fmtTime(now, tz.value);
    clockDate.value = fmtDate(now, tz.value);
  }
  /** 时区变更回调（设置面板触发，落盘 localStorage + 刷新时钟）。 */
  function onTzChange(v: string) {
    tz.value = v;
    localStorage.setItem("clawdesk_tz", v);
    updateClock();
  }

  return { clockTime, clockDate, tz, updateClock, onTzChange };
}
