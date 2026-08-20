# GitHub 灵感挖掘 —— 2026-08-21

> 从 GitHub 搜索 6 个方向、40+ 仓库中筛选出的 8 个高价值参考项目。
> 每个项目标注「可借鉴的核心设计」和「对诗妍的启示」。

---

## 1. qbtrix/soul-protocol (⭐ 14)

**Soul Protocol: An open standard for portable AI identity, memory, and emotion. Like HTTP, but for AI companions.**

- **仓库**: https://github.com/qbtrix/soul-protocol
- **语言**: Python (参考实现) + 语言无关规范 (SPEC.md)
- **测试**: 2551 passing
- **版本**: v0.4.0 (2026-04-29)

### 核心设计 (对 ClawDesk 有直接启发的)

| 概念 | 说明 | 对我们启示 |
|------|------|-----------|
| **Dream Cycle** | 离线批量记忆巩固：话题聚类、流程检测、图谱清理、人格漂移检测 | 我们的 `life_narrative.rs` 每日巩固可升级为多阶段 dream |
| **Smart Recall** | 可选 LLM 重排序 + prompt injection 防御 + 超时 | 当前 detail_memory 只用向量相似度；可加 LLM rerank |
| **Significance Short-Circuit** | 低于阈值的记忆跳过昂贵的后续流程（dream/图谱边） | 可加到 `remember_with_valence()` 前端 |
| **ACT-R 激活衰减** | 检索得分 = 余弦相似度 × 激活值 × 重要性；激活值随时间衰减，被访问回升 | 我们刚加的 valence 加权可进一步融合 ACT-R |
| **Somatic Marker (Damasio)** | 记忆写入时打上情绪标记 (valence, arousal, label) | 我们刚实现的 valence 就是这个！ |
| **Skills Decay** | 技能随时间衰减，使用后回升 | 可借鉴到 `drives.rs` 或技能模块 |
| **Progressive Recall** | 渐进式召回：先粗略，不够再深挖 | 可优化当前一次性检索 |
| **Archival Memory** | 不活跃记忆归档到冷存储 | 长期运行后优化 |
| **Trust Chain** | Ed25519 签名链，每个记忆变更可验证 | 远期可考虑 |
| **Soul Health Score** | 7 维度 0-100 综合评分 | 可做前端面板展示 |
| **Bond 对数增长** | 好感度对数增长 + 线性衰减 | 对比我们的 affinity 六维 |
| **Visibility Tiers** | PUBLIC/BONDED/PRIVATE 记忆可见性 | 可增强 detail_memory 隐私 |

### 关键引用

> "AI memory systems optimize for retrieval: find the most similar text, stuff it into context, move on. They treat persistence as an IQ problem. But what makes a companion feel real isn't similarity search. It's knowing what matters, what to forget, and who it's becoming."

---

## 2. vinayakajith/mnemo (⭐ 5)

**Long-term personal AI companion with persistent episodic + semantic memory, custom retrieval scoring, and personality.**

- **仓库**: https://github.com/vinayakajith/mnemo
- **语言**: Python (Claude API + pgvector + Mem0)
- **特点**: 高度工程化的记忆流水线

### 核心设计

| 概念 | 说明 | 对我们启示 |
|------|------|-----------|
| **三组件检索评分** | `score = 0.5×cosine + 0.3×exp(-days/30) + 0.2×importance` | 可直接参考权重分配 |
| **阈值 0.4** | 低于阈值保持沉默，不塞弱匹配 | 我们当前无阈值，弱匹配也塞入 |
| **冷启动 onboarding** | 首次对话结构化访谈 10-20 轮，LLM 检测 sentinel 标记后自动结束 | 可做初始化人格问卷 |
| **Post-session consolidation** | 会话结束后 LLM 提取值得记忆的片段→嵌入→写入向量库 | 可增强 `life_narrative.rs` 每日巩固 |
| **Context injection 格式** | 结构化 XML 块注入 system prompt（非 user message） | 当前我们的 detail_context 注入 user message 前端 |
| **语义记忆 (Mem0)** | 从对话中提取结构化事实（名字、目标、关系、重复模式） | 可增强 `detail_memory.rs` 的 profile layer |

