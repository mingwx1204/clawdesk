# ClawDesk 灵感清单

> 从 GitHub / 开源社区 / 竞品分析中收集的灵感，按优先级排序。
> 每个灵感标注来源、关联度（⭐高/☆中/·低）和状态。

---

## 性能优化

| # | 灵感 | 来源 | 关联 | 状态 |
|---|------|------|------|------|
| 1 | 移除死依赖 scraper（Cargo.toml 声明但从未 use） | 本地分析 | ⭐ | ✅ 已完成 (bf7ad49) |
| 2 | screenshots crate 拖入 image@0.24.9 + windows@0.51/0.52 旧版本，与主项目冲突 | 本地分析 | ⭐ | 📋 待评估（需重写窗口截图逻辑或换 crate） |
| 3 | headless_chrome 拖入 ureq@3.3 + base64@0.22，与主 ureq@2.12 重复 | 本地分析 | ☆ | 📋 待评估 |
| 4 | dev profile 依赖 opt-level=2（自己代码=0），运行快 + 增量编译快 | 本地分析 | ⭐ | ✅ 已完成（增量 26s→14.8s, -43%） |
| 5 | sccache 编译缓存 | https://github.com/mozilla/sccache | ⭐ | ✅ 已安装（crate-type 多态不命中，保留配置） |
| 6 | release opt-level=3 + panic=abort + lto（速度优先） | 本地分析 | ⭐ | ✅ 已完成 |
| 7 | cargo-nextest 替代 cargo test（并行，404s→~200s） | https://nexte.st/ | ☆ | 📋 未安装 |
| 8 | Tauri 官方 App Size 优化指南 | https://v2.tauri.app/concept/size/ | ⭐ | ✅ 已应用核心项 |
| 9 | min-sized-rust 圣经（opt-level="z"） | https://github.com/johnthagen/min-sized-rust | ☆ | 📋 体积优先 vs 速度优先（当前选速度） |
| 10 | Rust 1.90 rust-lld 稳定（Windows 链接提速 2-5x） | https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable/ | ⭐ | 📋 需 rustup component add llvm-tools |
| 11 | cargo-bloat 分析二进制体积 | https://github.com/RazrFalcon/cargo-bloat | ☆ | 📋 未安装 |
| 12 | Tauri NSIS LZMA 压缩（安装包体积） | https://docs.rs/tauri-utils/ | ☆ | 📋 待评估 |
| 13 | UPX 二次压缩 exe（32MB→8-10MB） | https://blog.gitcode.com/a621ced0a4dd24cf863bda6b554e0e6b.html | ☆ | 📋 发布阶段用 |
---

## 功能特性

| # | 灵感 | 来源 | 关联 | 状态 |
|---|------|------|------|------|
| 1 | iLink Bot 协议参考：长轮询 + markdown 过滤 + ACL 配对 | https://github.com/zongrongjin/weixin-ilink | ⭐ | 📋 待对照现有实现 |
| 2 | 微信 ClawBot API 协议解析（腾讯 openclaw-weixin） | https://github.com/codeenxi/weixin-ClawBot-API | ⭐ | 📋 待读 |
| 3 | weixinProxy (Node.js) TS 类型定义——Rust 结构体设计参考 | https://github.com/AndySkaura/weixinProxy | ☆ | 📋 待参考 |
| 4 | 图片/文件加密踩坑（asset/download + 解密 key） | https://www.cnblogs.com/yaolin1228/p/20739268 | ⭐ | 📋 必须实现 |
| 5 | weixin-clawbot-bridge（Webhook 推送桥接） | https://github.com/cooooooooooode/weixin-clawbot-bridge | ☆ | 📋 待评估 |
| 6 | 多 Agent 编排 + 本地知识图谱 | https://github.com/rowboatlabs/rowboat | ☆ | 📋 待评估 |
| 7 | 可回放/回滚的自进化执行链路（三引擎一体） | https://hub.baai.ac.cn/view/57076 (HugAgentOS) | ☆ | 📋 待读 |
| 8 | DeepSeek + 插件化自进化桌面应用（同源竞品） | https://github.com/ahamoment-101/Open-DeepSeek-Harness-Desktop | ⭐ | 📋 待对比 |
| 9 | 桌面自动化同名项目（clawd-on-desk） | https://github.com/rullerzhou-afk/clawd-on-desk | ☆ | 📋 待定位差异化 |
| 10 | Windows 微信 + 桌面 AI 管家一体化 | https://github.com/Bxxxboo/Friday-WeChat-Windows-AI-Butler | ☆ | 📋 待读 |

