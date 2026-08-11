//! 定时任务调度器 —— 到点自动让 AI 执行任务，结果写入独立会话 + 可选微信推送。
//!
//! 支持触发方式：
//! - `Daily { time }`：每天 HH:MM 触发一次
//! - `Weekly { weekday, time }`：每周指定日（1=周一 … 7=周日）HH:MM 触发一次
//! - `Interval { seconds }`：每 N 秒触发（从添加时刻起计时）
//! - `Once { at_ms }`：一次性，到指定毫秒时间戳触发后自动禁用
//!
//! 持久化：`app_data_dir/scheduler.json`（明文，不含 API Key 等敏感信息）。
//! 执行：触发时走 `run_agent_loop`（与 `agent_chat` 同一引擎），结果写独立会话
//! `sched-<taskId>`，并 emit `scheduler://result` 事件（前端通知）；若任务开启
//! 微信推送且已绑定微信，则把结果发送给指定微信用户。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::AppState;
use crate::llm::progress::ProgressSink;
use crate::llm::runner::{run_agent_loop, ChatProvider};

/// 调度器心跳间隔（秒）：每 5 秒检查一次是否有到点任务。
const TICK_INTERVAL_SECS: u64 = 5;

/// 触发方式。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Schedule {
    /// 每天 HH:MM（24 小时制）触发一次。
    Daily { time: String },
    /// 每周指定日触发：weekday 1=周一 … 7=周日，time HH:MM。
    Weekly { weekday: u32, time: String },
    /// 每 N 秒触发一次。
    Interval { seconds: u64 },
    /// 一次性：到 at_ms（毫秒时间戳）触发，之后自动禁用。
    Once { at_ms: u64 },
}

/// 定时任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerTask {
    pub id: String,
    pub name: String,
    /// 交给 AI 执行的任务描述（即用户 prompt）。
    pub prompt: String,
    pub schedule: Schedule,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 上次触发时间（毫秒时间戳，用于去重 / 间隔计时）。
    #[serde(default)]
    pub last_run: u64,
    /// 结果是否推送到微信。
    #[serde(default)]
    pub push_wechat: bool,
    /// 微信推送目标用户（空则不推送；填了 push_wechat 才生效）。
    #[serde(default)]
    pub wechat_to: Option<String>,
    /// 微信推送槽位（0 = 微信1 …，默认 0）。旧数据无此字段时兼容为 0。
    #[serde(default)]
    pub wechat_slot: usize,
    /// 目标会话 ID（默认 `sched-<id>`，独立会话保存结果）。
    #[serde(default)]
    pub session_id: Option<String>,
    /// 是否显示桌面通知。
    #[serde(default = "default_true")]
    pub notify: bool,
}

fn default_true() -> bool {
    true
}

impl SchedulerTask {
    /// 上次触发的本地日期（YYYY-MM-DD），用于 Daily/Weekly 去重。
    fn last_run_date(&self) -> String {
        chrono::DateTime::from_timestamp_millis(self.last_run as i64)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
            .unwrap_or_default()
    }

    /// 判断当前时刻是否应触发（纯逻辑，由 tick 循环调用）。
    fn should_run(&self, now: u64) -> bool {
        if !self.enabled {
            return false;
        }
        let now_dt = chrono::Local::now();
        let cur_min = (now_dt.hour() * 60 + now_dt.minute()) as i64;
        match &self.schedule {
            Schedule::Daily { time } => {
                let (h, m) = parse_hhmm(time).unwrap_or((0, 0));
                cur_min >= (h * 60 + m) as i64 && self.last_run_date() != today_str()
            }
            Schedule::Weekly { weekday, time } => {
                let (h, m) = parse_hhmm(time).unwrap_or((0, 0));
                let dow = now_dt.weekday().num_days_from_monday() + 1; // 1=周一
                dow == *weekday
                    && cur_min >= (h * 60 + m) as i64
                    && self.last_run_date() != today_str()
            }
            Schedule::Interval { seconds } => {
                let secs = (*seconds).max(10); // 最短 10 秒，防误配
                self.last_run == 0 || now.saturating_sub(self.last_run) >= secs * 1000
            }
            Schedule::Once { at_ms } => self.last_run == 0 && now >= *at_ms,
        }
    }
}

