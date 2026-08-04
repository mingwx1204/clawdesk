import path from "path"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"
import { inspectAttr } from 'kimi-plugin-inspect-react'

export default defineConfig({
  base: './',
  plugins: [inspectAttr(), react()],
  server: {
    port: 3000,
    // 避免监听 Rust target 构建产物导致 EBUSY 崩溃
    watch: {
      ignored: ['**/src-tauri/target/**', '**/node_modules/**'],
    },
  },
  resolve: { alias: { "@": path.resolve(__dirname, "./src") } },
});
