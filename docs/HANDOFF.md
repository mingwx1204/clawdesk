# ClawDesk 会话交接文档

> 本文件记录了 2025-07-15 会话与 2026-08-20 续作会话中完成的全部工作，供下一个智能体继续推进。

---

## 本次会话提交总览

| 提交 | 说明 |
|------|------|
| `ce32d1d` | feat: 内言（inner monologue）——诗妍回复前写【心语】角色散文，面板💭可展开（参考 inner-voice / Katherine / soul-protocol） |
| `b2ff79c` | feat: 情绪键控记忆——detail_memory 新增 valence/precision，检索与当下 mood 加权匹配 |
| `5755381` | docs: 灵魂笔记 vs 后端现状对照清单（SOUL_GOALS.md） |
| `f740761` | Revert: 恢复被误删的 SOUL 设计草稿笔记 |
| `ad41d86` | feat: 应用图标全部统一为标题栏原子轨道样式 |
| `0aaea42` | fix: 界面不透明度限制在 0.6~1.0，防止旧配置白屏 |
| `7aa79c4` | feat: 微醋意——久未联系时偶尔俏皮嗔怪（>48h 沉默 + 20% 概率） |
| `acc347e` | feat: 特殊日子仪式感——AI 生日 + 百日里程碑自动注入 |
| `bc1b23e` | docs: SOUL_GOALS 标记记忆阈值完成 |
| `eecfc5a` | feat: 记忆检索阈值——弱匹配保持沉默，不污染 prompt（mnemo 原则） |
| `712d486` | feat: 互惠披露——主动聊天时先给出自己，再递话题（先分享、再关心） |
| `b75d3fe` | docs: 记录 GitHub 灵感清单（40+ 仓库中筛选 8 个高价值参考） |
| `ce32d1d` | feat: 内言（inner monologue）——诗妍回复前写【心语】角色散文，面板💭可展开 |
| `b2ff79c` | feat: 情绪键控记忆——detail_memory 新增 valence/precision，检索与当下 mood 加权匹配 |
| `5755381` | docs: 灵魂笔记 vs 后端现状对照清单（SOUL_GOALS.md） |
| `f740761` | Revert: 恢复被误删的 SOUL 设计草稿笔记 |
| `ad41d86` | feat: 应用图标统一为标题栏原子轨道样式 |
| `0aaea42` | fix: 界面不透明度限制在 0.6~1.0，防止旧配置白屏 |
| `585216d` | refactor: 移除微信 slot 兼容层，前后端单账号直连 |
| `bd4d7e9` | chore: 固化 rust check/test/verify npm 脚本并复用 src-tauri/target |
| `7b2b9eb` | test: 线程级数据目录覆盖修复并行测试 env race |
| `3b6684b` | fix: 清理单账号残留误导文案并给清除记忆加进行中状态 |
| `3a047f9` | feat: 会话级互斥锁串行化 agent_chat/压缩/清空/删除与微信记忆写入 |
| `c074faf` | revert: 微信恢复单账号并清理多槽位误导性死代码 |
| `cba1dc2` | build: CSS 类名审计纳入 npm run build 链路 |
| `18ac9a7` | perf: 微信灵魂面板改为展开时按需拉取快照 |
| `51d3cbf` | chore: 新增 CSS 类名回归审计脚本并修复 3 个缺失样式 |
| `1fe30e3` | ~~feat: 微信槽位标签显示最近消息时间与非当前槽位未读计数~~（❌ 已由 `c074faf` 回滚） |
| `f4c8610` | feat: 手动上下文压缩接入 LLM 摘要链 |
| `7d1a19f` | feat: 文件快照差异预览支持一键回滚与删除 |
| `a762167` | refactor: 微信面板剩余内联色值改为语义 CSS 类 |
| `9e72692` | fix: 补齐设置面板按钮/状态提示/页脚样式 |
| `d7589aa` | feat: 右侧面板接入文件快照列表与差异预览 |
| `665bdf7` | fix: 恢复权限/新建会话/搜索弹窗的 overlay 样式（game.css 删除回归） |
| `0a4b333` | feat: 右侧面板新增清空上下文快捷操作 |
| `b37ef0a` | ~~feat: 微信面板支持 3 槽位多账号切换~~（❌ 已由 `c074faf` 回滚） |
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
- `a762167`：`WechatPanel` 剩余内联色值（`#999` / `#f59e0b` / `#c0a060`）改为语义 CSS 类，亮暗主题都使用变量

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
- 手动压缩按钮「🗜 压缩」（需已配置 API Key，运行中禁用）：
  - 新增后端命令 `agent_session_compact`：从最近消息往回收集 ≤24K 字 transcript →
    主模型生成摘要 → `SessionManager::compact_with` 保留最近 10 条并用 system 摘要替换更早历史
  - `SessionManager::can_compact` 守卫：消息数必须 > keep_last+2，避免摘要反而变长
  - 压缩完成后自动刷新消息列表与用量面板，弹窗显示摘要长度 / 当前消息数 / 累计压缩次数