fn today_str() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 解析 "HH:MM" → (时, 分)；非法返回 None。
fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 调度器内部状态。
pub struct SchedulerInner {
    pub tasks: Mutex<Vec<SchedulerTask>>,
    pub data_dir: Mutex<Option<PathBuf>>,
}

pub struct SchedulerState(pub Arc<SchedulerInner>);

impl Default for SchedulerState {
    fn default() -> Self {
        Self(Arc::new(SchedulerInner {
            tasks: Mutex::new(Vec::new()),
            data_dir: Mutex::new(None),
        }))
    }
}

// ─── 持久化 ───

fn tasks_file(inner: &Arc<SchedulerInner>) -> Option<PathBuf> {
    let dir = inner.data_dir.lock().clone()?;
    Some(dir.join("scheduler.json"))
}

fn save_tasks(inner: &Arc<SchedulerInner>) {
    let Some(path) = tasks_file(inner) else { return };
    let tasks = inner.tasks.lock().clone();
    if let Ok(json) = serde_json::to_string_pretty(&tasks) {
        let _ = std::fs::write(path, json);
    }
}

fn load_tasks(inner: &Arc<SchedulerInner>) {
    let Some(path) = tasks_file(inner) else {
        eprintln!("[SCHED] ⚠️ tasks_file 返回 None（data_dir 未初始化），任务未加载");
        return;
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Vec<SchedulerTask>>(&text) {
            Ok(tasks) => {
                eprintln!("[SCHED] 已加载 {} 个定时任务: {}", tasks.len(), path.display());
                *inner.tasks.lock() = tasks;
            }
            Err(e) => {
                eprintln!("[SCHED] ⚠️ 任务文件解析失败（任务未加载）: {e}");
                eprintln!("[SCHED] 文件内容前 200 字符: {}", trunc_str(&text, 200));
            }
        },
        Err(e) => eprintln!("[SCHED] ⚠️ 任务文件读取失败: {e}"),
    }
}

fn trunc_str(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        chars[..max].iter().collect()
    }
}

/// 应用 setup 时初始化数据目录并加载已保存任务。
pub fn init_data_dir(app: &AppHandle, state: &SchedulerState) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        *state.0.data_dir.lock() = Some(dir);
    }
    load_tasks(&state.0);
}

// ─── 任务执行 ───

/// 触发一次任务：配置引擎 → 运行 Agent → 返回最终文本。
async fn run_task(app: AppHandle, task: &SchedulerTask) -> Result<String, String> {
    let state = app.state::<AppState>();
    let registry = state.registry.clone();
    let dispatcher = state.dispatcher.clone();
    let sandbox = state.sandbox.clone();
    let sessions = state.sessions.clone();
    let cancel_tokens = state.cancel_tokens.clone();
    let router = state.router.clone();
    let settings = state.settings.clone();
    let agent_mode = *state.agent_mode.read().unwrap();
    let max_rounds = *state.max_rounds.read().unwrap();
    drop(state);

    let keys = settings.keys();
    let api_key = keys.main;
    if api_key.trim().is_empty() {
        return Err("未配置 DeepSeek API Key，请先在「设置 → 模型 API」填写".into());
    }
    let s = settings.get();
    router.ensure_main_key(api_key.clone());
    crate::harness::engine::config::set_engine_config(crate::harness::engine::config::EngineConfig {
        api_key: api_key.clone(),
        base_url: "https://api.deepseek.com".to_string(),
        model: s.model.clone(),
        effort: crate::harness::engine::param::ReasoningEffort::Medium,
    });
    if !keys.vision.is_empty() {
        router.configure_vision(keys.vision.clone(), &s.vision_model, &s.vision_endpoint);
    }
    if !keys.image.is_empty() {
        router.configure_image(keys.image.clone(), &s.image_model, &s.image_endpoint);
    }

    let session_id = task
        .session_id
        .clone()
        .unwrap_or_else(|| format!("sched-{}", task.id));
    let run_id = format!("sched-{}", task.id);
    let cancel = cancel_tokens.create(run_id.clone());
    eprintln!(
        "[SCHED] 执行任务 {} ({}) session={}",
        task.name, task.id, session_id
    );

    let progress: ProgressSink = Box::new(|_| {});
    let provider: Arc<dyn ChatProvider> = router;
    let outcome = run_agent_loop(
        &provider,
        &registry,
        &sandbox,
        &dispatcher,
        &sessions,
        &cancel_tokens,
        &session_id,
        &task.prompt,
        max_rounds,
        agent_mode,
        false,
        300,
        &progress,
        &cancel,
        None, // 定时任务无人设
    )
    .await;
    cancel_tokens.remove(&run_id);
    outcome.map(|o| o.final_text).map_err(|e| e)
}

