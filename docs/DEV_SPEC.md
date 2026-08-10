# ClawDesk 开发规范（DEV_SPEC）

> 本文件是 ClawDesk 项目的唯一规范约束来源。所有阶段开发必须严格遵守本规范。
> 本规范由项目初始化会话固化，任何修改须经用户明确确认。

## 1. 项目概述

- 技术栈：Tauri 2.x + Rust + Vue 3 + TypeScript + Vite
- 项目根目录：`D:\workspace\ClawDesk`
- 目标：带 LLM 工具调用能力的桌面应用，支持 生图 / OCR / 外置 MCP / SkillHub / 窗口控制 等工具能力
- 开发方式：6 个线性阶段依次推进，禁止跨阶段；每阶段交付后须用户确认方可进入下一阶段

## 2. 六阶段路线图

| 阶段 | 名称 | 交付内容 |
|------|------|----------|
| 0 | 骨架与核心工具基础设施 | 项目脚手架、目录骨架、Rust/TS 镜像 UnifiedToolDef、ToolRegistry、ToolDispatcher、IPC 命令层、编译校验 |
| 1 | 内置工具与前端工具面板 | `builtin` 源执行器、前端 ToolPanel 组件、uiPayload 渲染机制 |
| 2 | 安全与调度 | 安全中间件链（高危确认拦截）、5 轮工具循环熔断、模型自动调度 |
| 3 | OCR 与生图 | `builtin` 源 OCR / 生图执行器及对应前端组件 |
| 4 | 外置 MCP 与 SkillHub | `mcp`、`skillhub` 源适配器、MCP 客户端接入 |
| 5 | 窗口控制与集成收尾 | `builtin` 窗口控制执行器、全链路联调、最终编译验证 |

## 3. 目录结构契约

```
ClawDesk/
├── docs/DEV_SPEC.md          # 本规范
├── package.json              # 前端 + Tauri CLI
├── vite.config.ts / tsconfig.json / index.html
├── src/                      # Vue3 + TS 前端
│   ├── main.ts / App.vue
│   ├── types/tool.ts         # UnifiedToolDef TS 镜像
│   ├── core/                 # 前端镜像层（渲染/调用用）
│   └── components/           # 阶段 1 起填充
└── src-tauri/
    ├── Cargo.toml / build.rs / tauri.conf.json
    ├── capabilities/default.json
    └── src/
        ├── main.rs / lib.rs  # 应用装配（仅挂载，不含业务）
        ├── core/             # ⚠️ 核心层：一次成型，永久冻结，禁止任何修改
        │   └── tool/         # def / registry / dispatcher / context / result / error
        ├── adapters/         # 适配器层：后续阶段只增不改
        ├── executors/        # 执行器层：后续阶段只增不改
        ├── middleware/       # 安全中间件：阶段 2
        └── commands/         # IPC 命令层（薄壳转发）
```

## 4. 分层隔离

### 4.1 核心层（core）

- `src-tauri/src/core` 是**永久冻结层**：一次成型，此后**禁止任何代码修改**
- 核心层只提供：统一数据结构、注册表、调度器、错误/结果类型、中间件 trait
- 核心层**不包含任何具体工具实现**（生图/OCR/MCP 等一律不进 core）

### 4.2 适配器层（adapters）

- 新增外部能力（MCP、SkillHub）时，新增适配器模块，**不得修改 core**
- 适配器负责将外部协议（如 MCP JSON-RPC）转换为统一数据结构

### 4.3 执行器层（executors）

- 每个工具一个执行器模块，实现具体业务逻辑
- 执行器通过 `UnifiedToolDef::new(source, name, ...)` 声明自身，注册进 ToolRegistry

### 4.4 命令层（commands）

- 仅做 IPC 薄壳转发，不包含业务逻辑
- 前端经 `invoke("list_tools")` / `invoke("invoke_tool")` 与后端通信

## 5. 统一数据结构契约（UnifiedToolDef）

