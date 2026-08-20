# 诗妍 vs eros-engine —— 架构对比与可借鉴点

> 来源：直接读取 eros-engine 官方文档（docs/architecture.md、memory-layers.md、affinity-model.md、ghost-mechanics.md、README.md）
> eros-engine 是 AGPL v3 的 Rust AI 伴侣引擎，含 core/store/llm/server 四 crate，是我们最贴合的参考实现。

## 一、七大基础（eros-engine）vs 诗妍当前 8 层

| eros-engine | 诗妍现状 | 差距
|---|---|---|
| 双层记忆（Profile 稳定事实 + Relationship 会话回调） | detail_memory 单层 + relationship 分开 | 我们已分两层，但 profile 层缺「跨会话稳定事实」显式抽象
| 进化好感度（6 维 affinity，分级写入+阻尼+时间衰减） | relationship.jsonl 记录时刻 + OCEAN 演化 | 我们的关系没有「数值维度」，只有叙事；缺分级/阻尼/衰减
| Persona 决策引擎（PDE，生成前先选动作+内在状态） | 主动聊天由头随机 + vibe 概率 | 我们缺「生成前决策状态机」，vibe 是概率不是决策
| Ghost 机制（可已读不回，有限耐心有限好奇） | 无 | ⭐ 完全没有！我们永远回复
| 结构化用户洞察（insight 可查询画像） | detail_memory 的 used 加权 | 缺事实抽取为结构化画像
| 模拟世界（persona 有幕后生活，世界导演演化关系图） | living_state 时间线 | 缺「多角色 + 关系图 + 每日脚本」
| 语音中断（barge-in） | 无 | 远期

## 二、最值得借鉴的三个设计（按价值排序）

### ⭐ 1. Ghost 机制（可已读不回）
- 核心洞察：永远回复 = 用户会写低质量消息；有限耐心 + 有限好奇 = 关系真实
- ghost_score = (1-intrigue)×0.4 + (1-patience)×0.4 + tension×0.2
- 四层保护：前10条不 ghost / 连续2次后第3次不 ghost / 1小时内冷却 / 阈值0.65（曾ghost则0.85）
- 对我们：诗妍可以「暂时没回」，这会极大增强「她有自己的状态」的活人感

### ⭐ 2. 六维 affinity 好感度模型
- 6 维：warmth/temperature、trust、intrigue、intimacy、patience、tension（只 4 条线轴 + 2 个派生端点）
- trust 和 intimacy 不衰减（「深」维度）；intrigue 每天 -0.01、tension 每天 -0.005（时间衰减）
- 写入是「分级」而非数值：judge 报告粗粒度 grade，引擎转成数值再 damping + gating
- 对我们：relationship 层应从「叙事记录」升级为「6 维数值 + 分级写入 + 衰减」

### ⭐ 3. 双层记忆（Profile vs Relationship 的语义分离）
- Profile（instance_id=NULL）= 跨会话稳定事实（对花生过敏），任何 persona 都该知道
- Relationship（instance_id=uuid）= 本会话回调（Aria 说今晚读 Bishop），其它 persona 不该知道
- 关键洞察：persona 稳定性 ≠ 关系亲密性，必须分层存
- 对我们：detail_memory 缺这个「事实 vs 时刻」的语义区分

## 三、架构分层启示

eros-engine 的严格分层：core（纯领域，零 IO，可单测）→ llm / store（独立集成）→ server（胶水）
诗妍现状：8 层都在 src-tauri/src 下，领域逻辑与 IO 耦合
改进方向：把 persona_traits/drives/mood/affinity 抽成纯领域 crate，可快速单测（我们现在已有测试，但测试里 IO 和领域混在一起）

## 四、行动建议（最终版，可开始实现）

1. Ghost 机制 —— 最高 ROI，直接增强「她有自己的状态」
   实现：基于 drives 的「玩心/连接」+ mood 的「焦虑/疲惫」算一个 ghost_score，加保护层

2. 六维 affinity 升级 relationship —— 把叙事记录升级为可衰减的数值维度

3. 记忆语义分层（Profile 稳定事实 / Relationship 会话时刻）—— 让 detail_memory 更「像人」

4.（已有）OCEAN 人格锚点 —— 对应 eros-engine 的「persona 决策引擎保持人设」，仍需做
