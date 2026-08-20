# ClawDesk 会话交接文档

> 本文件记录了 2025-07-15 会话中完成的全部工作，供下一个智能体继续推进。

---

## 本次会话提交总览

| 提交 | 说明 |
|------|------|
| `16d350b` | refactor: 三栏布局 + 右侧上下文监测面板 + 微信面板主题统一 |
| `889c901` | refactor: UI 全面改版为深空蓝玻璃拟态主题 + 删除废案 game.css + 字体放大到 14px |
| `1991cd8` | fix: 启动动画可拖动窗口 + 四周边缘不漏底色 |
| `7f71682` | feat: 启动动画联动本地视觉模型加载（就绪才淡出） |
| `ac28c69` | feat: 启动动画 splash（终端打字 + 脉冲光晕 + 粒子背景） |
| `538c9b6` | feat: llama-server 模型路径自动发现（环境变量→应用目录→常见目录→盘根浅扫） |

---

## 一、启动动画（splash）

**文件**：`index.html`（inline CSS + JS）

- 深空背景 `#0a0a0f`，终端打字动画 + 脉冲光晕 + 上浮粒子
- 使用 `data-tauri-drag-region` 支持无边框窗口拖动
- 等待 `systemApi.localVisionReady()` 返回 `true` 后才淡出
- 四周边缘不露底色（#splash 显式 `left/top/right/bottom:0`）

**注意**：标题栏（App.vue）使用 JS `startDragging()` 拖动，**不要**加 `data-tauri-drag-region`，否则会拦截 mousedown。

---

## 二、本地视觉模型（llama-server）

**模型**：Qwen2.5-VL-7B Q4_K_M GGUF
- 主模型：`D:\workspace\models\qwen2.5-vl-7b\qwen2.5-vl-7b-Q4_K_M.gguf`（4466MB）
- 投影器：`D:\workspace\models\qwen2.5-vl-7b\mmproj-model-f16.gguf`（1291MB）

**后端**：`src-tauri/src/commands/llama_server.rs`
- 自动发现：环境变量 → 应用数据目录 → 常见目录 → 盘根浅扫
- 启动参数：`-ngl 22`（GPU 加速），端口 8088
- 生命周期：应用启动时自动拉起，退出时自动关闭

**关键发现**（见 `docs/inspiration.md`）：
- 必须用 `-ngl 22` 而非 `--no-mmproj-offload`，否则图片编码从 1.3s 飙升到 29.7s
- RTX 2060 6GB：冷加载 ~11-13s，首次视觉 ~7-9.8s，预热后 ~6.8s

---

## 三、UI 主题改版（深空蓝玻璃拟态）

**设计语言**：呼应启动动画的深空蓝光晕，accent 色 `#6daaff`，卡面半透明玻璃质感。

### 改动的样式文件

| 文件 | 改动 |
|------|------|
| `src/styles/variables.css` | 重写：暗色玻璃变量，`--color-accent: #6daaff`，字体 14px |
| `src/styles/base.css` | 重写：统一暗色控件，14px 字体，box-shadow 聚焦环 |
| `src/styles/wallpaper.css` | 重写：深空蓝渐变壁纸，标题栏 44px 玻璃，新增右侧面板样式 |
| `src/styles/messages.css` | 重写：暗色玻璃气泡，14.5px，工具卡片暗色 |
| `src/styles/input.css` | 重写：输入区，发送按钮蓝色渐变，设置卡片暗色 |
| `src/styles/menu.css` | 重写：侧边栏/菜单/新建会话按钮暗色 |
| `src/styles/game.css` | **已删除**（废案，无引用） |

### 关键 CSS 变量（variables.css）

```css
--color-bg: #0a0a10;
--color-card: #16161f;
--color-surface: #13131c;
--glass: rgba(255,255,255,0.04);
--glass-border: rgba(255,255,255,0.07);
--color-accent: #6daaff;
--color-msg-user: #232a3d;
--bubble-user: rgba(109,170,255,0.18);
```

---

## 四、三栏布局（最新改动）

### 布局结构

```
.root
  .wallpaper（固定背景）
  .app（flex 列）
    .titlebar（44px 玻璃，可拖动）
    .sidebar（position:fixed overlay，左侧会话列表）
    .top-bar（搜索/导出按钮）
    .app-body（flex 行，填满剩余空间）
      .chat-col（flex:1，聊天区 + 输入框）
        .msgs（消息列表）
        BottomInput（底部输入）
      .right-panel（280px，可折叠）
        ├─ 窗口占用进度条（>70% 黄，>90% 红）
        ├─ 系统指令细分（sys prompt / 工具定义）
        ├─ 会话内容细分（消息 / 工具输出 / 文件）
        ├─ 累计统计（输入/输出 token、消息数、压缩次数）
        └─ 工具调用列表（实时状态）
    SettingsView / WechatPanel / 弹窗（overlay）
```

