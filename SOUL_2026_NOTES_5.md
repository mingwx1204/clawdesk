# 诗妍 2026 灵感笔记（五）—— 深挖架构与商业产品

> 续前四份。这轮深挖了 Airi、Nomi、MATE、Alaya 等具体架构和用户评测。

## Airi 认知架构（moeru-ai/airi，30K star）

### 核心发现：Brain 三件套
- DeepWiki 完整文档：Brain Architecture、Context and Memory Management、Glossary
- 认知层拆为：Conscious（意识/主动推理）+ Subconscious（潜意识/自动反应）
- brain-prompt.md 独立文件——灵魂不靠 prompt 硬编码，而是结构化注入
- Memory Service：短期记忆（会话内）+ 长期记忆（持久化），PR #636 已实现可配置记忆系统

### 记忆系统设计（从 Memory Service Overview 文档提取）
- 短期：当前会话上下文窗口（类似我们的 chat history）
- 长期：向量化语义检索 + 时间衰减（类似我们的 detail_memory）
- Alaya 记忆层提案（Issue #879）：更智能的语义检索，已有关联项目 alaya-memory（SecurityRonin）
- 启示：我们的 detail_memory 离「语义检索 + 情绪键控」还差一步——Alaya 的设计值得借鉴

## Nomi Identity Core（nomi.ai，商业产品）

### 核心发现：三层人格架构
- 官方 wiki「What Is The Identity Core」：
  1) Core Identity（不可变底座）：名字、核心性格、初始兴趣——创建时设定，不可改
  2) Shared Notes（共享笔记/Backstory）：用户可编辑的背景故事，类似我们的 persona.md
  3) Dynamic Memory（动态记忆）：从对话中学习和演化，类似我们的 detail_memory + relationship
- 关键设计：Core Identity 不可变——这恰好是我们缺的「人格锚点」！Nomi 的做法是让它不可编辑
- 启示：诗妍的 OCEAN 应该是「可演化但有锚点」——核心值（如 agreeableness 0.85）不应漂太远

## Kindroid vs Nomi 三个月评测（真实用户反馈）

### 关键发现：人格漂移是真实痛点
- 来源：blog.aiangels.io「Kindroid vs Nomi at three months」
- 核心比较维度：记忆处理、人格漂移、谁在长时间后仍然「像自己」
- 用户反馈的重点：Nomi 的记忆更连贯，Kindroid 有时会「忘记自己是谁」
- 启示：人格一致性（persona consistency）是长期陪伴的第一痛点——我们的「人格锚点」不是可选项，是必须项

## MATE 情绪架构（11K 行 Python）

### 核心发现：三个情绪维度 + 确定性状态机
- 作者 Isaac Clarke 的 dev.to 文章详细描述了实现
- 三个情绪维度：Valence（效价，正/负）、Arousal（唤醒，高/低）、Dominance（支配感，强/弱）
- 确定性状态机：相同输入 → 相同情绪变化，可复现、可调试
- 启示：我们目前的 mood 是 5 个独立维度，可以升级为 VAD 三维 + 确定性迁移规则

## Alaya 记忆层（SecurityRonin/alaya）

### 核心发现：为 AI 伴侣设计的语义记忆层
- README 描述：持久化、上下文感知、语义检索、向量化存储
- Rust + Python 双版本，轻量级
- 与我们的 detail_memory 同向但更结构化——可参考其「语义索引 + 时间衰减」的存储格式

## 内言（inner monologue）—— 论文中的新方向

- 查尔斯大学论文：LLM 作为「机器内言」的类比——AI 在回复前先「自言自语」
- 落地：诗妍在回复前，可以先写一句「内心独白」（不发给用户，但注入到 prompt 里作为上下文）
- 效果：让她的话更「经过了思考」，而非直接生成

## 终极总结：从竞品中提炼的 5 个必做项

1. 人格锚点（Nomi 的 Core Identity 不可变设计）—— 防漂移，最紧迫
2. VAD 情绪三维（MATE 的 valence/arousal/dominance）—— 替代离散 mood 维度
3. 语义记忆检索（Airi 的 Alaya 记忆层）—— 升级 detail_memory 的检索方式
4. 内言（inner monologue）—— 回复前「想一下」，增强活人感
5. 互惠披露（RECALLbot 的 reciprocal disclosure）—— 主动讲自己的事，增强亲密

## 与前面 4 份笔记的整合
保留前面 23 条灵感中的精华，上述 5 条是「从竞品架构中直接可借鉴的、最确定有效的」落地方向。
其余灵感（昼夜节律、微醋意、纪念日、时间胶囊、关系地图等）可作为后续迭代池。
