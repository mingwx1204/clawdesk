# 诗妍「活人感」灵感清单

> 来源：GitHub / 论文热榜搜索。记下备选，实现前可与用户确认。

## 已实现（8 层灵魂）
1. persona_traits —— OCEAN 人格底座 + 演化 + 深夜调幅 + 外向性调制
2. living_state —— 生活时间线（时序连贯）
3. life_narrative —— 每日睡前巩固 + 梦境回忆
4. relationship —— 主观关系叙事
5. detail_memory —— 遗忘曲线 + used 加权
6. drives —— 6 驱动力（连接/怕被遗忘/分享欲/安全感/玩心/倔强）
7. mood —— 瞬间情绪 + 深夜孤独放大
8. persona.md —— 静态身份（诗妍 大二女大学生）

## 备选（待实现，按优先级）
### 2) 昼夜节律作息层（circadian.rs）—— 推荐优先
- 来源：Hololand .ai-schedule.yml + dev.to「I gave an AI a sleep schedule, dreams, and a personal blog」
- 命中诉求：她有自己的时间线、时间同步现实世界
- 内容：真实作息锚点（起床/犯困/白天在哪/周末作息），该困时困该清醒时清醒

### 1) 效价-唤醒（valence/arousal）情绪状态机
- 来源：论文 2605.03882（affective state machine）+ MATE 情绪架构
- 内容：用 (效价,唤醒) 二维坐标统一情绪，平滑漂移而非离散跳变

### 3) 焦虑→释然（anxiety→relief）情绪闭环
- 来源：synth-mind（Synthetic Mind Stack，六模块含 predictive dreaming / anxiety→relief）
- 内容：等待被回应→得到回应→安心 的张力释放，情绪有起有落

## 其它参考项目（可继续挖）
- RogueCtrl/OpenClawDreams —— 后台反思 + 夜间梦境循环 + 加密记忆
- LucieEveille/kiwi-mem —— AI 伴侣记忆（向量搜索/记忆热度/Dream 睡眠整合/日历层级摘要）
- kase1111-hash/synth-mind —— NLOS 六心理模块
- kellyvv/OpenHer —— 神经驱动力涌现人格（已参考其 drives 层）
- BytePioneer-AI/clawmate —— OpenClaw 角色伴侣
- fangligamedev/openclaw-companion-memory —— 本地化记忆
- kov25a「Joint Personality-Emotion Framework」—— 人格-情绪一致对话
