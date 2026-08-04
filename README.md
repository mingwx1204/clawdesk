# ClawDesk · AI 桌面秘书

高性能、离线优先的桌面 AI 助手。永久记忆、智能体工具、微信 Bot、语音朗读、手机桥接，
开机自动恢复，一台真正的"随身 AI 秘书"。

![platform](https://img.shields.io/badge/platform-Windows%20x64-blue)
![tauri](https://img.shields.io/badge/Tauri-2.0-orange)
![license](https://img.shields.io/badge/license-MIT-green)

## 核心功能

| 模块 | 说明 |
| --- | --- |
| 💬 微信 Bot | 腾讯 iLink Bot 官方协议扫码登录个人微信，手机随时布置任务，AI 自动回复；token 持久化、开机自动恢复 |
| 🧠 永久记忆 | 对话自动提取记忆（重要性评分 + 语义检索 RAG），记忆图谱、自动整合、每日摘要，随用随长 |
| 🔄 智能进化 | 经验自动沉淀为"进化经验"注入系统提示，越用越懂你 |
| 🛠️ 智能体 | 文件读写 / 终端命令（真实 PTY）/ 联网搜索 / 截图 / 多工具编排 |
| 🗣️ 语音朗读 | 系统 SAPI 多音色 TTS，跟随默认音频设备 |
| 📱 手机桥接 | 局域网扫码，手机端同步对话（无需外部服务） |
| 💾 本地存储 | 对话 / 记忆 / 设置全量 SQLite 存储，开机自动恢复上次工作状态 |

## 更多能力

- 多轮上下文、SSE 流式输出（打字机 + 光标闪烁）、Markdown/JSON 导出、消息重新生成/复制/删除
- react-markdown + GFM 表格、Shiki 代码高亮、KaTeX 公式、图片缩略图放大
- 内置 DeepSeek 系列模型，支持自定义 OpenAI 兼容模型；快速/标准/深度三档推理
- 工作目录绑定 + 实时文件树（Rust 遍历 + notify 监听）
- 系统托盘、全局快捷键 Ctrl+Shift+O、桌面通知、开机自启、窗口置顶
- 对话列表虚拟滚动、消息分页懒加载、搜索 Web Worker（万级 <500ms）

## 技术栈

Tauri 2.0 · Rust · React 18 · TypeScript · Zustand · Tailwind CSS · SQLite · portable-pty

## 开发与构建

```bash
# 前端依赖
npm install

# 开发模式（vite + tauri dev）
npm run tauri:dev

# 打包 Windows 安装版（NSIS）
npm run tauri:build
```

环境要求：Node.js 20+、Rust stable、Windows 10/11 + WebView2。

## 微信接入说明

微信功能基于腾讯官方 **iLink Bot API**（`@tencent-weixin/openclaw-weixin` 协议，MIT 开源），
纯 Rust 实现，无需任何第三方服务：
- 设置 → Bot → 微信平台 → 「扫码登录」→ 手机微信扫一扫 → 确认登录
- 登录后 `bot_token` 持久化到本地，开机自动恢复连接
- 手机微信发消息 → AI 自动处理 → 回复到微信

## 数据与隐私

- 所有数据保存在本机 `%APPDATA%\com.clawdesk.app\`（对话、记忆、设置、微信凭据）
- API Key 使用 Web Crypto AES-GCM 加密存储
- 更新安装不会清除任何本地数据

## 构建产物

`npm run tauri:build` 生成的产物位于 `src-tauri/target/release/bundle/`：

- `nsis/ClawDesk_<版本>_x64-setup.exe` — NSIS 安装包（简体中文安装向导）
- 便携版：`src-tauri/target/release/clawdesk.exe` 单文件即绿色版，直接压缩为 zip 解压即用

## 使用指南

1. **首次启动**自动创建「开发助手」分身与一个新对话。未配置 API Key 时为离线演示模式（模拟流式回复）。
2. 打开 **设置 → 模型**，填入 DeepSeek 或其他 OpenAI 兼容模型的 API Key，或添加自定义模型。
3. 输入区左下角切换模型与推理模式；Enter 发送、Shift+Enter 换行；生成中点击红色按钮中断。
4. 顶部工具栏：编辑标题 / 打开工作文件夹 / 文件树 / 终端 / 设置。
5. 对话列表右键：重命名、置顶、删除、导出 Markdown/JSON。
6. **Ctrl+Shift+O** 全局唤起/隐藏窗口；关闭按钮默认最小化到托盘（可在设置中更改）。

## 已知边界

- 截屏当前为主屏整屏捕获，区域框选裁剪在后续版本提供。
- 前端实际使用 React 19（shadcn 模板基线），与 React 18 行为兼容。

## 使用说明

完整的使用教程（安装更新、微信 Bot 扫码、记忆进化、智能体工具、快捷键、FAQ 等）见 **[使用说明.md](使用说明.md)**。
