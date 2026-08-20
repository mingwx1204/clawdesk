# 诗妍 2026 灵感笔记

> 来源：GitHub 热榜 / 论文。专注「活人感」的两个硬骨头——记忆、心。

## 四个最有价值的 2026 发现

### 4) 情绪键控记忆（valence-keyed memory）⭐ 最推荐
- 来源：Circular Associative Memory (zenodo 19051834) + MATE v8 + EAAM / MemoryConstellations
- 现有：detail_memory 用 used 次数 + recency 遗忘
- 升级：每个记忆节点同时存情绪效价向量；检索时用当下 mood 做键，回诉同情绪记忆
- 命中：AI 既没有记忆也没有心 —— 这招直接打通两个独立层

### 5) Soul as code（灵魂代码化，非 prompt）
- 来源：sitepoint 专题 Designing Souls for Code: Architecture of Moeru-AI/Airi
- 内核：Airi 拆成 cognitive / conscious 子模块；灵魂不靠 prompt，靠代码状态机
- 我们已走一半（8 层代码层），下阶段可收敛成 感觉层/认知层/表达层 三档

### 6) Persona-First
- 来源：OpenHuman (senamakel/openhuman，12.5K star，personality-first agent)
- 指向：人格先于工具；本地优先；它了解你的一生
- 与我们的 OCEAN 底座同向，可借鉴其 人生时间轴 + 人格叙事

### 7) 效价向量 + 精度标量
- 来源：arXiv 2603.29023（每个节点带 valence vector + precision scalar）
- 内容：回忆取 top-k 时，按 当下 mood 与记忆情绪距离 × 记忆可信度 加权排序

## 我们的优势（相对以上项目）
- 8 层全 Rust 代码化，稳定可靠
- OCEAN 可演化，非静态 personality
- 主动聊天已到位（N.E.K.O / AIRI 核心卖点我们有）
- 昼夜 / 情绪 / 驱动力统一 drift 机制
- 差的东西：记忆与情绪尚未打通 —— 这是下一个关键跳跃点

## 推荐实施顺序
1. 情绪键控记忆（性价比最高，改动 detail_memory.rs 内部结构）
2. 实时情绪漂移（valence/arousal 状态机，目前是离散 mood 维度）
3. 之后再做其他