### 5.1 Rust 侧（src-tauri/src/core/tool/def.rs）

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `String` | 工具唯一 ID，格式 `source:name` |
| `source` | `String` | 工具来源，**动态字符串，禁止硬编码枚举** |
| `name` | `String` | 工具名，不得包含 `:` |
| `description` | `String` | 工具描述 |
| `params` | `Vec<ToolParamDef>` | 参数定义 |
| `is_high_risk` | `bool` | 高危标记，安全中间件消费 |
| `version` | `String` | 工具版本 |
| `ui_payload` | `Option<serde_json::Value>` | **仅前端渲染载荷，绝不混入 LLM 上下文** |
| `metadata` | `Map<String, Value>` | 扩展元数据 |

### 5.2 TS 侧（src/types/tool.ts）

- 与 Rust 侧**逐字段镜像**，字段名采用 camelCase（经 serde `rename_all = "camelCase"` 对齐）
- 修改任一侧必须同步另一侧，否则视为规范违规

### 5.3 修改规则

- 核心数据结构契约在阶段 0 冻结，后续阶段**禁止修改**（如需演进，必须新增独立结构并映射）

## 6. 命名规范

- 工具 ID：`source:name`，`source` 取值如 `builtin` / `mcp` / `skillhub`，**运行时动态，不硬编码**
- 注册时强制校验 `id == format!("{}:{}", source, name)`，不合法直接拒绝注册
- 工具调用参数：JSON 对象；回执：`ToolResult` 三态（success / error / interrupted）

## 7. 动态工具注册表

- ToolRegistry 使用 `RwLock<HashMap<String, UnifiedToolDef>>`（定义表）+ `RwLock<HashMap<String, ToolHandler>>`（处理器表）
- **无任何硬编码工具规则**：工具、来源全部运行时注册
- `sources()` / `list_by_source()` 动态枚举，供前端分组渲染

## 8. uiPayload 隔离规则

- `uiPayload` 仅存在于 `UnifiedToolDef` 的**渲染通道**（list_tools → 前端组件）
- **绝不**进入 LLM 上下文构建逻辑；执行器返回结果同样不得携带 uiPayload
- `ToolContext` 只承载执行期数据（round / session_id / timeout_secs / data）

## 9. 安全模型

- 工具循环熔断：`ToolCall.round > max_rounds(默认 5)` 直接拒绝
- 高危工具：`is_high_risk = true` 的工具有权被安全中间件拦截并要求用户确认（阶段 2 实现）
- 中间件链：`Middleware::before()` 返回 `Err` 即拦截；链在 dispatcher 中顺序执行
- 所有文件写入、Shell 执行、批量文件操作必须弹出确认窗口（阶段 2 中间件实现）

## 10. 编码规范

- Rust：edition 2021，`cargo fmt` / `cargo clippy` 零告警；serde 序列化统一 `rename_all = "camelCase"`
- TS：strict 模式，`vue-tsc --noEmit` 零错误；禁止 `any`（显式 `unknown`）
- 注释：关键设计约束必须写清 WHY（如 uiPayload 隔离原因）

## 11. 阶段交付与验证门禁

每个阶段完成后必须通过以下门禁方可交付，否则不得推进：

1. `cargo check` 零错误
2. `cargo test` 全绿（core 层单元测试）
3. `vue-tsc --noEmit` 零错误
4. `cargo build` 与 `vite build` 编译零错误

## 12. 禁止事项

- ❌ 禁止修改 `src-tauri/src/core` 下任何文件（阶段 0 之后）
- ❌ 禁止跨阶段开发（阶段 N 未确认不得开始阶段 N+1）
- ❌ 禁止一次性生成全项目代码（必须分模块分段输出）
- ❌ 禁止在 core 层硬编码任何工具来源/工具名
- ❌ 禁止将 uiPayload 传入任何 LLM 上下文构建逻辑
- ❌ 禁止绕过确认窗口执行高危操作（写文件/Shell/批量操作）
