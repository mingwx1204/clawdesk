<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/**
 * 设置面板 —— 五大标签页（文档 §四.3）：
 * ① 模型 API 配置  ② Agent 智能体参数  ③ MCP 工具管理
 * ④ 外观汉化配置   ⑤ 快照与安全配置
 * 所有控件双向绑定后端 settings（settings_get / settings_set，JSON 持久化）。
 */

defineProps<{ tz?: string }>();
const emit = defineEmits<{
  (e: "close"): void;
  (e: "keys", keys: { main?: string; vision?: string; image?: string }): void;
  (e: "tz", v: string): void;
}>();

// ── 设置结构（与后端 AppSettings camelCase 逐字段镜像）──
interface AppSettings {
  model: string;
  modelEndpoint: string;
  visionModel: string;
  visionEndpoint: string;
  imageModel: string;
  imageEndpoint: string;
  agentEnabled: boolean;
  agentMode: string;
  maxRounds: number;
  compactionThreshold: number;
  maxToolRounds: number;
  mcpEnabled: boolean;
  visionMcpEnabled: boolean;
  imageMcpEnabled: boolean;
  chineseOnly: boolean;
  fontSize: number;
  micaEnabled: boolean;
  darkTheme: boolean;
  uiOpacity: number;
  snapshotEnabled: boolean;
  highRiskConfirm: boolean;
  sensitiveFilesEnabled: boolean;
  logPath: string;
  disabledSkills: string[];
  autoStart: boolean;
  selfEvolveEnabled: boolean;
  selfEvolveModel: string;
  selfEvolveAuto: boolean;
  selfEvolveThreshold: number;
  opencodeWatchEnabled: boolean;
  opencodeWatchEndpoint: string;
  opencodeWatchApiKey: string;
  opencodeWatchIntervalSecs: number;
  // ── ⑬ 朗读 / TTS（Edge TTS 神经网络拟人音色）──
  ttsEnabled: boolean;
  ttsVoice: string;
  ttsRate: number;
  ttsStyle: string;
  // ── ⑭ 《人是怎么样的》书目录（AI 的"灵魂档案"）──
  humanBookDir: string;
}

// API Key（仅内存态，不持久化 —— 与后端安全红线一致）
const mainKey = ref("");
const visionKey = ref("");
const imageKey = ref("");
// Key 保存提示
const keysSavedTip = ref("");

// ── Key 使用额度追踪（后端 check_balance，根据端点自动识别提供商）──
const mainBalanceText = ref("");
const mainBalanceLoading = ref(false);
const visionBalanceText = ref("");
const visionBalanceLoading = ref(false);
const imageBalanceText = ref("");
const imageBalanceLoading = ref(false);

function balanceLabel(provider: string): string {
  const map: Record<string, string> = { "opencode-go": "OpenCode Go", deepseek: "DeepSeek", siliconflow: "SiliconFlow", zai: "智谱", openai: "OpenAI" };
  return map[provider] ?? provider;
}

async function checkBalance(type: "main" | "vision" | "image") {
  const keyVar = type === "main" ? mainKey : type === "vision" ? visionKey : imageKey;
  const textVar = type === "main" ? mainBalanceText : type === "vision" ? visionBalanceText : imageBalanceText;
  const loadVar = type === "main" ? mainBalanceLoading : type === "vision" ? visionBalanceLoading : imageBalanceLoading;
  const endpointKey = type === "main" ? "modelEndpoint" : type === "vision" ? "visionEndpoint" : "imageEndpoint";
  const endpointUrl = (settings.value as any)?.[endpointKey] ?? "";

  const key = keyVar.value.trim();
  if (!key) {
    textVar.value = `请先填写 ${balanceLabel(type === "main" ? "deepseek" : type === "vision" ? "zai" : "siliconflow")} ${type === "main" ? "主模型" : type === "vision" ? "视觉模型" : "绘图"} Key`;
    return;
  }
  loadVar.value = true;
  textVar.value = "查询中…";
  try {
    const r = await invoke<any>("check_balance", { apiKey: key, endpoint: endpointUrl });
    const infos: any[] = Array.isArray(r?.balance_infos) ? r.balance_infos : [];
    if (r?.is_available === false) {
      textVar.value = "账户不可用";
    } else if (infos.length) {
      textVar.value = infos
        .map(
          (b) =>
            `${b.currency} ${b.total_balance}（赠额 ${b.granted_balance ?? 0} / 充值 ${b.topped_up_balance ?? 0}）`
        )
        .join("；");
    } else {
      textVar.value = "未获取到余额信息";
    }
  } catch (e) {
    textVar.value = `查询失败：${typeof e === "string" ? e : JSON.stringify(e)}`;
  } finally {
    loadVar.value = false;
  }
}

// ── 快照回滚面板（项目 6/9：snapshot IPC）──
interface SnapshotItem {
  id: string;
  original: string;
  snapshot: string;
  createdAt: string;
  size: number;
}

const snapshots = ref<SnapshotItem[]>([]);
const snapshotTip = ref("");
const snapshotLoading = ref(false);

const activeTab = ref<"model" | "agent" | "mcp" | "appearance" | "security" | "skills" | "system" | "selfEvolve">("model");
const settings = ref<AppSettings | null>(null);
const tip = ref("");
const saving = ref(false);
const error = ref("");

const TABS = [
  { id: "model", label: "模型 API" },
  { id: "agent", label: "Agent 参数" },
  { id: "mcp", label: "MCP 管理" },
  { id: "appearance", label: "外观汉化" },
  { id: "security", label: "快照与安全" },
  { id: "skills", label: "技能管理" },
  { id: "system", label: "系统运维" },
  { id: "selfEvolve", label: "自进化" },
] as const;

onMounted(async () => {
  await load();
  await loadKeys();
  loadSkills();
  loadSandbox();
  loadMcp();
  loadVoices();
  // ★ 重启后自进化设置仍为开启 → 自动重新初始化后端引擎
  //   （后端引擎是内存单例，重启即空；不重新初始化会导致手动进化报"未初始化"）
  if (settings.value?.selfEvolveEnabled) {
    await startEvolve();
    await loadEvolveStatus();
  }
  requestAnimationFrame(() => moveIndicator());
});

/** 回显后端内存态 API Key（密码框显示已存 Key）。 */
async function loadKeys(): Promise<void> {
  try {
    const k = await invoke<{ main?: string; vision?: string; image?: string }>("settings_get_keys");
    if (k?.main) mainKey.value = k.main;
    if (k?.vision) visionKey.value = k.vision;
    if (k?.image) imageKey.value = k.image;
  } catch { /* 静默 */ }
}

async function load() {
  try {
    settings.value = await invoke<AppSettings>("settings_get");
    applyTheme(!!settings.value.darkTheme);
    applyUiOpacity(settings.value.uiOpacity ?? 1);
  } catch (e) {
    error.value = `加载设置失败：${String(e)}`;
  }
}

// ── 🔊 朗读设置（Edge TTS 神经网络拟人音色） ──
interface TtsVoiceInfo {
  id: string;
  name: string;
  gender: string;
  desc: string;
  region: string;
  styles: string[];
}