/// 调度主循环（setup 时 spawn；每 5 秒检查到点任务）。
pub(crate) async fn scheduler_loop(app: AppHandle, inner: Arc<SchedulerInner>) {
    eprintln!("[SCHED] 调度循环已启动（每 {} 秒检查一次）", TICK_INTERVAL_SECS);
    let mut ticker = tokio::time::interval(Duration::from_secs(TICK_INTERVAL_SECS));
    loop {
        ticker.tick().await;
        let now = now_ms();
        let due: Vec<SchedulerTask> = {
            let mut tasks = inner.tasks.lock();
            let mut due = Vec::new();
            for t in tasks.iter_mut() {
                if t.should_run(now) {
                    t.last_run = now;
                    due.push(t.clone());
                }
            }
            if !due.is_empty() {
                // Once 任务触发后自动禁用
                for t in tasks.iter_mut() {
                    if matches!(t.schedule, Schedule::Once { .. }) && t.last_run != 0 {
                        t.enabled = false;
                    }
                }
                // ★ 修复：显式释放锁后再持久化。
                //   旧代码 drop_tasks_lock(&tasks) 传引用不释放锁 → save_tasks 里
                //   重复 lock → parking_lot 同线程递归加锁 panic → 调度协程崩溃
                //   → 定时任务永不触发（卡死）。
                drop(tasks);
                save_tasks(&inner);
            }
            due
        };
        for t in due {
            eprintln!("[SCHED] 🕐 触发任务: {} ({})", t.name, t.id);
            let app2 = app.clone();
            tokio::spawn(async move {
                let result = run_task(app2.clone(), &t).await;
                match result {
                    Ok(text) => {
                        eprintln!("[SCHED] 任务 {} 完成", t.name);
                        // 桌面通知
                        if t.notify {
                            let _ = app2.emit(
                                "scheduler://result",
                                serde_json::json!({
                                    "taskId": t.id, "name": t.name, "ok": true,
                                    "result": text, "time": now_ms(),
                                }),
                            );
                        }
                        // 微信推送（指定槽位；默认 0 = 微信1，旧任务无槽位字段兼容）
                        if t.push_wechat {
                            if let Some(to) = &t.wechat_to {
                                if !to.is_empty() {
                                    let wc = app2.state::<crate::wechat::WechatBotState>();
                                    let inner = wc.bot(t.wechat_slot);
                                    // ★ 2026-08-12：处理 send_message 的 Result（原返回值被丢弃，失败无感知）
                                    if let Err(e) =
                                        crate::wechat::send_message(&inner, to, &text, None).await
                                    {
                                        eprintln!(
                                            "[SCHED] 微信推送失败 slot{} -> {}: {}",
                                            t.wechat_slot, to, e
                                        );
                                    } else {
                                        eprintln!(
                                            "[SCHED] 已推送微信 slot{} -> {}",
                                            t.wechat_slot, to
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[SCHED] 任务 {} 失败: {}", t.name, e);
                        let _ = app2.emit(
                            "scheduler://result",
                            serde_json::json!({
                                "taskId": t.id, "name": t.name, "ok": false,
                                "error": e, "time": now_ms(),
                            }),
                        );
                    }
                }
            });
        }
    }
}

// ─── Tauri 命令 ───

/// 列出全部定时任务。
#[tauri::command]
pub fn scheduler_list(state: State<'_, SchedulerState>) -> Vec<SchedulerTask> {
    state.0.tasks.lock().clone()
}

/// 添加定时任务（自动生成 id；Interval 从当前时刻起计时）。
#[tauri::command]
pub fn scheduler_add(
    state: State<'_, SchedulerState>,
    name: String,
    prompt: String,
    schedule: Schedule,
    push_wechat: Option<bool>,
    wechat_to: Option<String>,
    wechat_slot: Option<usize>,
    session_id: Option<String>,
    notify: Option<bool>,
) -> Result<SchedulerTask, String> {
    if name.trim().is_empty() {
        return Err("任务名称不能为空".into());
    }
    if prompt.trim().is_empty() {
        return Err("任务内容（prompt）不能为空".into());
    }
    // 校验时间格式
    match &schedule {
        Schedule::Daily { time } | Schedule::Weekly { time, .. } => {
            if parse_hhmm(time).is_none() {
                return Err(format!("时间格式应为 HH:MM，收到: {time}"));
            }
        }
        Schedule::Interval { seconds } => {
            if *seconds < 10 {
                return Err("间隔至少 10 秒".into());
            }
        }
        Schedule::Once { .. } => {}
    }
    let id = format!("t{}", now_ms());
    // Interval 从添加时刻起计时（不立即触发）；Once 保持 last_run=0 待触发
    let last_run = if matches!(schedule, Schedule::Interval { .. }) {
        now_ms()
    } else {
        0
    };
    let task = SchedulerTask {
        id: id.clone(),
        name: name.trim().to_string(),
        prompt: prompt.trim().to_string(),
        schedule,
        enabled: true,
        last_run,
        push_wechat: push_wechat.unwrap_or(false),
        wechat_to: wechat_to.filter(|s| !s.trim().is_empty()),
        wechat_slot: wechat_slot.unwrap_or(0).min(crate::wechat::MAX_BOTS - 1),
        session_id,
        notify: notify.unwrap_or(true),
    };
    let mut tasks = state.0.tasks.lock();
    tasks.push(task.clone());
    drop(tasks);
    save_tasks(&state.0);
    eprintln!("[SCHED] 添加任务 {} ({})", task.name, id);
    Ok(task)
}

/// 删除定时任务。
#[tauri::command]
pub fn scheduler_remove(state: State<'_, SchedulerState>, task_id: String) -> bool {
    let mut tasks = state.0.tasks.lock();
    let before = tasks.len();
    tasks.retain(|t| t.id != task_id);
    let removed = tasks.len() != before;
    drop(tasks);
    if removed {
        save_tasks(&state.0);
    }
    removed
}

/// 启用 / 停用定时任务。
#[tauri::command]
pub fn scheduler_set_enabled(
    state: State<'_, SchedulerState>,
    task_id: String,
    enabled: bool,
) -> bool {
    let mut tasks = state.0.tasks.lock();
    let mut changed = false;
    for t in tasks.iter_mut() {
        if t.id == task_id {
            t.enabled = enabled;
            changed = true;
        }
    }
    drop(tasks);
    if changed {
        save_tasks(&state.0);
    }
    changed
}

/// 立即手动触发一次任务（测试用），返回执行结果文本。
#[tauri::command]
pub async fn scheduler_trigger_now(
    app: AppHandle,
    state: State<'_, SchedulerState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let task = {
        let tasks = state.0.tasks.lock();
        tasks
            .iter()
            .find(|t| t.id == task_id)
            .cloned()
            .ok_or_else(|| format!("任务不存在: {task_id}"))?
    };
    let text = run_task(app, &task).await?;
    Ok(serde_json::json!({ "ok": true, "result": text }))
}

/// 调度器运行信息。
#[tauri::command]
pub fn scheduler_status(state: State<'_, SchedulerState>) -> serde_json::Value {
    let tasks = state.0.tasks.lock();
    serde_json::json!({
        "count": tasks.len(),
        "enabled": tasks.iter().filter(|t| t.enabled).count(),
        "tickSecs": TICK_INTERVAL_SECS,
    })
}
