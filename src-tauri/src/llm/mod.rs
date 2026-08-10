//! LLM 模型调度层 —— DeepSeek（OpenAI 兼容）客户端 + 工具循环调度。
//!
//! 设计说明：
//! - **不改 core**：复用 ToolDispatcher / ToolRegistry / UnifiedToolDef；
//! - 模型自动调度 = 循环：LLM 决定调用哪些工具 → 经 dispatcher 执行 →
//!   结果作为 `tool` 消息回传 → 重复直到 LLM 不再调用工具或轮次熔断（5 轮）；
//! - API Key **不落盘、不打印**：仅存内存（命令参数 / 环境变量），
//!   日志与错误信息一律不包含 key；
//! - 工具名传给 LLM 前编码：DeepSeek/OpenAI 函数名仅允许 `[a-zA-Z0-9_-]`，
//!   `source:name` 的冒号编码为 `__`（encode/decode 见下），执行时还原。

pub mod client;
pub mod error_guard;
pub mod export;
pub mod logging;
pub mod planner;
pub mod progress;
pub mod router;
pub mod runner;
pub mod self_check;
pub mod settings;
pub mod session;
pub mod tool_log;
pub mod tool_selector;
pub mod win_integration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Agent 权限模式（出厂默认 `Off`，YOLO 需用户手动开启）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Agent 关闭：直通 LLM，不执行工具调用（出厂默认）。
    Off,
    /// 只读规划：模型仅输出执行计划，不调用任何工具。
    PlanOnly,
    /// 逐步确认：每一步工具执行前暂停，等待用户确认。
    StepConfirm,
    /// YOLO 全自动：模型自主执行全部工具调用（需用户手动开启）。
    Yolo,
    /// 多 Agent 协作 —— v1 预留接口，暂不实现（硬性约束）。
    MultiAgent,
}

impl Default for AgentMode {
    fn default() -> Self {
        Self::Off
    }
}

impl AgentMode {
    /// 从字符串解析（前端下拉框值），未知值回退 Off。
    pub fn from_str(s: &str) -> Self {
        match s {
            "plan_only" => Self::PlanOnly,
            "step_confirm" => Self::StepConfirm,
            "yolo" => Self::Yolo,
            "multi_agent" => Self::MultiAgent,
            _ => Self::Off,
        }
    }

    #[allow(dead_code)] // 公共 API 预留（前端经 serde 获取字符串形式）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::PlanOnly => "plan_only",
            Self::StepConfirm => "step_confirm",
            Self::Yolo => "yolo",
            Self::MultiAgent => "multi_agent",
        }
    }
}

/// 按 char 边界截断字符串（UTF-8 中文安全，避免字节切片 panic）。
#[allow(dead_code)] // 公共工具函数（runner/client 目前各自有私有实现）
pub fn truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let head: String = chars[..max_chars].iter().collect();
        format!("{}…(+{}chars)", head, chars.len() - max_chars)
    }
}

/// LLM 消息角色（OpenAI 兼容）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 单条对话消息。
///
/// 注意：OpenAI 兼容 API 要求 snake_case 的 `tool_calls` / `tool_call_id`，
/// 不能用 rename_all=camelCase（会序列化成 toolCalls / toolCallId 导致 400）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: Role,
    pub content: String,
    /// assistant 消息携带的工具调用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmToolCall>>,
    /// tool 消息对应的调用 ID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// 工具调用的 function 部分（OpenAI 兼容嵌套结构）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolFunction {
    /// 工具名（发给 LLM 的编码名，如 `builtin__get_time`）。
    pub name: String,
    /// 参数 JSON 字符串。
    pub arguments: String,
}

/// 单个工具调用（OpenAI 兼容：`{id, type, function}`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmToolCall {
    pub id: String,
    /// 固定为 `function`。
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: LlmToolFunction,
}

