# ClawDesk 会话交接文档

> 本文件记录了 2025-07-15 会话与 2026-08-20 续作会话中完成的全部工作，供下一个智能体继续推进。

---

## 本次会话提交总览

| 提交 | 说明 |
|------|------|
| `0a4b333` | feat: 右侧面板新增清空上下文快捷操作 |
| `b37ef0a` | feat: 微信面板支持 3 槽位多账号切换 |
| `8a4c6d2` | feat: 暗色/亮色主题切换 + 外观设置面板即时生效 |
| `3c8fa93` | feat: 窗口缩窄时右侧面板自动折叠（<1020 收起 / >1080 恢复 / 手动展开覆盖） |
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

### 暗色 / 亮色主题切换（`8a4c6d2`）

**文件**：`src/styles/variables.css`（亮色变量）、`src/styles/theme-light.css`（硬编码暗色补偿层）、`src/components/SettingsView.vue`（外观控件）、`src/App.vue`（`applyAppearance`）

- 设置面板新增「外观」区：深色 / 亮色主题、界面不透明度（60%~100%）、字号（12~22px）
- 所有外观项通过 `settingsApi.set` 持久化，保存后 `@appearance` 事件即时同步主界面
- 亮色模式复用同一套 CSS 变量（`--bar`/`--glass-border`/`--txt` 等全部切换）
- `theme-light.css` 用 `html[data-theme="light"]` 前缀覆盖仍硬编码的暗色局部样式（壁纸/气泡/代码块/微信面板等），保证能压过 scoped 样式
- `#app { opacity: var(--ui-op) }` 让不透明度滑块真实生效

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

### 响应式自动折叠（`3c8fa93`）

**文件**：`src/App.vue`

- 窗口宽度 < `1020px`：右侧面板自动折叠（`rightAutoCollapsed = true`）
- 窗口宽度 ≥ `1080px`：若为自动折叠则自动恢复（60px 滞回，防止拖动时抖动）
- 窄窗口下手动展开：设置 `rightManualOverride = true`，本次窄窗口周期内不再自动收起
- 回到宽窗口后清除手动覆盖标记；标题栏按钮 tooltip 会提示「窄窗口已自动收起」
- 监听 `window.resize`，挂载时先应用当前尺寸，卸载时移除监听

### 右侧面板快捷操作（`0a4b333`）

- 面板头部新增「↻ 刷新」与「🧹 清空上下文」两个按钮
- 清空上下文：新增后端命令 `agent_session_clear`（`session_cmd.rs`），调用 `SessionManager::clear_context`
  - 删除全部消息并把 `lastInputTokens` 归零，**保留会话本身与累计 token 统计**
  - 前端二次确认、运行中禁用，完成后自动刷新消息列表和用量面板
- 未做假「手动压缩」按钮：自动压缩由引擎接管，真正的手动压缩需接入 LLM 摘要链，留作后续

---

## 五、微信面板主题统一与多槽位

**文件**：`src/components/WechatPanel.vue`

23 种硬编码色值全部替换为 CSS 变量：
- 卡面 `#1e2739` → `var(--color-card)`
- 边框 `#2a3752`/`#26324a` → `var(--glass-border)`
- 主色 `#3b82f6`/`#2563eb` → `var(--color-accent)`
- 文字 `#e8edf7`/`#94a3b8`/`#64748b` → `var(--color-text)`/`--color-text-secondary`/`--color-text-muted`

剩余的语义色（success/warning/danger/soul-panel 渐变）保持原样。

### 多槽位支持（`b37ef0a`）