### 会话级互斥（`3a047f9`）

- 新增 `SessionLocks`：`session_id -> Arc<tokio::sync::Mutex<()>>` 常驻锁表
- 以下路径持有同一把会话锁，同一会话的读-改-写不再并发覆盖：
  - `agent_chat`（主界面聊天 + 微信自动回复，覆盖整个 run_agent_loop）
  - `agent_session_compact` / `agent_session_clear` / `agent_session_delete`
  - `harness_start_task`（引擎路径，同样读写会话）
  - 微信主动聊天的记忆读取与写入（等待自动回复完成后再追加）
- `agent_session_delete` / `clear` 改为 async 并返回 `Result<bool, String>`（Tauri 约束）
- 新增单测 `session_locks_reuse_same_mutex_for_same_id`

### 文件变动预览（`d7589aa` / `7d1a19f`）

- 右侧面板新增「文件快照」卡片：展示最近 5 条 `file_write` 自动备份（原文件 + 时间）
- 点击快照 → 弹窗调用 `snapshot_diff` 显示行级 +/- 差异（快照行数 / 当前行数 / 差异数，最多 100 行）
- 弹窗内可一键「回滚到此快照」或「删除快照」（均二次确认；回滚覆盖当前文件属高危操作）
- 前端新增 `snapshotApi`（list / diff / restore / remove）封装四个已有后端命令
- ★ 附带修复：`889c901` 删除 game.css 时误删了仍在使用的 `.perm-overlay` / `.perm-card` 弹窗样式，导致权限确认 / 新建会话 / 历史搜索弹窗无 overlay，已按当前主题重建（`665bdf7`）

---

## 五、微信面板主题统一（单账号）

**文件**：`src/components/WechatPanel.vue`

23 种硬编码色值全部替换为 CSS 变量：
- 卡面 `#1e2739` → `var(--color-card)`
- 边框 `#2a3752`/`#26324a` → `var(--glass-border)`
- 主色 `#3b82f6`/`#2563eb` → `var(--color-accent)`
- 文字 `#e8edf7`/`#94a3b8`/`#64748b` → `var(--color-text)`/`--color-text-secondary`/`--color-text-muted`

剩余的语义色（success/warning/danger/soul-panel 渐变）保持原样。

### ★ 重要澄清：产品定位是单微信（`c074faf`）

- `7ceaea8`（2026-08-19）已明确把微信从 10 槽位**收敛为 1 个**（`MAX_BOTS = 1`），并删除了前端槽位列表
- 但 `slot` 参数、`selectSlot`、`.wc-slots` CSS 等历史兼容残留仍在，HANDOFF 旧条目「后端已支持多账号」是**过期信息**
- 本会话早期误据死代码把 `MAX_BOTS` 扩到 3，并重新渲染了槽位标签；`c074faf` 已全部回滚，并清理了 `selectSlot` / `.wc-slot*` 等误导性残留
- 当前状态：**单微信**，标题为「内置微信（独立于电脑上的微信）」
- `585216d`：正式移除 slot 兼容层——后端 `WechatBotState` 直接持有唯一 `Arc<WechatInner>`；所有 `wechat_*` 命令签名、前端 `wechatApi`、`WechatPanel`、`useWechat` 均不再传 `slot`；`wechat_bot_status` 改为返回单个 `bot` 对象
- 数据兼容保留：账号/人设/聊天记录仍在 `wechat/slot0/` 目录，无需迁移
- `3b6684b`：继续清理 README / 面板说明 / types / useWechat 中的多槽位残留文案；「清除 AI 记忆」增加清空中状态（后端会等待运行中的回复结束）

### 微信面板其他改动