---

## UI/UX

| # | 灵感 | 来源 | 关联 | 状态 |
|---|------|------|------|------|
| 1 | Compose Multiplatform 跨端聊天客户端交互设计 | https://github.com/succlz123/DeepCo | · | 📋 待参考 |
| 2 | 桌面 + 消息渠道（语音/浏览器/IM）全景交互 | https://github.com/siddsachar/row-bot | ☆ | 📋 待参考 |

---

## 架构

| # | 灵感 | 来源 | 关联 | 状态 |
|---|------|------|------|------|
| 1 | 把「Rust 二进制瘦身」沉淀为可复用 agent skill | https://github.com/ccusage/ccusage (rust-binary-size SKILL) | ☆ | 📋 待参考 |
| 2 | Tauri + SvelteKit + 本地 Ollama 的前后台分层 | https://github.com/stormixus/openClaw-Desktop | ☆ | 📋 待参考 |
| 3 | 自改进 agent 框架编排范式（LangGraph） | https://github.com/Framework-Island/hyperagents | · | 📋 待参考 |
| 4 | 本地部署「AI 生命体」系统（自主学习/进化/感知） | https://github.com/liulang5945-netizen/taiji | ☆ | 📋 待参考 |
| 5 | 零数据冷启动自进化（Agent0 论文） | https://ar5iv.labs.arxiv.org/html/2511.16043 | · | 📋 待读 |
| 6 | 自改进工程实现拆解（Hermes Agent） | https://developer.aliyun.com/article/1730226 | · | 📋 待读 |

---

## 优先建议（Top 5，含两轮深挖）

1. **min-sized-rust release profile**：改 Cargo.toml 几行，32MB→8-10MB，零风险（但 opt-level="z" 会牺牲运行速度，需权衡）。
2. **rust-lld 链接器**：✅ 已完成，增量编译 14.8s→11.8s（-20%）。
3. **weixin-ilink SDK 对照补全 wechat.rs**：必须支持图片/语音/文件收发+解密，补齐 ClawDesk 核心差异化。
4. **Reflexion Agent 自进化循环**：Actor+Evaluator+Memory 架构简单，是自进化最小可行实现。
5. **cargo-bloat + cargo-nextest**：体积监控 + 测试提速，安装即用零配置。

---

## 本地视觉模型（识图）—— 2026-08-20 落地

> 目标：给 ClawDesk 的 `analyze_image` 工具加本地识图能力，不依赖云端视觉 API（GLM-5V）。
> 结论：**Qwen2.5-VL-7B（Q4_K_M 量化）+ llama-server 常驻服务**，6GB 显存即可，识图 1~15 秒，精度无损。

### 关键发现（重要，避免重蹈覆辙）

1. **`--no-mmproj-offload` 是陷阱**：它把视觉投影器(mmproj)不仅权重放 CPU，**连 ViT 计算也在 CPU 跑**，导致图片编码 29.7s。放回 GPU（默认 offload）后，编码 **29.7s → 1.3s（快 23 倍）**，精度 0 损失。
2. **不牺牲精度也能提速**：之前误以为只能「限 token / 降精度换速度」，实际只要 mmproj 上 GPU 即可。最优配置 `-ngl 22`（主体 22 层 + mmproj 全 GPU，6GB 显存刚好）。
3. **常驻服务省掉冷加载**：每次 CLI 冷启动要重载 4.5GB 模型（~5s），用 `llama-server` 常驻后，端到端从 47s → 3~15s。
4. **环境已有 Ollama**：本机 `D:\AppData\ollama` 已有 qwen2.5vl:3b、minicpm-v-4.6 等 6 个模型，可直接复用（但 3B 精度低于 7B，最终选 7B）。