### 右侧面板数据来源

- `sessionsApi.usage(sessionId)` → 后端 `agent_session_usage` 命令
- 返回：`windowTokens`、`windowLimit`、`pct`、`totalInput`、`totalOutput`、`messages`、`compactions`、`sys`、`usr`
- 切换会话时自动刷新（`loadSessionUsage()`）
- 发送消息后也会刷新

### 折叠控制

- 标题栏的第二个切换按钮（三栏图标）控制 `rightCollapsed` ref
- 折叠时宽度变为 0（带 0.22s 过渡动画）

---

## 五、微信面板主题统一

**文件**：`src/components/WechatPanel.vue`

23 种硬编码色值全部替换为 CSS 变量：
- 卡面 `#1e2739` → `var(--color-card)`
- 边框 `#2a3752`/`#26324a` → `var(--glass-border)`
- 主色 `#3b82f6`/`#2563eb` → `var(--color-accent)`
- 文字 `#e8edf7`/`#94a3b8`/`#64748b` → `var(--color-text)`/`--color-text-secondary`/`--color-text-muted`

剩余的语义色（success/warning/danger/soul-panel 渐变）保持原样。

---

## 六、文件结构

```
ClawDesk/
├── index.html              # 启动动画（splash）
├── src/
│   ├── main.ts             # 入口，挂载 Vue，声明 __splashHide
│   ├── App.vue             # 主布局（三栏 + 标题栏 + 所有弹窗）
│   ├── components/
│   │   ├── BottomInput.vue # 底部输入框（附件/模型选择/发送）
│   │   ├── SettingsView.vue # 设置弹窗（模型配置）
│   │   └── WechatPanel.vue # 微信 Bot 面板（多账号/聊天/人设/灵魂面板）
│   ├── styles/
│   │   ├── variables.css   # CSS 变量（主题色/字体/间距）
│   │   ├── base.css        # 基础控件样式
│   │   ├── wallpaper.css   # 壁纸 + 标题栏 + 右侧面板 + 工具卡片
│   │   ├── messages.css    # 消息气泡 + 思考链 + Markdown 渲染
│   │   ├── input.css       # 输入区 + 附件菜单 + 设置卡片
│   │   └── menu.css        # 侧边栏 + 搜索弹窗 + 会话列表
│   ├── utils/
│   │   ├── api.ts          # IPC 调用封装层
│   │   ├── messageFormat.ts # 消息/工具格式化
│   │   └── markdown.ts     # Markdown 渲染 + HTML 转义
│   └── composables/        # useSessions / useWechat / useImageViewer / useClock
├── src-tauri/src/
│   ├── commands/
│   │   ├── llama_server.rs # llama-server 生命周期管理
│   │   └── session_cmd.rs  # 会话管理命令（含 agent_session_usage）
│   └── llm/mod.rs          # 构建 system prompt + 工具注册
└── docs/
    ├── inspiration.md      # 本地视觉模型调研笔记
    ├── DEV_SPEC.md         # 开发规范
    └── AstrBot对比研究报告.md
```

---

## 七、构建与校验

- **类型检查**：`npx vue-tsc --noEmit`（零错误）
- **前端构建**：`npx vite build`（~1.3s，46kB CSS，287kB JS）
- **Rust 检查**：`cargo check`（零 warning）
- **完整运行**：`npx tauri dev`（llama-server 自动加载 ~5s）

---

## 八、待办 / 后续方向

1. **右侧面板增强**：可以加入「快捷操作」按钮（如清空上下文、触发压缩）或「文件变动预览」
2. **微信面板**：配色已统一，但 `.wc-chat-item.active` 的 accent 仍用旧蓝色（`#3b82f6` 已替换为变量），可进一步微调
3. **响应式适配**：窗口缩窄时右侧面板自动折叠或隐藏
4. **暗色/亮色主题切换**：变量体系已就绪，只需加一个切换按钮即可支持亮色模式
5. **微信面板多槽位**：当前仅单个槽位，后端已支持多账号，前端可扩展

---

*最后更新：2025-07-15 · 会话累计 25 次提交 · 工作目录 `D:\workspace\ClawDesk`*
