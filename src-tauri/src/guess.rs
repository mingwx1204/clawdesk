//! 猜人物游戏 —— 真实 LLM 驱动。
//!
//! 玩法：用户心里想一个「人物」（真实历史人物 / 虚构角色 / 动漫游戏影视角色 / 文学人物），
//! AI 通过「是/否」类封闭式提问逐步缩小范围，最终猜出。
//!
//! 实现要点：
//! - 优先用 `deepseek-reasoner` 模型 → 真实思考链（reasoning_content）流式推到前端
//!   （对应界面的「已思考 N 秒」+ 思考过程展示）。
//! - reasoner 不可用（如 HTTP 404）时自动回退 `deepseek-chat`（无思考链，游戏仍可玩）。
//! - 游戏会话保存在内存（HashMap），事件推送到 `guess://progress`：
//!   `text_delta` / `thinking_delta` / `done` / `error`。
//! - 普通模式 ≤20 问；专家模式 ≤10 问（更高区分度问题）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use futures_util::StreamExt;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::harness::engine::client::LlmClient;
use crate::harness::engine::param::{ModelParams, ReasoningEffort};
use crate::harness::engine::stream::SseEvent;

/// 游戏事件通道。
const EVENT: &str = "guess://progress";

/// 一个进行中的游戏会话（内存态，应用退出即消失）。
struct GuessGame {
    expert: bool,
    messages: Vec<Value>,
    round: u32,
}

static GAMES: OnceLock<Mutex<HashMap<String, GuessGame>>> = OnceLock::new();

fn games() -> &'static Mutex<HashMap<String, GuessGame>> {
    GAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 猜人物游戏 system prompt（普通 / 专家模式）。
fn system_prompt(expert: bool) -> String {
    let mut s = String::from(
        "你是「猜人物」游戏大师。用户心里想了一个人物（真实历史人物、虚构角色、动漫/游戏/影视角色、文学人物皆可），\
        你要通过提问猜出 TA 是谁。\n\n\
        规则：\n\
        1. 每轮只问一个「是/否」类封闭式问题（用户只能回答：是 / 否 / 不确定 / 接近了 / 补充提示）\n\
        2. 问题要由大到小、层层缩小：先锁定大分类（真实还是虚构 → 时代/作品 → 身份 → 特征 → 独特标志）\n\
        3. 每轮先在心里根据已有回答维护「候选范围」，排除不可能的角色，思考要体现排除逻辑\n\
        4. 当候选范围很小时直接给出猜测，格式严格为：我猜是：XXX\n\
        5. 用户说「接近了」说明方向对，继续追问独特特征，不要急着猜\n\
        6. 猜错后要修正思路，根据新信息重新收窄\n\
        7. 最多 20 个问题内猜出\n\n\
        输出要求（重要）：\n\
        - 每次只输出一行：要么「问题：……」要么「我猜是：XXX」，不要输出任何解释或多余文字\n\
        - 问题要具体、信息量大，禁止「你是不是很有名？」这类低信息量问题\n\
        - 前几个问题用于锁定大范围，之后逐步细化\n",
    );
    if expert {
        s.push_str(
            "\n★ 专家模式：你最多只能问 10 个问题。每个问题都必须是信息量最大、最能一锤定音的「高区分度」问题\
            （优先问独特标志、关键关系、稀有特征），快速把范围缩到最小，争取 8 问内猜出。\n",
        );
    }
    s
}

/// 首轮用户消息：直接开始提问。
const FIRST_USER: &str =
    "我心中已经想好了一个人物。请直接开始问第一个问题，不要问「你想好了吗」之类的废话。";

/// 构造用户回答消息。
fn user_reply(answer: &str, hint: Option<&str>) -> String {
    let mut s = format!("用户回答：{answer}");
    if let Some(h) = hint {
        let h = h.trim();
        if !h.is_empty() {
            s.push_str(&format!("\n补充提示：{h}"));
        }
    }
    s.push_str(
        "\n请根据以上回答继续：如果候选范围已很小且有把握，直接给出猜测（格式「我猜是：XXX」）；\
         否则问下一个问题（格式「问题：XXX」）。",
    );
    s
}

