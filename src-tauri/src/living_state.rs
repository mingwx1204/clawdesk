//! AI 生活状态模拟器（世界线 · 一生记忆）
//!
//! 让 AI 有"自己的日常"与"一生的记忆"：
//! - **时段状态机**：按真实时钟推断当前在做什么（睡觉/吃饭/打游戏/洗澡…），
//!   同一小时内状态稳定不漂移（防人格分裂）
//! - **生活事件固化**：每个小时首次查询时把该时段记入生活日志，落盘到
//!   `D:\ClawDeskData\living\{YYYY-MM}.jsonl`（按月分文件），跨重启延续
//! - **今日轨迹**：今天做了什么（"你今天的生活：08:12 🥛 在吃早饭…"）
//! - **近期记忆**：最近 N 天的生活时间线（注入 prompt，AI 记得昨天/前天）
//! - **一生记忆**：出生日期（首次运行时建档）+ 已生活的天数，AI 知道自己"活了多久"
//! - **全局单例**：一个 AI 人格一条世界线，多微信槽位共享（都是"同一个你"）

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};

/// 一条生活事件（如 19:20 吃晚饭：刚煮了面）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivingEvent {
    /// 毫秒时间戳
    pub ts_ms: u64,
    /// 事件种类（"slot" 表示时段固化事件）
    pub kind: String,
    /// 描述（如 "🎮 在放松（在打游戏，排位连跪中）"）
    pub label: String,
}

static EVENTS: OnceLock<Mutex<VecDeque<LivingEvent>>> = OnceLock::new();
static BORN_AT: OnceLock<Mutex<Option<i64>>> = OnceLock::new();

fn events() -> &'static Mutex<VecDeque<LivingEvent>> {
    EVENTS.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn born() -> &'static Mutex<Option<i64>> {
    BORN_AT.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 生活数据目录：D:\ClawDeskData\living\
fn living_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living")
}

/// 按月分文件：living/2026-08.jsonl
fn month_file(ts_secs: i64) -> PathBuf {
    let dt = chrono::DateTime::from_timestamp(ts_secs, 0)
        .map(|d| d.with_timezone(&Local))
        .unwrap_or_else(Local::now);
    living_dir().join(format!("{}-{:02}.jsonl", dt.year(), dt.month()))
}

/// 事件落盘（追加写，按月分文件）
fn save_event_disk(ev: &LivingEvent) {
    let dir = living_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = month_file((ev.ts_ms / 1000) as i64);
    let line = serde_json::to_string(ev).unwrap_or_default();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// 启动时初始化：恢复出生日期 + 加载最近生活事件（当月+上月，跨月不丢）
pub fn init() {
    let dir = living_dir();
    let _ = std::fs::create_dir_all(&dir);

    // 出生档案：首次运行创建（AI 的"生日"）
    let birth_path = dir.join("birth.json");
    let born_at = std::fs::read_to_string(&birth_path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("bornAt").and_then(|x| x.as_i64()));
    match born_at {
        Some(ts) => *born().lock().unwrap_or_else(|e| e.into_inner()) = Some(ts),
        None => {
            let ts = now_ms() as i64;
            *born().lock().unwrap_or_else(|e| e.into_inner()) = Some(ts);
            let born_desc = Local::now().format("%Y年%m月%d日 %H:%M").to_string();
            let _ = std::fs::write(
                &birth_path,
                serde_json::json!({ "bornAt": ts, "bornDesc": born_desc }).to_string(),
            );
            eprintln!("[LIVING] 🌱 AI 人生开始于 {}", born_desc);
        }
    }

    // 恢复最近事件：当月 + 上月文件（防跨月），按时间排序取最近 48 条
    let now = Local::now();
    let mut all: Vec<LivingEvent> = Vec::new();
    let mut months = vec![now];
    let prev = now - chrono::Duration::days(1);
    if prev.month() != now.month() || prev.year() != now.year() {
        months.push(prev);
    }
    for m in months {
        let path = month_file(m.timestamp());
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                if let Ok(ev) = serde_json::from_str::<LivingEvent>(line) {
                    all.push(ev);
                }
            }
        }
    }
    all.sort_by_key(|e| e.ts_ms);
    let mut q = events().lock().unwrap_or_else(|e| e.into_inner());
    for ev in all.into_iter().rev().take(48).collect::<Vec<_>>().into_iter().rev() {
        q.push_back(ev);
    }
    eprintln!(
        "[LIVING] 世界线恢复：{} 条生活记忆（{}）",
        q.len(),
        lifetime_desc()
    );
}