const voices = ref<TtsVoiceInfo[]>([]);
const previewing = ref(false);
const previewTip = ref("");

/** 当前选中音色支持的语气风格。 */
const currentVoiceStyles = computed(() => {
  const id = settings.value?.ttsVoice || "zh-CN-XiaoxiaoNeural";
  return voices.value.find((v) => v.id === id)?.styles ?? [];
});

/** 语气风格中文标签。 */
function styleLabel(style: string): string {
  const map: Record<string, string> = {
    cheerful: "😄 开心",
    empathetic: "💗 温柔共情",
    calm: "😌 平静",
    gentle: "🌸 温和",
    serious: "📌 严肃",
    newscast: "📰 新闻播报",
    sad: "😢 悲伤",
    angry: "😠 生气",
    excited: "🎉 兴奋",
    fearful: "😨 害怕",
    lyrical: "🎵 抒情",
    "poetry-reading": "📖 诗歌朗诵",
  };
  return map[style] || style;
}

/** 加载音色列表。 */
async function loadVoices(): Promise<void> {
  try {
    voices.value = await invoke<TtsVoiceInfo[]>("tts_list_voices");
  } catch { /* 静默 */ }
}

/** 试听当前选中音色。 */
async function previewVoice(): Promise<void> {
  const s = settings.value;
  if (!s) return;
  previewing.value = true;
  previewTip.value = "合成中…";
  try {
    const ok = await invoke<{ audioBase64: string; bytes: number; voice: string }>("tts_speak", {
      text: "你好呀！我是你的朗读助手，听听这个音色怎么样？如果你喜欢，就在设置里选我吧。",
      voice: s.ttsVoice || "zh-CN-XiaoxiaoNeural",
      rate: s.ttsRate ?? 1.0,
      style: s.ttsStyle || "",
    });
    const audio = new Audio("data:audio/mpeg;base64," + ok.audioBase64);
    audio.onended = () => { previewing.value = false; previewTip.value = "✅ 试听完成"; };
    audio.onerror = () => { previewing.value = false; previewTip.value = "❌ 播放失败（网络异常？）"; };
    await audio.play();
    previewTip.value = "🔊 播放中…";
  } catch (e) {
    previewing.value = false;
    previewTip.value = `❌ 合成失败：${String(e)}`;
  }
}

/** 停止试听。 */
function stopPreview(): void {
  previewing.value = false;
  previewTip.value = "";
}

/** 应用深浅主题（html[data-theme]）。 */
function applyTheme(dark: boolean) {
  if (dark) document.documentElement.setAttribute("data-theme", "dark");
  else document.documentElement.removeAttribute("data-theme");
}

/** 应用界面不透明度（--ui-op 驱动所有玻璃框背景透明度）。 */
function applyUiOpacity(v: number): void {
  document.documentElement.style.setProperty("--ui-op", String(v));
}

/** 拖动滑块：实时预览（不落盘）。 */
function onUiOpacityInput(e: Event): void {
  applyUiOpacity(Number((e.target as HTMLInputElement).value) / 100);
}

/** 松手保存：落盘持久化。 */
function onUiOpacityChange(e: Event): void {
  const v = Number((e.target as HTMLInputElement).value) / 100;
  void patch({ uiOpacity: v });
}

/** 局部更新：将字段补丁发送后端，成功后用返回的最新设置回填。 */
async function patch(p: Record<string, unknown>): Promise<void> {
  saving.value = true;
  tip.value = "";
  try {
    settings.value = await invoke<AppSettings>("settings_set", { patch: p });
    tip.value = "✅ 已保存（即时生效）";
  } catch (e) {
    tip.value = `❌ 保存失败：${String(e)}`;
  } finally {
    saving.value = false;
  }
}

/** 保存 API Key（显式按钮触发，防止密码框 @change 误覆盖）。 */
async function saveKeys(): Promise<void> {
  const m = mainKey.value.trim();
  const v = visionKey.value.trim();
  const i = imageKey.value.trim();
  if (!m && !v && !i) {
    keysSavedTip.value = "请至少填写一个 Key";
    setTimeout(() => { keysSavedTip.value = ""; }, 2000);
    return;
  }
  try {
    const p: Record<string, unknown> = {};
    if (m) p.mainKey = m;
    if (v) p.visionKey = v;
    if (i) p.imageKey = i;
    settings.value = await invoke<AppSettings>("settings_set", { patch: p });
    emit("keys", { main: m, vision: v, image: i });
    keysSavedTip.value = "✅ Key 已保存";
    setTimeout(() => { keysSavedTip.value = ""; }, 2500);
    // 清空输入框（安全）：后端已持久化到 keys.enc
    mainKey.value = "";
    visionKey.value = "";
    imageKey.value = "";
  } catch (e) {
    keysSavedTip.value = `❌ 保存失败：${String(e)}`;
  }
}

function field(key: keyof AppSettings, value: unknown): void {
  void patch({ [key]: value });
}

// ── ⑥ 技能管理（方案 3）：skills_list / skills_set_enabled / skills_reload ──
interface SkillItem {
  id: string;
  description: string;
  enabled: boolean;
}

const skills = ref<SkillItem[]>([]);
const skillsTip = ref("");
const skillsLoading = ref(false);

async function loadSkills(): Promise<void> {
  skillsLoading.value = true;
  try {
    skills.value = await invoke<SkillItem[]>("skills_list");
    skillsTip.value = "";
  } catch (e) {
    skillsTip.value = `❌ 加载技能失败：${String(e)}`;
  } finally {
    skillsLoading.value = false;
  }
}

async function toggleSkill(id: string, enabled: boolean): Promise<void> {
  skillsTip.value = "";
  try {
    await invoke<boolean>("skills_set_enabled", { skillId: id, enabled });
    await loadSkills();
    skillsTip.value = `✅ ${id} 已${enabled ? "启用" : "禁用"}（即时生效）`;
  } catch (e) {
    skillsTip.value = `❌ 操作失败：${String(e)}`;
  }
}

async function reloadSkills(): Promise<void> {
  skillsLoading.value = true;
  skillsTip.value = "";
  try {
    const n = await invoke<number>("skills_reload");
    await loadSkills();
    skillsTip.value = `✅ 已重新扫描，共 ${n} 个技能`;
  } catch (e) {
    skillsTip.value = `❌ 重扫失败：${String(e)}`;
  } finally {
    skillsLoading.value = false;
  }
}