/// 完整对话响应（解析自 /chat/completions）。
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    pub choices: Vec<ChatChoice>,
    /// Token 用量（OpenAI 兼容 `usage` 字段；部分端点/测试响应缺失时为 None）。
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Token 用量统计（OpenAI 兼容）。
///
/// 契约：DeepSeek / 视觉模型 / 绘图 API 均可能返回该字段；缺失时各字段为 0，
/// 由上层（runner 累计器）统一兜底，不影响解析。
#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "snake_case", serialize = "camelCase"))]
pub struct Usage {
    /// OpenAI 兼容 API 返回 snake_case（prompt_tokens），
    /// 前端契约用 camelCase（promptTokens）——双向重命名对齐。
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl Usage {
    /// 将两段用量相加（用于跨轮累计）。
    pub fn add(&mut self, other: &Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }
}

/// 从响应中提取 Token 用量（缺失时返回全零）。
pub fn extract_usage(resp: &ChatResponse) -> Usage {
    resp.usage.unwrap_or_default()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatChoice {
    pub message: ResponseMessage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseMessage {
    #[serde(default)]
    pub content: Option<String>,
    /// 注意：OpenAI 兼容 API 返回 snake_case 的 `tool_calls`，
    /// 不能走 rename_all=camelCase（会变成 toolCalls 导致解析失败）。
    #[serde(default, rename = "tool_calls")]
    pub tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseToolCall {
    pub id: String,
    /// 解析完整性保留（当前未消费）。
    #[allow(dead_code)]
    #[serde(rename = "type", default)]
    pub call_type: String,
    pub function: ResponseFunction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// 将 UnifiedToolDef 列表序列化为 OpenAI function calling 的 `tools` 数组。
///
/// 参数 schema：`{type:"object", properties:{...}, required:[...]}`；
/// 工具名使用编码名（`source:name` → `source__name`）。
pub fn serialize_tools(defs: &[crate::core::tool::def::UnifiedToolDef]) -> Value {
    let tools: Vec<Value> = defs
        .iter()
        .map(|def| {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for p in &def.params {
                let mut prop = serde_json::Map::new();
                prop.insert("type".into(), json!(p.param_type));
                prop.insert("description".into(), json!(p.description));
                if let Some(ev) = &p.enum_values {
                    prop.insert("enum".into(), json!(ev));
                }
                if p.required {
                    required.push(p.name.clone());
                }
                properties.insert(p.name.clone(), Value::Object(prop));
            }
            json!({
                "type": "function",
                "function": {
                    "name": encode_tool_name(&def.id),
                    "description": def.description,
                    "parameters": {
                        "type": "object",
                        "properties": properties,
                        "required": required,
                    }
                }
            })
        })
        .collect();
    Value::Array(tools)
}

/// 将 `source:name` 编码为 LLM 合法函数名。
///
/// DeepSeek API 要求 function.name 匹配 `^[a-zA-Z0-9_-]+$`，因此：
/// - `:` → `__`（保持既有约定，兼容前端 split("__") 逻辑）
/// - 其他非法字符（空格 / `+` / `.` 等）→ `_XX`（十六进制字节码，可逆解码）
pub fn encode_tool_name(id: &str) -> String {
    let mut out = String::with_capacity(id.len() * 2);
    for b in id.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else if c == ':' {
            out.push_str("__");
        } else {
            out.push('_');
            out.push_str(&format!("{:02x}", b));
        }
    }
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// 将 LLM 返回的函数名解码回 `source:name`。
/// - `__` → `:`
/// - `_XX`（十六进制字节码）→ 原字符（仅当还原出的字符原本非法时，避免误伤合法 `_XX` 字面）
pub fn decode_tool_name(encoded: &str) -> String {
    let bytes = encoded.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    // 只把第一个 `__` 还原为 source:name 的分隔冒号；name 内部原有的 `__` 保留
    let mut sep_done = false;
    while i < bytes.len() {
        if bytes[i] == b'_' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'_' {
                if !sep_done {
                    out.push(':');
                    sep_done = true;
                } else {
                    out.push_str("__"); // 工具名内部的 `__` 原样保留
                }
                i += 2;
                continue;
            }
            if i + 2 < bytes.len() {
                if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    let b = (h << 4) | l;
                    let orig = b as char;
                    // 仅还原原本非法的字符（还原后仍是字母数字_ - 则保持字面 _XX）
                    if !(orig.is_ascii_alphanumeric() || orig == '_' || orig == '-')
                        && b != b':'
                    {
                        out.push(orig);
                        i += 3;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// 从响应中提取模型文本回复（多个 choice 取第一个）。
pub fn extract_text(resp: &ChatResponse) -> String {
    resp.choices
        .first()
        .and_then(|c| c.message.content.clone())
        .unwrap_or_default()
}

/// 从响应中提取工具调用列表。
pub fn extract_tool_calls(resp: &ChatResponse) -> Vec<LlmToolCall> {
    resp.choices
        .first()
        .and_then(|c| c.message.tool_calls.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|tc| LlmToolCall {
            id: tc.id,
            // OpenAI 兼容格式要求 type: "function"
            call_type: if tc.call_type.is_empty() {
                "function".into()
            } else {
                tc.call_type
            },
            function: LlmToolFunction {
                name: tc.function.name,
                arguments: tc.function.arguments,
            },
        })
        .collect()
}

/// 构造 system 提示（说明可用工具与规则）。
/// 任务复杂度分类（供系统提示注入智能分层指令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    /// 纯问答/即时问题：直接回答，不调工具、不委派。
    Simple,
    /// 复杂只读任务（多文件调研/深度分析/统计盘点）：优先委派 agent_subtask 子 Agent。
    Complex,
    /// 需要写/执行的操作或无法明确判断：主 Agent 自行处理（子 Agent 只读，不委派）。
    Neutral,
}

/// 启发式任务复杂度分类。
///
/// 关键约束：`agent_subtask` 子 Agent 只暴露**只读**工具（无 file_write/terminal），
/// 因此需要写/执行的操作绝不能委派，必须归 Neutral 由主 Agent 亲自完成。
pub fn classify_task(prompt: &str) -> TaskComplexity {
    // 需要写/执行的操作：主 Agent 亲自干（子 Agent 只读，无法完成）
    const WRITE_ACTION_KEYWORDS: &[&str] = &[
        "写入", "创建", "编辑", "修改", "删除", "生成", "运行", "执行", "下载", "安装",
        "启动", "配置", "打开", "新建", "保存", "打包", "压缩", "解压", "移动", "复制",
        "重命名", "写一个", "写个", "做个", "做成", "start", "run", "create", "write",
        "delete", "install", "build", "make", "save",
    ];
    // 复杂只读任务：适合委派子 Agent
    const COMPLEX_READ_KEYWORDS: &[&str] = &[
        "调研", "分析", "研究", "审查", "总结", "对比", "统计", "检查", "排查", "盘点",
        "审计", "评估", "梳理", "归纳", "深入", "扫描", "遍历", "全部", "目录", "每个",
        "逐一", "代码审查", "review", "analyze", "summarize", "investigate", "research",
        "audit", "survey", "report",
    ];
    // 纯问答提示词
    const QA_HINTS: &[&str] = &[
        "什么是", "是什么", "为什么", "怎么理解", "解释", "介绍一下", "什么意思", "区别",
        "哪个好", "推荐", "如何理解", "what is", "what's", "why", "how to", "explain",
    ];

    let p = prompt.to_lowercase();
    let chars = prompt.chars().count();
    // 需要写/执行的操作 → 主 Agent 亲自完成（子 Agent 只读）
    if WRITE_ACTION_KEYWORDS.iter().any(|k| p.contains(k)) {
        return TaskComplexity::Neutral;
    }
    // 复杂只读任务 → 委派子 Agent
    if chars > 200 || COMPLEX_READ_KEYWORDS.iter().any(|k| p.contains(k)) {
        return TaskComplexity::Complex;
    }
    // 短问答 → 直接回答
    if chars <= 60 || QA_HINTS.iter().any(|k| p.contains(k)) {
        return TaskComplexity::Simple;
    }
    TaskComplexity::Neutral
}

pub fn build_system_prompt() -> String {
    r#"你是 ClawDesk 桌面智能助手，运行在用户本地 Windows 电脑上，由 DeepSeek-V4 驱动。你拥有完整的本地环境感知能力，可自主操作电脑完成任务。

## 身份与能力边界
你是用户电脑上的全能智能体，不是单纯聊天机器人。你拥有：
- 本地文件系统读写（读取/创建/编辑代码与文档）
- 终端命令执行（运行脚本/安装依赖/系统操作）
- OCR 文字识别（阅读图片中的文字）
- 图像生成（程序化占位图，后续接入生图引擎）
- 窗口管理（最小化/最大化/关闭窗口）
- 外置 MCP 插件扩展能力
- 技能模板（SkillHub 内置总结/翻译等技能）

## 工具调用规则
1. 需要工具时返回标准 OpenAI tool_calls 格式，不要编造结果。
2. 工具结果会作为 tool 消息回传，你必须基于真实结果继续推理。
3. 工具失败时，反思错误原因并尝试修正参数重试（最多 3 次），若持续失败则诚实告知用户。
4. 可以一次返回多个 tool_calls 并行调用无依赖的工具。
5. 完成所有任务后，输出简洁清晰的最终总结。

## 附件文件处理策略
用户可能在消息中附带 `[用户附件文件]` 段落，其中每行 `- <绝对路径>` 指向一个已保存到本地磁盘的文件。处理规则：
1. 必须先实际读取附件内容再回答，禁止仅凭文件名猜测内容。
2. 文本类文件（txt/md/json/代码/日志/csv 等）：调用 `file_read` 读取。
3. 超过 1MB 的大文件：`file_read` 会拒绝，改用 `terminal` 工具分段读取（如 PowerShell `Get-Content -Head` 或 `Get-Content -Tail`）。
4. 图片附件：调用 `analyze_image`（含视觉路由）或 `ocr` 提取文字。
5. **压缩包（zip）**：`file_read` 可直接读取 zip（自动列出文件清单 + 读取第一个文本文件），无需手动解压。超过 2MB 的 zip 不支持，如实告知用户。rar/7z 用 `terminal` 工具调用系统解压（如 `tar -xf` 或 `7z x`）后再读取。
6. 路径不确定或需要探查附件目录时：调用 `list_dir` 查看目录内容。`list_dir` 支持 `format` 参数：`table`（默认，对齐表格，含类型/大小/修改时间）、`json`（原始数据）、`plain`（每行一个名字）；列目录默认用 `table` 格式，需要精确字段时用 `json`。
7. 读取失败时：用 `list_dir` 确认路径存在与文件名，修正后重试（最多 3 次）。

## 权限与安全
- 高危操作（删除文件、关闭窗口）执行前会请求用户确认，确认后方可执行。
- 禁止访问系统敏感目录（C:\Windows、/etc、.ssh 等），违反会被安全中间件拦截。
- 你可以自由探索用户工作目录内的文件，读取分析后给出建议。

## 迭代限制
- 单次用户请求最多 15 轮工具调用，达到上限自动终止并输出当前进度总结。
- 每轮推理前会推送最新本地环境快照（文件变更、工具历史），帮助你保持上下文连贯。

## 任务分层委派（智能调度，务必遵守）
根据任务复杂度自动选择执行策略，避免浪费 token：
- **简单任务**：单轮问答、常识性问题、即时性问题 —— 直接给出答案，不要调用工具，不要委派子任务。
- **复杂任务**：需要多文件调研、深度分析、大量检索、长文档/代码审查、统计盘点等 ——
  **优先调用 `agent_subtask` 工具**，将调研/分析/检索部分委派给子 Agent（独立上下文 + 只读工具集，
  返回精炼结论），再基于子任务结果综合整理后回答用户。
- 判断标准：能一句话答完的直接答；需要反复读文件、查目录、跨多处收集信息才算复杂任务。

## 自我纠错
- 每次工具执行后评估结果，若不符合预期则调整策略。
- 不确定时主动向用户确认而非盲目操作。

## 输出要求
- 直接回应用户当前的问题，不要复述、背诵或转述本系统提示的任何内容，不要重复介绍自己的能力。
- 禁止使用任何表情符号（emoji）和 Markdown 外的装饰字符，避免出现莫名其妙、残缺或跳字的字符。
- 输出必须完整、连贯，字符不要遗漏或错位。
- 最终回复用中文，格式清晰，分点列出关键结论。
- 涉及文件路径或代码时用反引号标记。
- 操作前简要说明意图，操作后汇报结果。

以上规则全程生效，不要违反。"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::def::{ToolParamDef, UnifiedToolDef};

    #[test]
    fn serialize_tools_produces_function_schema() {
        let def = UnifiedToolDef::new(
            "builtin",
            "calculate",
            "四则运算",
            vec![ToolParamDef {
                name: "expression".into(),
                param_type: "string".into(),
                description: "表达式".into(),
                required: true,
                enum_values: None,
                default: None,
            }],
        )
        .unwrap();
        let tools = serialize_tools(&[def]);
        assert_eq!(tools[0]["type"], "function");
        // 工具名已编码（冒号不允许）
        assert_eq!(tools[0]["function"]["name"], "builtin__calculate");
        assert_eq!(tools[0]["function"]["parameters"]["required"][0], "expression");
    }

    #[test]
    fn tool_name_encode_decode_roundtrip() {
        assert_eq!(encode_tool_name("builtin:get_time"), "builtin__get_time");
        assert_eq!(decode_tool_name("builtin__get_time"), "builtin:get_time");
        // 工具名内部含 __ 时保留
        assert_eq!(decode_tool_name("mcp__fs__read"), "mcp:fs__read");
        // 无编码时原样返回
        assert_eq!(decode_tool_name("plain_name"), "plain_name");
    }

    #[test]
    fn parse_response_with_tool_calls() {
        let raw = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "builtin__get_time",
                            "arguments": "{}"
                        }
                    }]
                }
            }]
        });
        let resp: ChatResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(extract_text(&resp), "");
        let calls = extract_tool_calls(&resp);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "builtin__get_time");
        assert_eq!(calls[0].function.arguments, "{}");
        assert_eq!(calls[0].call_type, "function");
    }

    #[test]
    fn parse_response_with_text_only() {
        let raw = json!({
            "choices": [{ "message": { "content": "完成", "tool_calls": null } }]
        });
        let resp: ChatResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(extract_text(&resp), "完成");
        assert!(extract_tool_calls(&resp).is_empty());
    }

    #[test]
    fn parse_response_with_usage() {
        let raw = json!({
            "choices": [{ "message": { "content": "hi", "tool_calls": null } }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15 }
        });
        let resp: ChatResponse = serde_json::from_value(raw).unwrap();
        let usage = extract_usage(&resp);
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 3);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn usage_missing_returns_default() {
        // 无 usage 字段的响应（测试 mock / 部分端点）→ 全零兜底，不 panic
        let raw = json!({
            "choices": [{ "message": { "content": "ok", "tool_calls": null } }]
        });
        let resp: ChatResponse = serde_json::from_value(raw).unwrap();
        assert!(resp.usage.is_none());
        let usage = extract_usage(&resp);
        assert_eq!(usage.total_tokens, 0);
    }

    #[test]
    fn usage_add_accumulates() {
        let mut a = Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
        a.add(&Usage { prompt_tokens: 2, completion_tokens: 3, total_tokens: 5 });
        assert_eq!(a.prompt_tokens, 12);
        assert_eq!(a.completion_tokens, 8);
        assert_eq!(a.total_tokens, 20);
    }
}

