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

*最后更新：2026-08-14（第二轮灵感深挖 + sccache/rust-lld 接入完成）*