### 模型选型对比

| 模型 | 量化 | 体积 | 中文识图/OCR | 6GB 显存 | 取舍 |
|------|------|------|------|------|------|
| Qwen2.5-VL-7B | Q4_K_M | 4.4GB + mmproj 1.3GB | ⭐ 最强 | ✅ 刚好 | 本模块最终选择 |
| Qwen2.5-VL-3B (ollama) | — | 3.2GB | 良好 | ✅ 宽松 | 精度略逊，速度快 |
| MiniCPM-V-4.6 (ollama) | — | 1.5GB | 中 | ✅ 很宽松 | 轻量备选 |
| Moondream2 (1.8B) | 4-bit | 1.5GB | 弱（中文差） | ✅ | 边缘设备用 |

### 基准数据（本机 RTX 2060 6GB + Ryzen 4800H）

- 图片编码（mmproj GPU）：29.7s → **1.3s**（真实复杂截图）
- 端到端识图（含冷加载 CLI）：47s → **26s**
- 端到端识图（llama-server 常驻）：**简单图 3.1s，复杂截图 14.6s**
- Rust 集成测试实测：`local_vision_fallback` 7.4s / 250 字符

### 相关链接

- 模型/量化：https://huggingface.co/lmstudio-community/Qwen2.5-VL-7B-Instruct-GGUF （bartowski 官方量化，含 mmproj）
- 运行引擎：https://github.com/ggml-org/llama.cpp （预编译 Windows CUDA 版 `llama-b10507-bin-win-cuda-13.3-x64.zip`）
- HF 镜像（国内加速）：https://hf-mirror.com
- 低显存高分辨率优化参考：https://github.com/ggml-org/llama.cpp/issues/17801 （`deepshnv/mtmd_vram_opti`：权重留 CPU 但计算 stream 到 GPU）
- 视觉编码器设备选择 PR：https://github.com/ggml-org/llama.cpp/pull/14236

### 已排除的方向（评估后不采用）

- **thecodacus/llama.cpp fork**（https://github.com/thecodacus/llama.cpp）：只针对 **MoE 模型 + CPU offload** 场景（`GGML_CUDA_REGISTER_HOST` / `GGML_SCHED_PREFETCH_EXPERTS` / pin CPU weights 三连），对**稠密 VLM（Qwen2.5-VL-7B）+ 显存够装**的场景**零作用**。混淆点：它 README 标榜的 MoE offload 加速 64%，仅适用于专家权重超显存的 MoE 模型。

### 接入实现

- `analyze_image.rs` 新增 `local_vision_fallback()`：云端视觉未配置/失败时，自动探测 `http://127.0.0.1:8088/health`（300ms 超时），在线则走本地 Qwen2.5-VL-7B，不在线则降级元信息（零配置）。
- 环境变量 `CLAWDESK_DISABLE_LOCAL_VISION=1` 可显式禁用本地视觉。
- **自动启动（零配置）**：`commands/llama_server.rs` 封装 llama-server 生命周期——ClawDesk 启动时自动 `spawn`（后台、非阻塞，检测模型文件存在 + 端口占用），退出 `RunEvent::Exit` 时自动 `kill` 回收。关键实现细节：`spawn` 时必须 `current_dir` 设为 llama-server 所在目录（新版 llama.cpp 预编译版把主逻辑拆到 `llama-server-impl.dll` + `ggml-cuda.dll` 等，不设 cwd 会找不到 DLL）。实测 8 秒就绪。
- 手动启动命令（兜底）：`llama-server -m Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf --mmproj mmproj-model-f16.gguf -ngl 22 -c 2048 --host 127.0.0.1 --port 8088`

---

*最后更新：2026-08-20（本地视觉模型 Qwen2.5-VL-7B 接入落地 + 识图速度优化）*