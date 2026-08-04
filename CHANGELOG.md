# ClawDesk 版本记录

## v0.2.0 (2026-08-03)

### 🔧 Bug 修复 (14 项)
- 修复无对话时发送消息自动创建对话
- 修复 AI 朗读在正常对话中完全不工作
- 修复语音输入"长按空格"无延迟检测 → 添加 400ms 阈值
- 修复语音识别文本疯狂重复追加
- 修复 API Key 输入框 type="password" 阻止粘贴 → 改为 text
- 修复颜色选择器 HSL 字符串始终显示为黑色 → HSL→Hex 转换
- 修复自动保存只触发一次且要求≥3条消息 → 独立 autoSaveAfterTurn
- 修复工具调用出错后错误消息被误解析为工具调用
- 修复 \_\_auto\_\_ 模型路由无兜底模型
- 修复 Mock DB 消息不排序导致历史消息乱序
- 修复 TTS onerror 不调用 onEnd 回调
- 修复图片粘贴竞态条件（blob 为 null 时图片丢失）
- 修复无法删除最后一个分身 → 移除限制 + 添加删除全部按钮

### 🧹 代码清理
- 移除种子数据"完整开发历程"泄漏到生产包
- 删除 4 个未使用文件: seed.ts, search.worker.ts, WechatBotTab.tsx, PermissionBar.tsx
- 移除 7 处死代码: stopSpeaking, notifyWarning, isCustomModel, DEFAULT_IMAGE_GEN_CONFIG 等
- 统一 mediaGen 默认 provider: comfyui → pollinations
- 移除过期 Web Worker 注释

### 🖥️ 部署兼容性
- 添加 .cargo/config.toml 静态链接 VC++ 运行时
- 添加 WebView2 运行时检测 + 自动下载引导
- NSIS 安装包仅简体中文

### 📦 环境
- Rust: 1.97.1 (MSVC, crt-static)
- Tauri: 2.x
- React: 18 + Vite 7.3

---

## v0.1.0 (2026-08-02)

- 初始版本
- Tauri + React 桌面应用
- DeepSeek V4 API 集成
- 对话管理、分身系统
- 文件操作工具
- Bot 平台（6 平台）
- 微信扫码连接
- 语音输入 + TTS 朗读
- 媒体生成集成
- Win11 风格 UI + 动画系统
