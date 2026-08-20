import { createApp } from "vue";

// 启动动画（splash）—— 见 index.html 内联脚本，Vue 挂载前已渲染。
// 由 App.vue 初始化完成后调用 window.__splashHide() 淡出。
declare global {
  interface Window {
    __splashHide?: () => void;
  }
}
import "./styles/variables.css";
import "./styles/base.css";
import "./styles/wallpaper.css";
import "./styles/messages.css";
import "./styles/input.css";
import "./styles/menu.css";
import "./styles/theme-light.css";
import App from "./App.vue";

createApp(App).mount("#app");