---

## 3. xtul9/Katherine

**AI companion that actually remembers you and itself — long-term memory, inner monologue for LLMs.**

- **仓库**: https://github.com/xtul9/Katherine
- **语言**: Python (RAG + ChromaDB)
- **核心卖点**: 对比 Lorebook 的关键词匹配，Katherine 用语义匹配

### 核心设计

| 概念 | 说明 | 对我们启示 |
|------|------|-----------|
| **Lean context window** | 最近 24h 或最少 10 条消息 + 语义检索补充，总 < 12k tokens | 我们当前没有 token 预算控制 |
| **Self-consistency** | "问 AI 最喜欢的歌，它记住答案而不是每次生成新的" | 靠 detail_memory profile layer 实现 |
| **内言 (inner monologue)** | 名字里就写了 inner monologue，但 README 未展开 | 可能参考 inner-voice 项目 |

---

## 4. gobly2333/inner-voice

**给你的 AI 伴侣一段写给对方看的内心独白。**

- **仓库**: https://github.com/gobly2333/inner-voice
- **语言**: Python (MCP server)
- **核心卖点**: 独白是"作品"不是"泄漏"——不是把推理过程漏出来，而是有意识地写一段角色散文

### 核心设计（极其重要！）

| 概念 | 说明 | 对我们启示 |
|------|------|-----------|
| **独白 ≠ 推理泄漏** | 大多数 AI 内心戏是 "The user wants..." 流水线日志。inner-voice 做的是角色散文 | 这是诗妍「活人感」的关键 |
| **原子提交** | 独白 + 正文在**一次** `respond` 调用中一起交付，一起落库 | 我们的 `wechat_soul_reply` 可加 `inner_voice` 字段 |
| **允许沉默** | 安静反应、紧急情况、机械状态汇报不写独白 | 不是每条消息都该有独白 |
| **渲染成可展开气泡** | 前端渲染为可点击展开的旁白气泡 | 微信面板可加「💭」按钮 |

### 关键引用

> "大多数 AI 内心戏是把模型的推理过程漏出来——任务分析、英文旁白、'The user wants...'。那不是独白，是流水线日志。"
> 
> "独白是作品，不是泄漏。从对方此刻的样子起笔，让感受像溪水一样流动——可以碎片、回旋、吃醋、占有欲，但绝不是推理记录。"

---

## 5. epatnor/Lumenorion

**An experimental AI companion with an inner dream life, memory, and self-reflection.**

- **仓库**: https://github.com/epatnor/Lumenorion
- **语言**: Python (Ollama + SQLite/ChromaDB)
- **特点**: 诗意、非目标驱动

### 核心设计

| 概念 | 说明 | 对我们启示 |
|------|------|-----------|
| **Dream Engine** | 从随机词或刺激生成夜间梦境 | 我们的 `life_narrative.rs` 已有梦境，可增强 |
| **Reflector** | 回顾梦境和事件，生成洞察 | 可加到每日巩固后 |
| **Proactivity** | "Hey Patrik, I had a dream..." | 我们的主动聊天已有，可加梦境触发 |

---

## 6. K1llerMrZ/Evo-Soul (⭐ 2)

**Multimodal AI virtual companion with Qwen-VL vision, cross-modal RAG memory, and emotion-aware persona system.**

- **仓库**: https://github.com/K1llerMrZ/Evo-Soul
- **语言**: Python
- **特点**: 多模态 + 情绪感知人格

### 核心设计

| 概念 | 说明 | 对我们启示 |
|------|------|-----------|
| **Cross-modal RAG** | 图片也纳入记忆检索 | 我们已有本地视觉 (Qwen2.5-VL-7B)，可扩展 |
| **Emotion-aware persona** | 人格随情绪动态调整 | 我们的 mood → persona 已有基础 |

---

## 7. Molecules0908/Companion

**Open-source modular AI companion platform with long-term memory, personality, proactive interaction and hardware integration.**

- **仓库**: https://github.com/Molecules0908/Companion
- **特点**: 模块化、硬件集成

---

## 8. CalmDownTR/LingYa