// ── 技能中文注释（三层）：① 精确映射表 ② 自动分词翻译（新增技能自动生效） ③ description 兜底 ──
const SKILL_ZH_MAP: Record<string, string> = {
  "skillhub:summarize": "文本/对话总结摘要",
  "skillhub:humanizer": "内容拟人化、去 AI 味改写",
  "skillhub:humanizer-zh": "中文内容拟人化改写",
  "skillhub:humanizer-zh-pro": "中文内容深度拟人化改写",
  "skillhub:ppt-generator": "PPT 演示文稿生成",
  "skillhub:ppt-crate": "PPT 生成（Crate 模板）",
  "skillhub:ppt-win32com-editor": "PowerPoint 编辑（Windows 组件）",
  "skillhub:slides-maker": "幻灯片制作",
  "skillhub:word-docx": "Word DOCX 文档处理",
  "skillhub:word-formatter": "Word 文档排版格式化",
  "skillhub:wps-office-suite": "WPS Office 办公套件操作",
  "skillhub:excel-formula-generator": "Excel 公式生成",
  "skillhub:excel-auto-zh": "Excel 表格自动化（中文）",
  "skillhub:excel-sync-bitable": "Excel 与飞书多维表格同步",
  "skillhub:ws-excel": "Excel 工作表操作",
  "skillhub:chinese-official-writing": "中文公文写作",
  "skillhub:official-document-skill": "公文/正式文档写作",
  "skillhub:gov-document-typesetting": "政务公文排版",
  "skillhub:patent-drafting-cn": "中文专利撰写",
  "skillhub:patent-disclosure-skill": "专利交底书撰写",
  "skillhub:litigation-docs-generator": "诉讼文书生成",
  "skillhub:hr-compensation-officer": "HR 薪酬专员（薪酬计算/分析）",
  "skillhub:tax-policy-knowledge": "税务政策知识库",
  "skillhub:meeting-and-brief": "会议纪要与简报生成",
  "skillhub:code-simplifier": "代码简化/重构",
  "skillhub:data-visualization": "数据可视化图表",
  "skillhub:diagram-builder": "图表/示意图绘制",
  "skillhub:smart-charts": "智能图表生成",
  "skillhub:ontology": "本体论知识建模",
  "skillhub:baidu-search": "百度搜索",
  "skillhub:web-tools-guide": "网页工具指南",
  "skillhub:topnews": "热点新闻聚合",
  "skillhub:weather": "天气查询",
  "skillhub:github": "GitHub 仓库操作",
  "skillhub:agent-browser": "智能体浏览器自动化",
  "skillhub:browser-automation-toolbox": "浏览器自动化工具箱",
  "skillhub:cn-financial-scraper": "中国金融数据抓取",
  "skillhub:content-collector": "内容采集器",
  "skillhub:douyin-copy-extract": "抖音文案提取",
  "skillhub:douyin-montage": "抖音视频混剪",
  "skillhub:douyin-brain-strategy": "抖音运营策略",
  "skillhub:douyin-image-generator": "抖音图片生成",
  "skillhub:douyin-script-optimizer": "抖音脚本优化",
  "skillhub:douyin-pro": "抖音全套运营（专业版）",
  "skillhub:douyindashi": "抖音大师（视频创作）",
  "skillhub:lingyi-private-domain-tag-system": "私域标签体系管理",
  "skillhub:lingyi-wx-video-decomposer-exp": "微信视频拆解（实验版）",
  "skillhub:lingyi-wx-viral-script-generator": "微信爆款脚本生成",
  "skillhub:agently-mail": "邮件撰写/发送",
  "skillhub:local-whisper": "本地语音转文字（Whisper）",
  "skillhub:ocr-local": "本地 OCR 文字识别",
  "skillhub:pdf-ocr-md": "PDF OCR 转 Markdown",
  "skillhub:pdf-image-text-extractor": "PDF/图片文字提取",
  "skillhub:wechatanalyzer": "微信聊天数据分析",
  "skillhub:memory-manager-v2": "记忆管理（版本 2）",
  "skillhub:self-improving": "自我改进智能体",
  "skillhub:self-improving-agent": "自我改进智能体",
  "skillhub:skill-vetter": "技能质量审查",
  "skillhub:find-skill-skillhub": "技能查找/搜索",
  "skillhub:algorithmic-poster-philosophy": "算法海报设计方法论",
  "skillhub:karpathy-llm-wiki": "LLM 大模型知识百科",
  "skillhub:seedance-prompt-expert": "Seedance 提示词专家",
  "skillhub:workbuddy-guide": "工作助手指南",
  "skillhub:workbuddy-gift-claimer": "礼物领取助手",
  "skillhub:unclecheng-reduce-ai-perception": "降低 AI 痕迹（去 AI 味）",
  "skillhub:perfectly-replicate-writing-skills": "写作风格复刻",
  "skillhub:666-v2": "全能助手（666 版）",
  "skillhub:qilinbashe": "麒麟八蛇（创意生成）",
  "skillhub:aistockresearcher": "AI 股票研究员",
  "skillhub:cnfinancialscraper": "中国金融数据抓取",
  "skillhub:document-summarizer": "文档总结",
};

/** 自动分词翻译：按英文单词拼接中文注释（新增技能无需手动维护即可显示中文）。 */
const ZH_WORDS: Record<string, string> = {
  agent: "智能体", browser: "浏览器", automation: "自动化", toolbox: "工具箱",
  algorithmic: "算法", poster: "海报", philosophy: "方法论",
  cn: "中文", financial: "金融", scraper: "抓取器", content: "内容", collector: "采集器",
  diagram: "图表", builder: "生成器", douyin: "抖音", copy: "文案", extract: "提取",
  montage: "混剪", mail: "邮件", baidu: "百度", search: "搜索",
  chinese: "中文", official: "公文", writing: "写作", code: "代码", simplifier: "简化",
  data: "数据", visualization: "可视化", brain: "策略", strategy: "策略",
  image: "图片", generator: "生成", script: "脚本", optimizer: "优化",
  word: "Word", docx: "Word 文档", ppt: "PPT", excel: "Excel",
  humanizer: "拟人化", summarize: "总结", analysis: "分析", analyzer: "分析器",
  pdf: "PDF", ocr: "OCR 识别", md: "转 Markdown", text: "文本", extractor: "提取器",
  local: "本地", whisper: "语音识别", memory: "记忆", manager: "管理器",
  self: "自我", improving: "改进", proactive: "主动", skill: "技能", vetter: "审查",
  find: "查找", weather: "天气", github: "GitHub", repo: "仓库",
  patent: "专利", drafting: "撰写", disclosure: "交底书", litigation: "诉讼",
  docs: "文档", template: "模板", guide: "指南",
  meeting: "会议", brief: "简报", compensation: "薪酬", officer: "专员", hr: "人力资源",
  tax: "税务", policy: "政策", knowledge: "知识库",
  workbuddy: "工作助手", gift: "礼物", claimer: "领取", reduce: "降低",
  ai: "AI", perception: "感知", replicate: "复刻", perfectly: "完美",
  slides: "幻灯片", maker: "制作", editor: "编辑", win32com: "Windows 组件",
  suite: "套件", office: "办公", wps: "WPS", sync: "同步", bitable: "多维表格",
  formula: "公式", auto: "自动化", zh: "中文", wechat: "微信", chat: "对话",
  seedance: "Seedance", prompt: "提示词", expert: "专家",
  viral: "爆款", video: "视频", decomposer: "拆解", exp: "实验版",
  private: "私域", domain: "领域", tag: "标签", system: "体系",
  dashi: "大师", pro: "专业版", optimize: "优化", ontology: "本体论",
  llm: "大模型", wiki: "百科", karpathy: "Karpathy", stock: "股票", researcher: "研究员",
  analyze: "分析", assistant: "助手",
};