- `.wc-chat-item.active` 微调：accent 边框 + 3px 左侧 accent 指示条
- `a762167`：灵魂面板 / 主动聊天的内联颜色改为 `.wc-soul-sub` / `.wc-ghost-text` / `.wc-tip-warn`，并使用 `--color-warning` 等变量
- `18ac9a7`：灵魂全景快照改为「展开灵魂面板时按需拉取」，不再随 5 秒轮询重复读取八层状态

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
│   │   └── WechatPanel.vue # 微信 Bot 面板（单账号/聊天/人设/灵魂面板）
│   ├── styles/
│   │   ├── variables.css   # CSS 变量（主题色/字体/间距）
│   │   ├── base.css        # 基础控件样式
│   │   ├── wallpaper.css   # 壁纸 + 标题栏 + 右侧面板 + 工具卡片
│   │   ├── messages.css    # 消息气泡 + 思考链 + Markdown 渲染
│   │   ├── input.css       # 输入区 + 附件菜单 + 设置卡片
│   │   ├── menu.css        # 侧边栏 + 搜索弹窗 + 会话列表
│   │   └── theme-light.css # 亮色主题精细补偿层
│   ├── utils/
│   │   ├── api.ts          # IPC 封装（session / chat / settings / snapshot / wechat…）
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
│   └── wechat.rs           # 微信 Bot（单账号，slot 参数仅历史兼容）
└── docs/
    ├── inspiration.md      # 本地视觉模型调研笔记
    ├── DEV_SPEC.md         # 开发规范
    └── AstrBot对比研究报告.md
```

---

## 七、构建与校验

- **类型检查**：`npm run typecheck`（零错误）
- **一键校验**：`npm run verify` = typecheck + build（含 CSS 审计）+ cargo check + cargo test
- **Rust 快捷命令**：`npm run check:rust` / `npm run test:rust`（复用 `src-tauri/target`，不重复编译）
- **CSS 回归审计**：`npm run audit:css`（`scripts/audit-css-classes.mjs`，扫描 Vue 模板 class vs CSS 定义；`cba1dc2` 起已并入 `npm run build` 链路）
- **前端构建**：`npx vite build`（~1.4s，约 57kB CSS / 294kB JS）
- **Rust 检查**：`cargo check`（零 warning）
- **Rust 全量测试**：`cargo test`（378 passed / 0 failed / 1 ignored，含 clear_context / can_compact / session_locks 单测）
- **完整运行（冒烟）**：`timeout 120s npx tauri dev` 实测通过——Vite ready、Rust 编译、窗口创建、SQLite/知识库/沙箱初始化、llama-server 模型加载并监听 8088，无 panic；超时自动退出后端口已释放
- **测试稳定性**：✅ 已修复（`7b2b9eb`）——原全量并行偶发 `CLAWDESK_DATA_DIR` env race，改为 `settings.rs` 的线程级 `DATA_DIR_THREAD_OVERRIDE`；logging / tool_log / self_check 测试不再改全局环境变量，默认并行全量 378 passed / 0 failed / 1 ignored

---

## 八、待办 / 后续方向

原五项待办已全部完成：

1. **右侧面板增强**：✅ 已完成（`0a4b333`，清空上下文 + 刷新统计）
2. **微信面板 active 微调**：✅ 已完成（`b37ef0a`，accent 边框 + 左指示条）
3. **响应式适配**：✅ 已完成（`3c8fa93`，窗口缩窄时右侧面板自动折叠）
4. **暗色/亮色主题切换**：✅ 已完成（`8a4c6d2`，设置面板即时切换并持久化）
5. **微信面板多槽位**：✅ 完成收敛——产品单微信，slot 兼容层已由 `585216d` 移除（仅保留 slot0 数据目录兼容）

后续可选方向（已完成的已并入正文）：

- ✅ **文件变动预览**：已完成（`d7589aa` 列表+差异，`7d1a19f` 回滚/删除）
- ✅ **亮色主题基础打磨**：已完成（`a762167`，微信面板内联色值清理）
- ✅ **真正的手动压缩**：已完成（`f4c8610`，`agent_session_compact` 命令 + 右栏「🗜 压缩」按钮）
- ✅ **多槽位细节**：不适用——已确认单账号定位，槽位未读/最近消息时间已随 `c074faf` 回滚；soul 状态全局唯一是正确的，无需按槽位隔离
- ✅ **弹窗/CSS 回归审计**：已完成（`51d3cbf`，`npm run audit:css` 并修复 `.tc-fold-on` / `.wc-living` / `.wc-msg-time`）
- ✅ **会话操作并发锁**：已完成（`3a047f9`，`SessionLocks` 覆盖 agent_chat / 压缩 / 清空 / 删除 / 微信记忆写入）

---

*最后更新：2026-08-21 · 本次续作最新至 `7aa79c4`（微醋意）· 共实现 5 项活人感增强：情绪键控记忆、内言、互惠披露、记忆阈值、仪式感+微醋意 · GitHub 灵感清单见 `docs/GITHUB_INSPIRATION.md` · 工作目录 `D:\workspace\ClawDesk`*