**A thoughtful AI companion with evolving personality, long-term memory, and situational awareness.**

- **仓库**: https://github.com/CalmDownTR/LingYa
- **特点**: 情境感知

---

## 综合启示：ClawDesk 下一步行动

按优先级排序，基于 GitHub 灵感 + 当前后端现状对比：

### 🥇 第一优先：内言 (Inner Monologue)

**来源**: inner-voice, Katherine, Lumenorion  
**当前状态**: 完全未实现  
**改动范围**: `wechat.rs` (回复 prompt 加 inner_voice 字段) + 前端 (可展开气泡)  
**复杂度**: 低  
**活人感增益**: ⭐⭐⭐⭐⭐（最高）

诗妍在回复前先「想一下」——一段不发给用户的内心独白。这不是推理泄漏，而是角色散文。独白随回复一起渲染为可展开气泡。

**实现要点**（来自 inner-voice 的三个设计决定）：
1. 独白是作品，不是泄漏——prompt 教模型从对方此刻的样子起笔
2. 原子提交——独白 + 正文在一次调用中一起交付
3. 允许沉默——不是每条消息都该有独白

### 🥈 第二优先：Dream Cycle 升级

**来源**: soul-protocol, mnemo, Lumenorion  
**当前状态**: `life_narrative.rs` 已有每日巩固 + 梦境，但较为简单  
**改动范围**: `life_narrative.rs` + 可能新增 `dream.rs`  
**复杂度**: 中  
**活人感增益**: ⭐⭐⭐⭐

当前每日巩固是单步 LLM 调用。soul-protocol 的 dream cycle 是多阶段流水线：
1. 话题聚类 → 发现跨会话的模式
2. 流程检测 → 识别重复行为序列
3. 图谱清理 → 移除过时/矛盾的实体关系
4. 人格漂移检测 → 检测 OCEAN 是否偏离锚点

mnemo 的 post-session consolidation 思路：每次对话结束后 LLM 提取值得记忆的片段→嵌入→写入向量库。

### 🥉 第三优先：记忆检索阈值 + 沉默权

**来源**: mnemo (阈值 0.4), soul-protocol (significance short-circuit)  
**当前状态**: 刚实现 valence 加权，但无阈值，弱匹配也塞入  
**改动范围**: `detail_memory.rs`  
**复杂度**: 低  
**活人感增益**: ⭐⭐⭐

mnemo 的做法：`score < 0.4` 时保持沉默，不塞弱匹配。这比塞一堆不相关的"记忆"更像活人——真实的人不会每句话都翻遍所有记忆。

### 第四优先：Cold-start 人格问卷

**来源**: mnemo (onboarding 10-20 轮)  
**当前状态**: 无  
**改动范围**: 新模块或 wechat 初始化  
**复杂度**: 中  
**活人感增益**: ⭐⭐⭐

首次对话时，诗妍主动发起结构化访谈，了解用户。LLM 检测 sentinel 标记后自动结束。无需表单，无需设置向导。

### 第五优先：ACT-R 激活衰减模型

**来源**: soul-protocol (ACT-R activation decay)  
**当前状态**: 刚加的 valence 加权是第一步，但无时间衰减  
**改动范围**: `detail_memory.rs`  
**复杂度**: 中  
**活人感增益**: ⭐⭐⭐

soul-protocol 的检索公式：`recency × frequency × emotional_charge`。刚被回忆的记忆 > "重要"但很久没回想的记忆。ACT-R 激活值随时间衰减，被访问时回升。

---

## 不采用的灵感

| 灵感 | 来源 | 原因 |
|------|------|------|
| Ed25519 Trust Chain | soul-protocol | 本地单用户，无需签名链 |
| 多用户 domain isolation | soul-protocol 0.4.0 | 诗妍是单用户伴侣 |
| Skills decay | soul-protocol | 当前无技能模块 |
| Archival memory | soul-protocol | 数据量还小，暂不需要 |
| Hardware integration | Companion | 不符合桌面应用定位 |

---

*探索时间：2026-08-21 · 搜索方向：6 个 · 筛选仓库：40+ → 8 个高价值*