/// 发起一轮流式对话：reasoner（真实思考链）→ chat（回退）。
async fn stream_turn(
    app: &AppHandle,
    game_id: String,
    expert: bool,
    mut msgs: Vec<Value>,
    round: u32,
) -> Result<(), String> {
    let cfg = crate::harness::engine::config::engine_config()
        .ok_or_else(|| "引擎未配置，请先在设置中填写 DeepSeek API Key 并发送一条消息".to_string())?;
    let client = LlmClient::new(cfg.api_key.clone(), cfg.base_url.clone())
        .map_err(|e| format!("初始化 LLM 客户端失败: {e}"))?;

    // 优先 reasoner（真实思考链），模型不可用/被拒时回退 chat
    let models = ["deepseek-reasoner", "deepseek-chat"];
    let mut last_err = String::new();
    for model in models {
        let params = ModelParams {
            model: model.to_string(),
            reasoning_effort: ReasoningEffort::Medium,
            temperature: Some(0.6),
            max_tokens: Some(1024),
            ..Default::default()
        };
        let no_tools = serde_json::json!([]);
        let stream = match client
            .stream_chat(&params, &msgs, Some(&system_prompt(expert)), &no_tools)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                last_err = e.to_string();
                continue; // 尝试下一个模型
            }
        };

        let mut full = String::new();
        let mut stream = stream;
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(SseEvent::TextDelta { content }) => {
                    full.push_str(&content);
                    let _ = app.emit(
                        EVENT,
                        serde_json::json!({ "type": "text_delta", "content": content }),
                    );
                }
                Ok(SseEvent::ThinkingDelta { content }) => {
                    let _ = app.emit(
                        EVENT,
                        serde_json::json!({ "type": "thinking_delta", "content": content }),
                    );
                }
                Ok(SseEvent::Error { message }) => return Err(message),
                Ok(_) => {}
                Err(e) => return Err(format!("读取流失败: {e}")),
            }
        }

        // 本轮完成：写回历史（assistant 消息）+ 通知前端
        msgs.push(serde_json::json!({ "role": "assistant", "content": full }));
        if let Some(g) = games().lock().unwrap().get_mut(&game_id) {
            g.messages = msgs;
        }
        let _ = app.emit(
            EVENT,
            serde_json::json!({ "type": "done", "text": full, "round": round }),
        );
        return Ok(());
    }

    Err(format!("模型调用失败（reasoner 与 chat 均不可用）: {last_err}"))
}

/// 开始一局新游戏，返回 game_id。
#[tauri::command]
pub async fn guess_start(
    app: AppHandle,
    api_key: String,
    base_url: String,
    expert: bool,
) -> Result<String, String> {
    // 写入引擎配置（与主聊天一致，key 仅内存态）
    crate::harness::engine::config::set_engine_config(crate::harness::engine::config::EngineConfig {
        api_key,
        base_url: if base_url.trim().is_empty() {
            "https://api.deepseek.com".to_string()
        } else {
            base_url
        },
        model: "deepseek-reasoner".to_string(),
        effort: ReasoningEffort::Medium,
    });

    let id = uuid::Uuid::new_v4().to_string();
    let msgs = vec![serde_json::json!({ "role": "user", "content": FIRST_USER })];
    games().lock().unwrap().insert(
        id.clone(),
        GuessGame {
            expert,
            messages: msgs.clone(),
            round: 0,
        },
    );

    let app2 = app.clone();
    let id2 = id.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = stream_turn(&app2, id2.clone(), expert, msgs, 1).await {
            let _ = app2.emit(EVENT, serde_json::json!({ "type": "error", "message": e }));
        }
    });
    Ok(id)
}

/// 用户回答后，AI 继续提问或猜测。
#[tauri::command]
pub async fn guess_reply(
    app: AppHandle,
    game_id: String,
    answer: String,
    hint: Option<String>,
) -> Result<(), String> {
    let (msgs, expert, round) = {
        let mut g = games().lock().unwrap();
        let game = g.get_mut(&game_id).ok_or_else(|| "游戏不存在或已过期，请重新开始".to_string())?;
        game.messages
            .push(serde_json::json!({ "role": "user", "content": user_reply(&answer, hint.as_deref()) }));
        game.round += 1;
        (game.messages.clone(), game.expert, game.round)
    };

    let app2 = app.clone();
    let id2 = game_id.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = stream_turn(&app2, id2.clone(), expert, msgs, round).await {
            let _ = app2.emit(EVENT, serde_json::json!({ "type": "error", "message": e }));
        }
    });
    Ok(())
}

/// 结束并清理一局（前端「重新开始」时调用）。
#[tauri::command]
pub fn guess_stop(game_id: String) -> bool {
    games().lock().unwrap().remove(&game_id).is_some()
}
