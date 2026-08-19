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
| 4 | dev profile 依赖 opt-level=2（自己代码=0），编译一次运行快 | 本地分析 | ⭐ | 🔄 进行中 |
| 5 | sccache 编译缓存 + mold/lld 链接器（Windows 用 lld-link） | Rust 官方论坛 | ⭐ | 📋 未安装，需下载二进制 |
| 6 | release 加 opt-level=3 + panic=abort 极限优化 | Tauri 官方 size 文档 | ⭐ | ✅ 已完成 |
| 7 | cargo-nextest 替代 cargo test（并行测试，404s→~200s） | 社区最佳实践 | ☆ | 📋 未安装 |
| 8 | Tauri 官方 App Size 优化指南（strip/lto/panic/codegen-units） | https://v2.tauri.app/concept/size/ | ⭐ | ✅ 已应用核心项 |
| 9 | Tauri v2 性能与 bundle 体积优化专题（前端+后端整体瘦身） | https://www.oflight.co.jp/en/columns/tauri-v2-performance-bundle-size | ☆ | 📋 待读 |

---

## 功能特性

| # | 灵感 | 来源 | 关联 | 状态 |
|---|------|------|------|------|
| 1 | iLink Bot 协议参考：长轮询 + markdown 过滤 + ACL 配对设计 | https://github.com/zongrongjin/weixin-ilink | ⭐ | 📋 待对比现有实现 |
| 2 | 微信 ClawBot API 协议解析（腾讯 openclaw-weixin） | https://github.com/codeenxi/weixin-ClawBot-API | ⭐ | 📋 待读 |
| 3 | 自进化 Skills 机制（技能自动生成+评测） | https://github.com/wizos/Abu-Cowork | ⭐ | 📋 待借鉴 |
| 4 | 无训练增量学习记忆库（每次任务自动学习） | https://github.com/C10udsea/evolvebank | ☆ | 📋 待借鉴 |
| 5 | 4 层分层记忆（工作/情景/语义/程序性，仿人类认知） | https://github.com/rohitpatill/ace-autonomous-agent | ☆ | 📋 待借鉴 |
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

## 优先建议（Top 3）

1. **微信集成**：weixin-ilink + weixin-ClawBot-API 是同源协议参考，对齐长轮询 + markdown 过滤 + ACL 配对。
2. **自进化**：Abu-Cowork（技能）+ evolvebank（记忆）+ ace-autonomous-agent（分层记忆）组合覆盖 ClawDesk 三层。
3. **性能/体积**：以 Tauri 官方 size 文档为基准，把瘦身沉淀为可复用 skill。

---

*最后更新：2026-08-14 由 GitHub 灵感搜索子代理汇总*