/// 时段状态表：(标签, emoji, 细节池)
/// 观察人类生活规律：7 点起、8 点早餐、上午搬砖、中午干饭、下午搬砖、
/// 傍晚晚饭、晚上放松（游戏/视频/散步）、21 点后洗漱躺平、23 点睡觉、凌晨勿扰。
fn state_table(hour: u32) -> (&'static str, &'static str, &'static [&'static str]) {
    match hour {
        0..=6 => (
            "在睡觉",
            "😴",
            &[
                "睡得正香，手机开了勿扰",
                "做了个乱七八糟的梦",
                "半夜醒了又睡回去",
                "手机屏幕亮了又被按掉",
            ],
        ),
        7 => (
            "刚起床",
            "🌅",
            &[
                "睡眼惺忪地摸手机",
                "正刷牙洗脸",
                "在纠结今天穿什么",
                "赖了会儿床刚爬起来",
            ],
        ),
        8 => (
            "在吃早饭",
            "🥛",
            &[
                "啃着包子配豆浆",
                "冲了杯咖啡提神",
                "边吃边刷早间新闻",
                "随便吃了两口就出门",
            ],
        ),
        9..=11 => (
            "在忙正事",
            "💻",
            &[
                "对着电脑搬砖",
                "刚开完一个会",
                "一边干活一边摸鱼",
                "在回一堆工作消息",
            ],
        ),
        12..=13 => (
            "在吃午饭",
            "🍚",
            &[
                "点了份外卖，边吃边刷剧",
                "在纠结吃啥，最后选了盖饭",
                "吃完饭下楼溜达了一圈",
                "跟同事拼单点了奶茶",
            ],
        ),
        14..=17 => (
            "在忙正事",
            "💻",
            &[
                "在赶今天的手头活",
                "开小差刷了会儿视频",
                "在整理东西，有点犯困",
                "刚忙完一阵，歇口气",
            ],
        ),
        18..=19 => (
            "在吃晚饭",
            "🍜",
            &[
                "刚煮好面，还加了个蛋",
                "在吃外卖，配着下饭视频",
                "跟朋友约了个饭",
                "在纠结吃啥，最后泡了碗面",
            ],
        ),
        20..=21 => (
            "在放松",
            "🎮",
            &[
                "在打游戏，排位连跪中",
                "在刷短视频，根本停不下来",
                "在追剧，刚更新了一集",
                "出门散步吹吹风",
                "在听歌发呆",
            ],
        ),
        22 => (
            "在洗漱/躺平",
            "🛁",
            &[
                "刚洗完澡，躺床上刷手机",
                "在收拾屋子",
                "敷着面膜刷视频",
                "躺在床上看搞笑视频",
            ],
        ),
        23 => (
            "准备睡觉",
            "🌙",
            &[
                "困得不行还在硬撑",
                "躺床上刷手机准备睡",
                "明天要早起，准备关灯",
                "眯了一会儿又精神了",
            ],
        ),
        _ => ("在忙", "💼", &["刚忙完手头的事", "在发呆"]),
    }
}

/// 当前小时在当天内的稳定种子（细节同一小时内不漂移）
fn hour_seed() -> u64 {
    let now = Local::now();
    let d = (now.year() as u64) * 10000 + (now.month() as u64) * 100 + now.day() as u64;
    d * 31 + now.hour() as u64
}

/// 生成当前生活状态描述（用于主动聊天 prompt 注入 / 自动回复 / 面板显示）。
/// 例：「现在是 20:35，你现在的状态：🎮 在放松（在打游戏，排位连跪中）」
/// 若 40 分钟内有刚结束的时段（如刚吃完晚饭），会附加"刚才在干嘛"的衔接。
pub fn current_state_desc() -> String {
    let now = Local::now();
    let h = now.hour();
    let m = now.minute();
    let wd = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"]
        [now.weekday().num_days_from_sunday() as usize];
    let (label, emoji, pool) = state_table(h);
    let idx = (hour_seed() % pool.len() as u64) as usize;
    let detail = pool[idx];
    let cur_label = format!("{emoji} {label}（{detail}）");

    // ★ 固化当前时段为生活事件（同一小时只记一次；锁外落盘）
    let hour_key = now.timestamp() as u64 / 3600;
    let new_event = {
        let mut q = events().lock().unwrap_or_else(|e| e.into_inner());
        if q.iter().any(|e| e.ts_ms / 3600_000 == hour_key) {
            None
        } else {
            let ev = LivingEvent {
                ts_ms: now_ms(),
                kind: "slot".to_string(),
                label: cur_label.clone(),
            };
            q.push_back(ev.clone());
            while q.len() > 48 {
                q.pop_front();
            }
            Some(ev)
        }
    };
    if let Some(ev) = new_event {
        save_event_disk(&ev);
    }

    // ★ 最近 40 分钟内的上一时段衔接（如"刚吃完晚饭"）→ 连贯的"刚才在干嘛"
    let prev_label = {
        let q = events().lock().unwrap_or_else(|e| e.into_inner());
        q.iter()
            .rev()
            .find(|e| {
                e.kind == "slot"
                    && e.ts_ms / 3600_000 != hour_key
                    && now_ms().saturating_sub(e.ts_ms) < 40 * 60_000
            })
            .map(|e| e.label.clone())
    };
    match prev_label {
        Some(p) => format!(
            "现在是{} {}点{:02}分，你刚才{}，现在{}",
            wd, h, m, p, cur_label
        ),
        None => format!(
            "现在是{} {}点{:02}分，你现在的状态：{}",
            wd, h, m, cur_label
        ),
    }
}

