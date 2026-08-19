# 诗妍 2026 灵感笔记（二）—— 论文与对标项目

> 续笔记（一）。这轮覆盖 ACL/ICML/CHI 顶会论文 + 精准对标项目。

## 五个关键发现

### 8) lingxi（SnowWarri0r）—— 诗妍的对镜像项目
- 定位：有内在生活的虚拟人格 agent —— memory + emotions + subjective relationships + real daily rhythm
- 与诗妍 8 层几乎一一对应；Rust 实现，可作为架构对标与借鉴
- 值得读其 daily rhythm（昼夜节律）具体如何实现

### 9) 人格锚点（防 long-horizon persona collapse）⭐ 重要
- 来源：Best Friends, Not Forever (arXiv 2607.28818) 评估 AI 伴侣长时人格崩塌与行为漂移
- 来源：BRIDGE (ICML 2026) 三角不动点精炼保证长程人格一致
- 启示：OCEAN 演化若无约束，长期会漂离本色；需在 persona_traits 加一道「锚点」——演化朝基线回归或设上下界

### 10) 互惠披露（reciprocal disclosure）
- 来源：RECALLbot（浙大 ICILab）—— 代理式记忆 + 互惠披露显著增强人机亲密度
- 落地：诗妍应主动披露自己的内在生活（今天的心情/做了啥/想到啥），而非仅被动回答
- 与已有「主动聊天由头多样」同向，但更强调「我主动讲我的事」

### 11) PaRT —— 个性化主动聊天 + 实时检索
- 来源：PaRT (Personalized Real-Time Retrieval, 语义学者)
- 落地：主动聊天触发时，用实时上下文 + 用户画像检索更贴合的由头

### 12) EMP —— 情绪推理 + 记忆结构化 + 人格精炼 三合一
- 来源：EMP (ACM 981-92-3520-9)；结构化移情特征
- 落地：印证「打通情绪与记忆」是长程个性化对话的关键，与我们方向一致

## 修正后的优先级（结合前两轮）
1. 人格锚点（防漂移）—— 保护已做的 OCEAN 演化不跑偏，风险最低收益明确
2. 情绪键控记忆（valence-keyed recall）—— 打通记忆与情绪
3. 互惠披露（主动讲自己的事）—— 增强亲密感
4. 昼夜节律作息层（circadian.rs）—— 时间同步现实