- 后端 `src-tauri/src/wechat.rs`：`MAX_BOTS` 由 1 放开到 **3**（微信1/2/3），每个槽位独立登录凭据/人设/聊天记录/AI 会话
- 前端 `WechatPanel.vue`：标题栏下新增槽位标签条（在线状态点 + 人设标记），点击切换；`selectSlot` 原本已具备全部切换逻辑，这次补上模板入口
- `App.vue` 微信在线圆点改为多槽位聚合：任一微信连接即亮灯；启动时主动拉取一次 `wechat_bot_status`
- `.wc-chat-item.active` 微调：accent 边框 + 3px 左侧 accent 指示条

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
│   │   ├── SettingsView.vue # 设置弹窗（模型/外观配置）
│   │   └── WechatPanel.vue # 微信 Bot 面板（多账号/聊天/人设/灵魂面板）
│   ├── styles/
│   │   ├── variables.css   # CSS 变量（主题色/字体/间距）
│   │   ├── base.css        # 基础控件样式
│   │   ├── wallpaper.css   # 壁纸 + 标题栏 + 右侧面板 + 工具卡片
│   │   ├── messages.css    # 消息气泡 + 思考链 + Markdown 渲染
│   │   ├── input.css       # 输入区 + 附件菜单 + 设置卡片
│   │   ├── menu.css        # 侧边栏 + 搜索弹窗 + 会话列表
│   │   └── theme-light.css # 亮色主题精细补偿层
│   ├── utils/
│   │   ├── api.ts          # IPC 调用封装层
│   │   ├── messageFormat.ts # 消息/工具格式化
│   │   └── markdown.ts     # Markdown 渲染 + HTML 转义
│   └── composables/        # useSessions / useWechat / useImageViewer / useClock
├── src-tauri/src/
│   ├── commands/
│   │   ├── llama_server.rs # llama-server 生命周期管理
│   │   └── session_cmd.rs  # 会话管理命令（usage / clear / export / search…）
│   ├── llm/
│   │   ├── mod.rs          # 构建 system prompt + 工具注册
│   │   └── session.rs      # 会话管理器（持久化 + clear_context）
│   └── wechat.rs           # 微信 Bot（MAX_BOTS=3 多槽位）
└── docs/
    ├── inspiration.md      # 本地视觉模型调研笔记
    ├── DEV_SPEC.md         # 开发规范
    └── AstrBot对比研究报告.md
```

---

## 七、构建与校验

- **类型检查**：`npx vue-tsc --noEmit`（零错误）
- **前端构建**：`npx vite build`（~1.4s，约 52kB CSS / 290kB JS）
- **Rust 检查**：`cargo check`（零 warning）+ `cargo test`（新增 clear_context 单测通过）
- **完整运行**：`npx tauri dev`（llama-server 自动加载 ~5s）

---

## 八、待办 / 后续方向

原五项待办已全部完成：

1. **右侧面板增强**：✅ 已完成（`0a4b333`，清空上下文 + 刷新统计）
2. **微信面板 active 微调**：✅ 已完成（`b37ef0a`，accent 边框 + 左指示条）
3. **响应式适配**：✅ 已完成（`3c8fa93`，窗口缩窄时右侧面板自动折叠）
4. **暗色/亮色主题切换**：✅ 已完成（`8a4c6d2`，设置面板即时切换并持久化）
5. **微信面板多槽位**：✅ 已完成（`b37ef0a`，后端 3 槽位 + 前端槽位标签切换）

后续可选方向：

- **亮色主题打磨**：部分语义色/内联色仍为深色优化（如 `WechatPanel` 内联 `#999`），可按需继续替换
- **真正的手动压缩**：`SessionManager::needs_compaction` / `compact_with` 已就绪但未接线，可加「LLM 摘要 → 压缩」命令
- **文件变动预览**：右侧面板可接入快照 diff（后端已有 `snapshot_diff` 命令）
- **多槽位细节**：槽位标签可显示未读消息数；微信面板轮询可按当前槽位拆分，避免三槽位重复拉取全局 soul 状态

---

*最后更新：2026-08-20 · 本次新增 4 次提交（`3c8fa93` / `8a4c6d2` / `b37ef0a` / `0a4b333`） · 工作目录 `D:\workspace\ClawDesk`*