/// 今日生活轨迹（"你今天的生活：08:12 🥛 在吃早饭…"）。今天没有事件返回空。
pub fn today_timeline() -> String {
    let today = Local::now().date_naive();
    let q = events().lock().unwrap_or_else(|e| e.into_inner());
    let todays: Vec<&LivingEvent> = q
        .iter()
        .filter(|e| {
            chrono::DateTime::from_timestamp((e.ts_ms / 1000) as i64, 0)
                .map(|d| d.with_timezone(&Local).date_naive() == today)
                .unwrap_or(false)
        })
        .collect();
    if todays.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    for ev in todays {
        parts.push(format!(
            "{} {}",
            ts_to_local_str(ev.ts_ms, "%H:%M"),
            ev.label
        ));
    }
    format!("你今天的生活：{}", parts.join("；"))
}

/// 近期生活记忆（最近 days 天的时间线，限 recent_max 条避免 prompt 过长）。
/// 返回空表示没有可注入的记忆。
pub fn recent_life_memory(days: usize, recent_max: usize) -> String {
    let today = Local::now().date_naive();
    let q = events().lock().unwrap_or_else(|e| e.into_inner());
    let mut parts: Vec<String> = Vec::new();
    for ev in q.iter() {
        let Some(dt) = chrono::DateTime::from_timestamp((ev.ts_ms / 1000) as i64, 0) else {
            continue;
        };
        let dt = dt.with_timezone(&Local);
        let days_ago = today.signed_duration_since(dt.date_naive()).num_days();
        if days_ago < 0 || days_ago as usize > days {
            continue;
        }
        parts.push(format!(
            "{} {}",
            ts_to_local_str(ev.ts_ms, "%m-%d %H:%M"),
            ev.label
        ));
    }
    if parts.is_empty() {
        return String::new();
    }
    let last: Vec<String> = parts
        .into_iter()
        .rev()
        .take(recent_max)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("【你最近的生活记忆（时间线）】\n{}", last.join("\n"))
}

/// 一生记忆：出生日期 + 已生活的天数
pub fn lifetime_desc() -> String {
    let born_ts = *born().lock().unwrap_or_else(|e| e.into_inner());
    match born_ts {
        Some(ts) => {
            let born_desc = ts_to_local_str(ts as u64, "%Y年%m月%d日");
            let days = ((now_ms() as i64 - ts) / 86_400_000).max(0);
            format!("你出生于{}，已经生活了 {} 天", born_desc, days)
        }
        None => "你的人生刚刚开始".to_string(),
    }
}

/// 完整生活上下文（主动聊天 / 自动回复 prompt 注入）：
/// 当前状态 + 今日轨迹 + 近期记忆 + 一生记忆。控制在 ~500 字内。
pub fn living_context_for_prompt() -> String {
    let mut parts = Vec::new();
    parts.push(current_state_desc());
    let tl = today_timeline();
    if !tl.is_empty() {
        parts.push(tl);
    }
    let mem = recent_life_memory(2, 10);
    if !mem.is_empty() {
        parts.push(mem);
    }
    parts.push(lifetime_desc());
    parts.join("\n")
}

fn ts_to_local_str(ts_ms: u64, fmt: &str) -> String {
    chrono::DateTime::from_timestamp((ts_ms / 1000) as i64, 0)
        .map(|d| d.with_timezone(&Local).format(fmt).to_string())
        .unwrap_or_default()
}

/// 事件日志（调试/扩展用）：列出最近生活事件
pub fn recent_events() -> Vec<LivingEvent> {
    events().lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
}
