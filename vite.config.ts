import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// Tauri 2 开发约定：固定端口 1420，与 src-tauri/tauri.conf.json 的 devUrl 对应
export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // 避免监听 Rust 编译产物目录（target/*.pdb 被锁定会导致 EBUSY 崩溃）
      ignored: ["**/src-tauri/target/**"],
    },
  },
  build: {
    target: "es2021",
    outDir: "dist",
  },
});