function skillAutoZh(id: string): string {
  const name = id.replace(/^skillhub:/, "").replace(/^@[^/]+\//, "");
  const tokens = name.split(/[-_+\s/]+/).filter(Boolean);
  const parts: string[] = [];
  for (const tk of tokens) {
    const w = ZH_WORDS[tk.toLowerCase()];
    if (w) parts.push(w);
  }
  return parts.join("");
}

/** 技能中文注释：精确映射 → 自动分词翻译 → 自带 description 兜底。 */
function skillZhNote(id: string, desc: string): string {
  const exact = SKILL_ZH_MAP[id];
  if (exact) return exact;
  const auto = skillAutoZh(id);
  if (auto) return auto;
  if (desc && /[\u4e00-\u9fff]/.test(desc)) return desc;
  return desc ? `用途：${desc}` : "暂无说明";
}

// ── 快照面板操作（项目 6/9）──

/** 加载快照列表。 */
async function loadSnapshots(): Promise<void> {
  snapshotLoading.value = true;
  snapshotTip.value = "";
  try {
    snapshots.value = await invoke<SnapshotItem[]>("snapshot_list");
  } catch (e) {
    snapshotTip.value = `❌ 加载快照失败：${String(e)}`;
  } finally {
    snapshotLoading.value = false;
  }
}

/** 一键回滚单个快照（覆盖原文件）。 */
async function restoreSnapshot(id: string): Promise<void> {
  const ok = window.confirm("⚠️ 将用该快照覆盖当前文件，确定回滚？");
  if (!ok) return;
  snapshotTip.value = "";
  try {
    const res = await invoke<{ restoredBytes: number }>("snapshot_restore", { snapshotId: id });
    snapshotTip.value = `✅ 回滚成功（恢复 ${res.restoredBytes} 字节）`;
    await loadSnapshots();
  } catch (e) {
    snapshotTip.value = `❌ 回滚失败：${String(e)}`;
  }
}

/** 删除一条快照（文件 + 索引项）。 */
async function deleteSnapshot(id: string): Promise<void> {
  const ok = window.confirm("确定删除该快照？此操作不可恢复。");
  if (!ok) return;
  snapshotTip.value = "";
  try {
    const done = await invoke<boolean>("snapshot_delete", { snapshotId: id });
    snapshotTip.value = done ? "🗑 快照已删除" : "❌ 删除失败";
    await loadSnapshots();
  } catch (e) {
    snapshotTip.value = `❌ 删除失败：${String(e)}`;
  }
}

/** 对比快照与当前文件（回滚前审查）。 */
async function diffSnapshot(id: string): Promise<void> {
  snapshotTip.value = "";
  try {
    const res = await invoke<{ diff: string[]; diffCount: number }>("snapshot_diff", { snapshotId: id });
    const preview = (res.diff ?? []).slice(0, 30).join("\n");
    window.alert(`📋 差异共 ${res.diffCount} 处：\n\n${preview || "（无差异）"}`);
  } catch (e) {
    snapshotTip.value = `❌ 对比失败：${String(e)}`;
  }
}
// ── v6：标签选中框平移 ──
function setTab(id: (typeof TABS)[number]["id"]) {
  activeTab.value = id;
  requestAnimationFrame(() => moveIndicator());
}
function moveIndicator() {
  const el = document.querySelector<HTMLElement>(`#scTabs .tab[data-tab="${activeTab.value}"]`);
  const ind = document.getElementById("tabIndicator");
  if (el && ind) {
    ind.style.top = el.offsetTop + "px";
    ind.style.height = el.offsetHeight + "px";
  }
}

// ── MCP 服务器管理（真实后端持久化） ──
const mcpServers = ref<{ name: string; cmd: string }[]>([]);
const mcpName = ref("");
const mcpCmd = ref("");
const mcpArgs = ref("");
async function loadMcp() {
  const list = await invoke<{ name: string; command: string; args?: string[] }[]>("mcp_list_servers").catch(() => []);
  mcpServers.value = list.map((s) => ({ name: s.name, cmd: [s.command, ...(s.args ?? [])].join(" ") }));
}
async function addMcpServer() {
  const n = mcpName.value.trim();
  const c = mcpCmd.value.trim();
  if (!n || !c) return;
  const args = mcpArgs.value.trim() ? mcpArgs.value.trim().split(/\s+/) : [];
  await invoke<number>("mcp_add_server", { config: { name: n, command: c, args } }).catch(() => 0);
  mcpName.value = "";
  mcpCmd.value = "";
  mcpArgs.value = "";
  await loadMcp();
}
async function removeMcpServer(i: number) {
  const s = mcpServers.value[i];
  if (!s) return;
  await invoke<boolean>("mcp_remove_server", { name: s.name }).catch(() => false);
  await loadMcp();
}

// ── 沙箱根目录（真实后端持久化） ──
const sandboxRefs = ref<string[]>([]);
const sandboxInput = ref("");
async function loadSandbox() {
  sandboxRefs.value = await invoke<string[]>("sandbox_roots").catch(() => []);
}
async function addSandbox() {
  const p = sandboxInput.value.trim();
  if (!p) return;
  const ok = await invoke<boolean>("sandbox_add_root", { path: p }).catch(() => false);
  if (ok) {
    sandboxInput.value = "";
    await loadSandbox();
  }
}
async function removeSandbox(i: number) {
  const p = sandboxRefs.value[i];
  if (!p) return;
  await invoke<boolean>("sandbox_remove_root", { path: p }).catch(() => false);
  await loadSandbox();
}

// ── 日志查看（真实后端 debug.log / audit.log） ──
const logKind = ref("debug");
const logPreview = ref("");
const logSize = ref("");
async function readLog() {
  const lines = await invoke<string[]>("logs_tail", { kind: logKind.value, lines: 100 }).catch(() => []);
  const size = await invoke<number>("logs_size", { kind: logKind.value }).catch(() => 0);
  logPreview.value = lines.join("\n");
  logSize.value = size > 0 ? `${lines.length} 行 · ${size} B` : "";
}

// ── v6：自检 / 导出（接入真实后端） ──
const selfResult = ref("");
async function runSelfCheck() {
  selfResult.value = "正在检测模型 API 连通性…";
  try {
    const items = await invoke<any[]>("self_check_run");
    const lines = (items ?? []).map((i) => `${i.status === "ok" ? "✅" : i.status === "fail" ? "❌" : "⚠️"} ${i.name}：${i.detail ?? ""}`);
    selfResult.value = lines.length ? lines.join("\n") : "✅ 自检通过：所有项目正常";
  } catch (e) {
    selfResult.value = `❌ 自检失败：${typeof e === "string" ? e : JSON.stringify(e)}`;
  }
}
const exportTip = ref("");
async function exportAll() {
  exportTip.value = "正在导出全部数据（会话 / 设置 / 快照 / 技能）…";
  try {
    const path = await invoke<string>("export_all");
    exportTip.value = `✅ 已导出到：${path}`;
  } catch (e) {
    exportTip.value = `❌ 导出失败：${typeof e === "string" ? e : JSON.stringify(e)}`;
  }
}

// ── 自进化系统 ──
interface EvolveRankItem {
  toolId: string;
  total: number;
  successRate: number;
}
interface EvolveStatus {
  enabled?: boolean;
  totalTracked?: number;
  generatedSkills?: unknown[];
  ranking?: EvolveRankItem[];
  error?: string;
}
const evolveRunning = ref(false);
const evolveTip = ref("");
const evolveStatus = ref<EvolveStatus | null>(null);

/** 启用自进化（调用后端初始化引擎）。
 *  ★ 用真实 API Key（settings_get_keys 内存态），不再用 sk-placeholder 占位 */
async function startEvolve() {
  try {
    const s = settings.value;
    if (!s) return;
    // 优先用后端内存态 Key（settings.json 不落盘）；输入框填了则用输入框的
    let apiKey = mainKey.value.trim();
    if (!apiKey) {
      try {
        const k = await invoke<{ main?: string }>("settings_get_keys");
        apiKey = k?.main?.trim() ?? "";
      } catch { /* 静默 */ }
    }
    if (!apiKey) {
      evolveTip.value = "❌ 未找到 API Key：请先在「模型 API」页填写主模型 Key";
      return;
    }
    await invoke("self_evolve_enable", {
      apiKey,
      baseUrl: s.modelEndpoint || "https://api.deepseek.com",
      model: s.selfEvolveModel || "deepseek-chat",
      enabled: true,
    });
    evolveTip.value = "✅ 自进化引擎已启动";
  } catch (e) {
    evolveTip.value = `❌ 启动失败：${String(e)}`;
  }
}

/** 手动触发一次进化。 */
async function runEvolve() {
  evolveRunning.value = true;
  evolveTip.value = "🧬 进化中…AI 正在分析失败任务并生成改进技能";
  try {
    const result = await invoke<Record<string, unknown>>("self_evolve_run");
    evolveTip.value = `✅ ${result?.summary || "进化完成"}（生成 ${result?.generatedCount ?? 0} 个技能）`;
    await loadEvolveStatus();
  } catch (e) {
    evolveTip.value = `❌ 进化失败：${String(e)}`;
  } finally {
    evolveRunning.value = false;
  }
}

/** 加载进化状态。 */
async function loadEvolveStatus() {
  try {
    evolveStatus.value = await invoke<EvolveStatus>("self_evolve_status");
  } catch (e) {
    evolveStatus.value = { error: String(e) };
  }
}
</script>

<template>
  <div class="settings-overlay open" @click.self="emit('close')">
    <div class="settings-card">
      <header class="sc-header">
        <h3>设置</h3>
        <button class="sc-close" @click="emit('close')">✕</button>
      </header>

      <div class="sc-main">
      <nav class="tabs" id="scTabs">
        <span class="tab-indicator" id="tabIndicator"></span>
        <button
          v-for="t in TABS"
          :key="t.id"
          class="tab"
          :class="{ active: activeTab === t.id }"
          :data-tab="t.id"
          @click="setTab(t.id)"
        >
          {{ t.label }}
        </button>
      </nav>

      <div class="sc-body">
        <p v-if="error" class="sc-error">{{ error }}</p>
        <p v-if="!settings && !error" class="sc-loading">加载中…</p>

        <template v-if="settings">
          <!-- ① 模型 API 配置 -->
          <section v-show="activeTab === 'model'" class="sc-group" :class="{ active: activeTab === 'model' }">
            <h4>主模型</h4>
            <p class="sc-desc">文本推理 / 规划 / 工具选择固定走主模型</p>
            <label class="sc-label">模型</label>
            <select :value="settings.model" class="sc-select" @change="field('model', ($event.target as HTMLSelectElement).value)">
              <optgroup label="OpenCode Go（opencode.ai/zen/go）">
                <option value="deepseek-v4-flash">deepseek-v4-flash（推荐 · 快速直答）</option>
                <option value="deepseek-v4-pro">deepseek-v4-pro（更强推理 · 含思考链）</option>
                <option value="glm-5.2">glm-5.2（智谱 GLM-5.2）</option>
                <option value="kimi-k3">kimi-k3（Kimi K3）</option>
                <option value="qwen3.7-max">qwen3.7-max（通义千问）</option>
              </optgroup>
              <optgroup label="DeepSeek 官方（api.deepseek.com）">
                <option value="deepseek-chat">deepseek-chat（DeepSeek-V3 对话）</option>
                <option value="deepseek-reasoner">deepseek-reasoner（DeepSeek-R1 · 真实思考链）</option>
              </optgroup>
            </select>
            <p class="sc-desc">💡 想看到模型真实的思考过程？开启右上角 Agent（StepConfirm / Yolo 模式），对话区会流式展示完整思考链（OpenCode Go 走 deepseek-v4-pro，DeepSeek 官方走 deepseek-reasoner）。</p>
            <label class="sc-label">API 地址</label>
            <input :value="settings.modelEndpoint" class="sc-input" @change="field('modelEndpoint', ($event.target as HTMLInputElement).value)" />

            <h4>视觉模型（识图路由）</h4>
            <p class="sc-desc">analyze_image 自动路由至视觉专用模型</p>
            <label class="sc-label">模型</label>
            <input :value="settings.visionModel" class="sc-input" @change="field('visionModel', ($event.target as HTMLInputElement).value)" />
            <label class="sc-label">API 地址</label>
            <input :value="settings.visionEndpoint" class="sc-input" @change="field('visionEndpoint', ($event.target as HTMLInputElement).value)" />

            <h4>绘图 API（生图路由）</h4>
            <p class="sc-desc">generate_image 自动路由至 Flux / SD 系列</p>
            <label class="sc-label">模型</label>
            <input :value="settings.imageModel" class="sc-input" @change="field('imageModel', ($event.target as HTMLInputElement).value)" />
            <label class="sc-label">API 地址</label>
            <input :value="settings.imageEndpoint" class="sc-input" @change="field('imageEndpoint', ($event.target as HTMLInputElement).value)" />

            <h4>API Key（输入后点击「保存 Key」）</h4>
            <input v-model="mainKey" type="password" class="sc-input" placeholder="DeepSeek 主模型 Key" />
            <div class="sc-balance-row">
              <button class="sc-btn" :disabled="mainBalanceLoading" @click="checkBalance('main')">
                {{ mainBalanceLoading ? "查询中…" : "查询余额" }}
              </button>
              <span v-if="mainBalanceText" class="sc-balance" :class="{ err: mainBalanceText.startsWith('查询失败') }">{{ mainBalanceText }}</span>
            </div>
            <input v-model="visionKey" type="password" class="sc-input" placeholder="视觉模型 Key" />
            <div class="sc-balance-row">
              <button class="sc-btn" :disabled="visionBalanceLoading" @click="checkBalance('vision')">
                {{ visionBalanceLoading ? "查询中…" : "查询余额" }}
              </button>
              <span v-if="visionBalanceText" class="sc-balance" :class="{ err: visionBalanceText.startsWith('查询失败') }">{{ visionBalanceText }}</span>
            </div>
            <input v-model="imageKey" type="password" class="sc-input" placeholder="绘图 API Key" />
            <div class="sc-balance-row">
              <button class="sc-btn" :disabled="imageBalanceLoading" @click="checkBalance('image')">
                {{ imageBalanceLoading ? "查询中…" : "查询余额" }}
              </button>
              <span v-if="imageBalanceText" class="sc-balance" :class="{ err: imageBalanceText.startsWith('查询失败') }">{{ imageBalanceText }}</span>
            </div>
            <div class="sc-balance-row" style="margin-top:4px">
              <button class="sc-btn" style="background:#4caf50;border-color:#4caf50" @click="saveKeys">💾 保存 Key</button>
              <span v-if="keysSavedTip" style="color:#4caf50;font-size:12px">{{ keysSavedTip }}</span>
            </div>

            <h4>🔄 opencode 网关自动回切</h4>
            <p class="sc-desc">持续检测 opencode 网关是否恢复（如遇宕机返回 500），恢复后自动把主/视觉模型端点与 Key 切回 opencode，并自动关闭本开关。检测间隔 {{ settings.opencodeWatchIntervalSecs || 120 }} 秒。</p>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.opencodeWatchEnabled" @change="field('opencodeWatchEnabled', ($event.target as HTMLInputElement).checked)" />
              启用持续检测（当前 opencode 网关未恢复时建议开启）
            </label>
            <label class="sc-label">opencode 端点</label>
            <input :value="settings.opencodeWatchEndpoint" class="sc-input" @change="field('opencodeWatchEndpoint', ($event.target as HTMLInputElement).value)" />
            <label class="sc-label">opencode API Key（恢复后自动填入主/视觉 Key）</label>
            <input :value="settings.opencodeWatchApiKey" type="password" class="sc-input" placeholder="sk-KnQr..." @change="field('opencodeWatchApiKey', ($event.target as HTMLInputElement).value)" />
          </section>

          <!-- ② Agent 智能体参数 -->
          <section v-show="activeTab === 'agent'" class="sc-group" :class="{ active: activeTab === 'agent' }">
            <h4>全局 Agent 开关</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.agentEnabled" @change="field('agentEnabled', ($event.target as HTMLInputElement).checked)" />
              启用 Agent（默认关闭）
            </label>

            <h4>默认权限模式</h4>
            <select :value="settings.agentMode" class="sc-select" @change="field('agentMode', ($event.target as HTMLSelectElement).value)">
              <option value="off">关闭（直通模型）</option>
              <option value="plan_only">计划只读</option>
              <option value="step_confirm">逐步确认</option>
              <option value="yolo">YOLO 全自动</option>
            </select>

            <h4>最大迭代轮数</h4>
            <p class="sc-desc">ReAct 循环硬上限（1–50），防止无限循环消耗 token</p>
            <input :value="settings.maxRounds" type="number" min="1" max="50" class="sc-input" @change="field('maxRounds', Number(($event.target as HTMLInputElement).value))" />

            <h4>上下文压缩阈值</h4>
            <p class="sc-desc">单轮上下文 token 超阈值自动摘要压缩</p>
            <input :value="settings.compactionThreshold" type="number" min="1000" step="500" class="sc-input" @change="field('compactionThreshold', Number(($event.target as HTMLInputElement).value))" />

            <h4>工具循环上限</h4>
            <p class="sc-desc">AI 单轮可调工具次数（最低 1 / 最高 30，推荐 5~15）。过低时识图/读 PDF 多步操作会被熔断打断，过高有死循环风险</p>
            <input :value="settings.maxToolRounds ?? 5" type="number" min="1" max="30" class="sc-input" @change="field('maxToolRounds', Number(($event.target as HTMLInputElement).value))" />
          </section>

          <!-- ③ MCP 工具管理 -->
          <section v-show="activeTab === 'mcp'" class="sc-group" :class="{ active: activeTab === 'mcp' }">
            <h4>第三方 MCP 插件</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.mcpEnabled" @change="field('mcpEnabled', ($event.target as HTMLInputElement).checked)" />
              启用 MCP 工具
            </label>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.visionMcpEnabled" @change="field('visionMcpEnabled', ($event.target as HTMLInputElement).checked)" />
              启用视觉 MCP（替换内置识图）
            </label>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.imageMcpEnabled" @change="field('imageMcpEnabled', ($event.target as HTMLInputElement).checked)" />
              启用绘图 MCP（替换内置生图）
            </label>
            <h4>MCP 服务器</h4>
            <p class="sc-desc">添加第三方 MCP 服务器，工具列表自动更新</p>
            <div class="mcp-list">
              <div v-for="(s, i) in mcpServers" :key="i" class="mcp-item">
                <span class="mcp-ico"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="2" width="20" height="8" rx="2"/><rect x="2" y="14" width="20" height="8" rx="2"/><line x1="6" y1="6" x2="6.01" y2="6"/><line x1="6" y1="18" x2="6.01" y2="18"/></svg></span>
                <span class="mcp-info"><span class="mcp-name">{{ s.name }}</span><span class="mcp-cmd">{{ s.cmd }}</span></span>
                <button class="mcp-del" @click="removeMcpServer(i)">✕</button>
              </div>
            </div>
            <div class="mcp-add">
              <input v-model="mcpName" class="sc-input" placeholder="名称（如 fs）">
              <input v-model="mcpCmd" class="sc-input" placeholder="命令（如 npx / node）">
              <input v-model="mcpArgs" class="sc-input" placeholder="参数（空格分隔）">
              <button class="btn-primary" @click="addMcpServer">添加服务器</button>
            </div>
          </section>

          <!-- ④ 外观汉化配置 -->
          <section v-show="activeTab === 'appearance'" class="sc-group" :class="{ active: activeTab === 'appearance' }">
            <h4>简体中文</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.chineseOnly" @change="field('chineseOnly', ($event.target as HTMLInputElement).checked)" />
              全局简体中文（出厂默认中文）
            </label>

            <h4>字体大小</h4>
            <input :value="settings.fontSize" type="number" min="12" max="22" class="sc-input" @change="field('fontSize', Number(($event.target as HTMLInputElement).value))" />

            <h4>时区</h4>
            <select class="sc-select" :value="tz" @change="emit('tz', ($event.target as HTMLSelectElement).value)">
              <option value="Asia/Shanghai">Asia/Shanghai（UTC+8 中国）</option>
              <option value="Asia/Tokyo">Asia/Tokyo（UTC+9 日本）</option>
              <option value="Asia/Singapore">Asia/Singapore（UTC+8）</option>
              <option value="Europe/London">Europe/London（UTC+0 伦敦）</option>
              <option value="America/New_York">America/New_York（UTC−5 美东）</option>
              <option value="UTC">UTC（协调世界时）</option>
            </select>

            <h4>外观主题</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.darkTheme" @change="(e) => { const v = (e.target as HTMLInputElement).checked; field('darkTheme', v); applyTheme(v); }" />
              深色模式（默认浅色玻璃壁纸风格）
            </label>

            <h4>界面不透明度</h4>
            <label class="sc-row">
              <input
                type="range"
                min="20"
                max="100"
                step="5"
                :value="Math.round((settings.uiOpacity ?? 1) * 100)"
                @input="onUiOpacityInput"
                @change="onUiOpacityChange"
              />
              <span class="sc-range-val">{{ Math.round((settings.uiOpacity ?? 1) * 100) }}%</span>
            </label>
            <p class="sc-desc">调低后气泡/顶栏/输入区等玻璃框变透明，露出壁纸背景（20%~100%）</p>

            <hr class="sc-divider" />

            <!-- ⑬ 朗读 / TTS 设置（Edge TTS 神经网络拟人音色） -->
            <h4>🔊 朗读设置</h4>
            <p class="sc-desc">神经网络拟人音色（微软 Edge TTS，免费无需 Key），支持语气风格，比系统语音自然得多</p>

            <label class="sc-check">
              <input type="checkbox" :checked="settings.ttsEnabled" @change="field('ttsEnabled', ($event.target as HTMLInputElement).checked)" />
              启用 AI 朗读（输出完自动朗读，默认开启）
            </label>

            <template v-if="settings.ttsEnabled">
              <label class="sc-label">朗读音色</label>
              <select :value="settings.ttsVoice || 'zh-CN-XiaoxiaoNeural'" class="sc-select" @change="field('ttsVoice', ($event.target as HTMLSelectElement).value)">
                <option v-for="v in voices" :key="v.id" :value="v.id">
                  {{ v.name }}（{{ v.gender }} · {{ v.region }}）— {{ v.desc }}
                </option>
              </select>

              <label class="sc-label">语气风格</label>
              <select :value="settings.ttsStyle || '__natural__'" class="sc-select" @change="field('ttsStyle', ($event.target as HTMLSelectElement).value === '__natural__' ? '' : ($event.target as HTMLSelectElement).value)">
                <option value="__natural__">🌿 自然（无语气）</option>
                <option v-for="st in currentVoiceStyles" :key="st" :value="st">{{ styleLabel(st) }}</option>
                <option v-if="!currentVoiceStyles.length" value="" disabled>该音色不支持语气风格</option>
              </select>

              <label class="sc-label">语速：{{ (settings.ttsRate ?? 1).toFixed(2) }}×</label>
              <div class="sc-row">
                <input
                  type="range"
                  min="50"
                  max="200"
                  step="5"
                  :value="Math.round((settings.ttsRate ?? 1) * 100)"
                  @change="field('ttsRate', Number(($event.target as HTMLInputElement).value) / 100)"
                />
                <span class="sc-range-val">{{ Math.round((settings.ttsRate ?? 1) * 100) }}%</span>
              </div>
              <p class="sc-desc">50% = 慢速清晰，100% = 正常，200% = 快速</p>

              <div class="ops-row">
                <button class="btn-primary" :disabled="previewing" @click="previewVoice">
                  {{ previewing ? '试听中…' : '🎧 试听当前音色' }}
                </button>
                <button class="btn-primary" @click="stopPreview">⏹ 停止</button>
              </div>
              <p v-if="previewTip" class="sc-tip">{{ previewTip }}</p>
            </template>
          </section>

          <!-- ⑤ 快照与安全配置 -->
          <section v-show="activeTab === 'security'" class="sc-group" :class="{ active: activeTab === 'security' }">
            <h4>文件快照备份</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.snapshotEnabled" @change="field('snapshotEnabled', ($event.target as HTMLInputElement).checked)" />
              文件修改前自动备份快照
            </label>

            <h4>高危操作确认</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.highRiskConfirm" @change="field('highRiskConfirm', ($event.target as HTMLInputElement).checked)" />
              高危操作二次确认
            </label>

            <h4>敏感文件保护</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.sensitiveFilesEnabled" @change="(e) => invoke('set_sensitive_guard', { enabled: (e.target as HTMLInputElement).checked }).catch(() => {})" />
              拦截对 .env / 私钥 / 凭据等敏感文件的访问
            </label>
            <p class="sc-desc">关闭后工具可读取/写入 .env、*.pem、*.key、credentials、token 等文件（有泄露风险）</p>

            <h4>Windows 集成</h4>
            <label class="sc-check">
              <input type="checkbox" :checked="settings.autoStart" @change="(e) => field('autoStart', (e.target as HTMLInputElement).checked)" />
              开机自启动 ClawDesk
            </label>

            <h4>本地日志路径</h4>
            <input :value="settings.logPath" class="sc-input" placeholder="留空使用默认路径" @change="field('logPath', ($event.target as HTMLInputElement).value)" />

            <h4>📖 《人是怎么样的》书目录</h4>
            <p class="sc-desc">AI 的"灵魂档案"路径（含 条目/ 子目录）。书搬家后在这里改路径，AI 的拟人参考、情绪、记忆会跟随新位置。默认：<code>D:\人是怎么样的</code></p>
            <input :value="settings.humanBookDir" class="sc-input" placeholder="D:\人是怎么样的" @change="field('humanBookDir', ($event.target as HTMLInputElement).value)" />
            <hr class="sc-divider" />
            <h4>快照回滚面板</h4>
            <p class="sc-desc">查看全部文件修改快照，可一键回滚 / 删除 / 对比差异</p>
            <button class="btn-primary" :disabled="snapshotLoading" @click="loadSnapshots">
              {{ snapshotLoading ? "加载中…" : "🔄 刷新快照" }}
            </button>
            <p v-if="snapshotTip" class="sc-tip">{{ snapshotTip }}</p>

            <div v-if="snapshots.length" class="snap-list">
              <div v-for="s in snapshots" :key="s.id" class="snap-item">
                <div class="snap-info">
                  <div class="snap-file" :title="s.original">{{ s.original }}</div>
                  <div class="snap-meta">{{ s.createdAt }} · {{ s.size }} B</div>
                </div>
                <div class="snap-actions">
                  <button class="snap-btn" title="对比差异" @click="diffSnapshot(s.id)">对比</button>
                  <button class="snap-btn" title="回滚此文件" @click="restoreSnapshot(s.id)">回滚</button>
                  <button class="snap-btn danger" title="删除快照" @click="deleteSnapshot(s.id)">删除</button>
                </div>
              </div>
            </div>
            <p v-else-if="!snapshotLoading" class="sc-desc">暂无快照（文件修改后自动生成）</p>
          </section>

          <!-- ⑥ 技能管理 -->
          <section v-show="activeTab === 'skills'" class="sc-group" :class="{ active: activeTab === 'skills' }">
            <h4>已安装技能</h4>
            <p class="sc-desc">
              安装新技能后点「🔄 重新扫描」即时加载（扫描
              <code>%APPDATA%/com.clawdesk.app/skills</code>）。关闭开关即禁用该技能（不进入 LLM 工具列表）。
            </p>
            <div class="skill-toolbar">
              <button class="btn-primary" :disabled="skillsLoading" @click="reloadSkills">
                {{ skillsLoading ? "扫描中…" : "🔄 重新扫描" }}
              </button>
              <span class="skill-count" v-if="!skillsLoading">共 {{ skills.length }} 个技能</span>
            </div>
            <p v-if="skillsTip" class="sc-tip">{{ skillsTip }}</p>
            <div v-if="skillsLoading" class="sc-loading">加载中…</div>
            <div v-else-if="skills.length" class="skill-list">
              <label v-for="s in skills" :key="s.id" class="skill-item">
                <input
                  type="checkbox"
                  :checked="s.enabled"
                  @change="toggleSkill(s.id, ($event.target as HTMLInputElement).checked)"
                />
                <span class="skill-text">
                  <span class="skill-id" :title="s.description">{{ s.id }}</span>
                  <span class="skill-zh">{{ skillZhNote(s.id, s.description) }}</span>
                </span>
              </label>
            </div>
            <p v-else class="sc-desc">暂无技能（可在 SkillHub 安装后点「重新扫描」）</p>
          </section>

          <!-- ⑧ 自进化系统 -->
          <section v-show="activeTab === 'selfEvolve'" class="sc-group" :class="{ active: activeTab === 'selfEvolve' }">
            <h4>🧬 自进化引擎</h4>
            <p class="sc-desc">AI 自动分析失败任务，生成改进技能并注册到工具库，实现闭环自我优化</p>

            <label class="sc-check">
              <input type="checkbox" :checked="settings.selfEvolveEnabled" @change="(e) => { field('selfEvolveEnabled', (e.target as HTMLInputElement).checked); if ((e.target as HTMLInputElement).checked) startEvolve(); }" />
              启用自进化（AI 自动学习并生成新技能）
            </label>

            <label class="sc-check">
              <input type="checkbox" :checked="settings.selfEvolveAuto" @change="field('selfEvolveAuto', ($event.target as HTMLInputElement).checked)" :disabled="!settings.selfEvolveEnabled" />
              自动进化（每次启动 / 每天自动运行一次进化循环）
            </label>

            <h4>进化参数</h4>
            <label class="sc-label">进化模型（生成技能用的 LLM）</label>
            <select :value="settings.selfEvolveModel" class="sc-select" @change="field('selfEvolveModel', ($event.target as HTMLSelectElement).value)" :disabled="!settings.selfEvolveEnabled">
              <option value="deepseek-chat">deepseek-chat（DeepSeek-Chat，默认）</option>
              <option value="deepseek-v4-flash">deepseek-v4-flash（DeepSeek-V4-Flash）</option>
              <option value="deepseek-v4-pro">deepseek-v4-pro（DeepSeek-V4-Pro）</option>
            </select>

            <label class="sc-label">触发阈值（成功率低于此值触发进化）</label>
            <div class="sc-row">
              <input type="range" :value="Math.round((settings.selfEvolveThreshold ?? 0.6) * 100)" min="20" max="95" step="5" @change="field('selfEvolveThreshold', Number(($event.target as HTMLInputElement).value) / 100)" :disabled="!settings.selfEvolveEnabled" />
              <span class="sc-range-val">{{ Math.round((settings.selfEvolveThreshold ?? 0.6) * 100) }}%</span>
            </div>
            <p class="sc-desc">工具成功率低于此值 + 至少执行 5 次 → 自动生成改进技能</p>

            <hr class="sc-divider" />

            <h4>手动触发</h4>
            <div class="ops-row">
              <button class="btn-primary" :disabled="evolveRunning || !settings.selfEvolveEnabled" @click="runEvolve">
                {{ evolveRunning ? '进化中…' : '立即进化' }}
              </button>
              <button class="btn-primary" :disabled="!settings.selfEvolveEnabled" @click="loadEvolveStatus">
                刷新状态
              </button>
            </div>
            <p v-if="evolveTip" class="sc-tip">{{ evolveTip }}</p>

            <h4>进化状态</h4>
            <div v-if="evolveStatus" class="evolve-status">
              <div class="es-row"><span class="es-label">状态：</span>{{ evolveStatus.enabled ? '✅ 已启用' : '⏸ 未启用' }}</div>
              <div class="es-row"><span class="es-label">已追踪工具数：</span>{{ evolveStatus.totalTracked ?? 0 }}</div>
              <div class="es-row"><span class="es-label">已生成技能数：</span>{{ (evolveStatus.generatedSkills || []).length }}</div>
              <div v-if="evolveStatus.ranking?.length" class="es-ranking">
                <span class="es-label">工具排名（前 10）：</span>
                <div v-for="(r, i) in evolveStatus.ranking.slice(0, 10)" :key="i" class="es-item">
                  {{ i + 1 }}. {{ r.toolId }} — {{ r.total }}次 · 成功率 {{ r.successRate }}
                </div>
              </div>
            </div>
          </section>

          <!-- ⑦ 系统运维 -->
          <section v-show="activeTab === 'system'" class="sc-group" :class="{ active: activeTab === 'system' }">
            <h4>沙箱根目录（权限白名单）</h4>
            <p class="sc-desc">Agent 仅可访问以下目录，防止越权读取系统文件</p>
            <div class="sandbox-list">
              <div v-for="(p, i) in sandboxRefs" :key="i" class="sandbox-item">
                <span class="sx-ico"><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg></span>
                <span class="sx-path">{{ p }}</span>
                <button class="sx-del" @click="removeSandbox(i)">✕</button>
              </div>
            </div>
            <div class="sandbox-add">
              <input v-model="sandboxInput" class="sc-input" placeholder="添加允许访问的目录路径">
              <button class="btn-primary" @click="addSandbox">添加</button>
            </div>
            <hr class="sc-divider" />
            <h4>日志查看</h4>
            <div class="log-view">
              <div class="log-toolbar">
                <select v-model="logKind" class="sc-select">
                  <option value="debug">调试日志（debug.log）</option>
                  <option value="audit">审计日志（audit.log）</option>
                </select>
                <button class="btn-primary" @click="readLog">读取日志</button>
                <span class="skill-count">{{ logSize }}</span>
              </div>
              <textarea class="log-preview" :value="logPreview" readonly placeholder="点击「读取日志」查看最近日志…"></textarea>
            </div>
            <hr class="sc-divider" />
            <h4>自检</h4>
            <p class="sc-desc">校验模型 API 连通性 / 路由层 / 技能注册状态</p>
            <div class="ops-row">
              <button class="btn-primary" @click="runSelfCheck">运行自检</button>
              <button class="btn-primary" @click="exportAll">导出全部数据</button>
            </div>
            <div v-if="selfResult" class="selfcheck-result">{{ selfResult }}</div>
            <p v-if="exportTip" class="sc-tip">{{ exportTip }}</p>
          </section>
        </template>
      </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.sc-error { color: #b3261e; font-size: 12px; margin-bottom: 6px; }
.sc-loading { color: #7a7a8a; font-size: 13px; padding: 10px 0; }
.sc-balance-row { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.sc-btn {
  padding: 7px 16px;
  border-radius: 8px;
  border: 1px solid #3b5bbf;
  background: #2f4fc4;
  color: #fff;
  font-size: 13px;
  cursor: pointer;
  transition: 0.15s;
}
.sc-btn:hover { background: #3a5cd0; }
.sc-btn:disabled { opacity: 0.55; cursor: not-allowed; }
.sc-balance {
  font-size: 12.5px;
  color: #2f9e44;
  background: rgba(47, 158, 68, 0.1);
  border: 1px solid rgba(47, 158, 68, 0.3);
  padding: 5px 10px;
  border-radius: 8px;
}
.sc-balance.err {
  color: #b3261e;
  background: rgba(179, 38, 30, 0.08);
  border-color: rgba(179, 38, 30, 0.3);
}
</style>