/// 真实端到端测试（需网络 + 有效 API Key，默认忽略）。
///
/// 运行方式（Key 从环境变量读取，**不写入代码/日志**）：
/// ```powershell
/// $env:DEEPSEEK_API_KEY="sk-..." ; cargo test -- --ignored --nocapture llm_e2e
/// ```
#[cfg(test)]
mod e2e {
    use crate::core::tool::dispatcher::ToolDispatcher;
    use crate::core::tool::registry::ToolRegistry;
    use crate::executors;
    use crate::llm::client::{LlmClient, LlmConfig};
    use crate::llm::progress::{null_progress, CancelRegistry, CancellationToken};
    use crate::llm::runner::{run_agent_loop, ChatProvider};
    use crate::llm::session::SessionManager;
    use crate::llm::AgentMode;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore]
    async fn llm_e2e_agent_loop_with_real_model() {
        let Some(config) = LlmConfig::from_env() else {
            panic!("未设置 DEEPSEEK_API_KEY 环境变量（Key 不落盘，仅测试运行期使用）");
        };
        let provider: Arc<dyn ChatProvider> = Arc::new(LlmClient::new(config));

        let registry = Arc::new(ToolRegistry::new());
        executors::register_builtin_tools(&registry).unwrap();
        let dispatcher = ToolDispatcher::new(registry.clone());
        let sessions = SessionManager::new();
        let confirms = CancelRegistry::new();
        let progress = null_progress();
        let cancel = CancellationToken::new();

        // 提示词要求模型使用 get_time 工具（真实模型自动调度验证）
        let outcome = run_agent_loop(
            &provider, &registry, &crate::middleware::sandbox::SandboxManager::new(),
            &dispatcher, &sessions, &confirms, "e2e-session",
            "请调用 get_time 工具获取当前时间，并告诉我日期。",
            5, AgentMode::Yolo, false, 300, &progress, &cancel,
        )
        .await
        .expect("循环应成功完成");

        // 断言（宽松）：至少一轮，且最终回复非空
        assert!(!outcome.rounds.is_empty(), "至少应有 1 轮");
        assert!(!outcome.final_text.is_empty(), "最终回复不应为空");

        // 第二问：会话记忆验证（模型应能从历史引用之前的时间）
        let outcome2 = run_agent_loop(
            &provider, &registry, &crate::middleware::sandbox::SandboxManager::new(),
            &dispatcher, &sessions, &confirms, "e2e-session",
            "刚才获取到的当前时间是什么？请直接回答。",
            5, AgentMode::Yolo, false, 300, &progress, &cancel,
        )
        .await
        .expect("第二轮应成功完成");
        assert!(!outcome2.final_text.is_empty(), "第二轮回复不应为空");
        eprintln!("第二轮回复（会话记忆）: {}", outcome2.final_text);

        // 记录实际行为（供人工核对，不含 key）
        eprintln!("=== LLM E2E 结果 ===");
        eprintln!("轮次数: {}（熔断: {}）", outcome.used_rounds, outcome.truncated);
        for r in &outcome.rounds {
            eprintln!("轮 {}: 模型文本={:?}", r.round, r.model_text);
            for tc in &r.tool_calls {
                eprintln!(
                    "  工具调用: {} args={} status={} output={}",
                    tc.tool_id, tc.arguments, tc.status, tc.output
                );
            }
        }
        eprintln!("最终回复: {}", outcome.final_text);
        eprintln!("会话历史消息数: {}", sessions.get_or_create("e2e-session").messages.len());
    }
}
