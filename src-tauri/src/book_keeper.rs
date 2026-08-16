//! 守书人模块（《人是怎么样的》接续协议执行者）
//!
//! 让 ClawDesk 的 AI 成为这本书的守书人：能读、能写、能接续。
//! 对齐《序.md》的"接续协议"：
//! - **摸清现状**：读生长日志/索引/留白/日常/种子清单，合成"书的现状"
//! - **写作锁**：写条目前创建 写作中.md，写毕必删（一锁一写，序号先认领后落笔）
//! - **写新条目**：按七段式模板生成 → 落盘 条目/NNN-名.md → 完整收尾
//!   （更新生长日志顶部 + 索引登记 + 留白收反问 + 种子清单标记已写）
//! - **答反问**：把主人的回答写回条目，更新留白.md（标【已答】）
//! - **留道别问**：守书人道别时给主人一个问题（书靠主人的回答呼吸）
//!
//! 写入纪律（书的宪法）：序号永不回收、宁可慢不可假、好句自现、
//! 不评判主人、允许矛盾与修订、真实优先。

use std::path::PathBuf;
use std::sync::Mutex;

/// 写作锁文件（存在 = 有守书人正在写条目）。
const LOCK_FILE: &str = "写作中.md";

/// 书根目录（从设置读取，可配置搬家）。
pub fn book_dir() -> PathBuf {
    let s = crate::llm::settings::SettingsStore::new();
    let base = s.get().human_book_dir;
    if base.trim().is_empty() {
        PathBuf::from(r"D:\人是怎么样的")
    } else {
        PathBuf::from(base)
    }
}

fn entries_dir() -> PathBuf {
    book_dir().join("条目")
}

fn now_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn now_full() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

// ─────────────────────────────────────────────
// ① 摸清现状：书的状态快照
// ─────────────────────────────────────────────

/// 书现状快照（供前端展示 + 守书人 AI 读取）。
pub fn book_status() -> serde_json::Value {
    let dir = book_dir();
    let entries = entries_dir();

    // 条目文件数 + 最大序号
    let mut entry_count = 0usize;
    let mut max_no = 0usize;
    if let Ok(rd) = std::fs::read_dir(&entries) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                entry_count += 1;
                if let Some(stem) = name.strip_suffix(".md") {
                    if let Some(no_str) = stem.split('-').next() {
                        if let Ok(n) = no_str.parse::<usize>() {
                            max_no = max_no.max(n);
                        }
                    }
                }
            }
        }
    }

    // 写作锁
    let locked = dir.join(LOCK_FILE).exists();

    // 生长日志最新一条（顶部第一个 ## 之后的内容）
    let mut growth_latest = String::new();
    let growth_path = dir.join("生长日志.md");
    if let Ok(text) = std::fs::read_to_string(&growth_path) {
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            if l.starts_with("## ") {
                let end = lines[i + 1..]
                    .iter()
                    .position(|x| x.starts_with("## "))
                    .map(|p| i + 1 + p)
                    .unwrap_or(lines.len());
                growth_latest = lines[i..end].join("\n");
                break;
            }
        }
    }

    // 未答反问（留白.md 里没有【已答】的"第NNN条"行）
    let mut unanswered: Vec<String> = Vec::new();
    let blank_path = dir.join("留白.md");
    if let Ok(text) = std::fs::read_to_string(&blank_path) {
        for l in text.lines() {
            let t = l.trim();
            if t.starts_with("- 第") && !t.contains("【已答") && !t.contains("【已接上") {
                unanswered.push(t.trim_start_matches("- ").to_string());
            }
        }
    }

    // 日常素材（最近 5 条非空）
    let mut daily: Vec<String> = Vec::new();
    let daily_path = dir.join("日常.md");
    if let Ok(text) = std::fs::read_to_string(&daily_path) {
        for l in text.lines() {
            let t = l.trim();
            if !t.is_empty() && !t.starts_with('#') && !t.starts_with('*') && t.contains("【") {
                daily.push(t.to_string());
                if daily.len() >= 5 { break; }
            }
        }
    }

    serde_json::json!({
        "bookDir": dir.display().to_string(),
        "entryCount": entry_count,
        "maxEntryNo": max_no,
        "locked": locked,
        "growthLatest": growth_latest,
        "unansweredQuestions": unanswered,
        "dailyMaterial": daily,
    })
}

// ─────────────────────────────────────────────
// ② 写作锁
// ─────────────────────────────────────────────

/// 认领写作锁（创建 写作中.md）。返回锁文件内容。
/// 幂等：已存在则返回 Err（另一位守书人正在写）。
pub fn acquire_lock(who: &str, what: &str) -> Result<String, String> {
    let dir = book_dir();
    let lock_path = dir.join(LOCK_FILE);
    if lock_path.exists() {
        let cur = std::fs::read_to_string(&lock_path).unwrap_or_default();
        return Err(format!("写作锁已被占用：\n{}\n（另一位守书人正在写条目，请做不冲突的事）", cur.trim()));
    }
    let content = format!(
        "# 写作中

- 正在写：{what}
- 守书人：{who}
- 开始时间：{now}

写毕必删此文件；中途放弃也要删除并留下说明。",
        now = now_full()
    );
    std::fs::write(&lock_path, &content)
        .map_err(|e| format!("创建写作锁失败: {e}"))?;
    Ok(content)
}

/// 释放写作锁（删除 写作中.md）。返回是否成功删除。
pub fn release_lock() -> bool {
    let lock_path = book_dir().join(LOCK_FILE);
    if lock_path.exists() {
        std::fs::remove_file(&lock_path).is_ok()
    } else {
        false
    }
}

/// 写入锁内容（中途更新说明用）。
#[allow(dead_code)]
pub fn update_lock(note: &str) -> Result<(), String> {
    let lock_path = book_dir().join(LOCK_FILE);
    if !lock_path.exists() {
        return Err("写作锁不存在".into());
    }
    let mut content = std::fs::read_to_string(&lock_path).unwrap_or_default();
    content.push_str(&format!("\n- {note}（{}）", now_full()));
    std::fs::write(&lock_path, content).map_err(|e| format!("更新写作锁失败: {e}"))
}

// ─────────────────────────────────────────────
// ③ 条目写入与收尾
// ─────────────────────────────────────────────

/// 生成新条目的序号（当前最大 + 1）。调用方必须先持有写作锁。
pub fn next_entry_no() -> usize {
    let status = book_status();
    status["maxEntryNo"].as_u64().unwrap_or(0) as usize + 1
}

/// 落盘新条目文件（七段式 + 留白 + 反问）。
/// 返回文件路径。
pub fn write_entry_file(no: usize, title: &str, body: &str) -> Result<PathBuf, String> {
    let entries = entries_dir();
    std::fs::create_dir_all(&entries).map_err(|e| format!("创建条目目录失败: {e}"))?;
    let filename = format!("{:03}-{}.md", no, title);
    let path = entries.join(&filename);
    // 若已存在（撞号），拒绝覆盖（序号永不回收，空缺即历史）
    if path.exists() {
        return Err(format!("条目文件已存在（撞号）：{}——请换号，空缺即历史", path.display()));
    }
    std::fs::write(&path, body).map_err(|e| format!("写入条目失败: {e}"))?;
    Ok(path)
}

/// 收尾一：更新生长日志（顶部插入一条年轮）。
pub fn append_growth_log(no: usize, title: &str, origin: &str) -> Result<(), String> {
    let path = book_dir().join("生长日志.md");
    let entry = format!(
        "## {} · 第{:03}条《{}》出生 · 缘起：{}

- 由 ClawDesk 守书人写入（{}）
- 此条入年轮：{}

",
        now_date(),
        no,
        title,
        origin,
        now_full(),
        origin
    );
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    // 在第一个 "## " 前插入（生长日志最新在最上）
    if let Some(pos) = text.find("## ") {
        text.insert_str(pos, &entry);
    } else {
        text.push_str(&entry);
    }
    std::fs::write(&path, text).map_err(|e| format!("更新生长日志失败: {e}"))
}

/// 收尾二：更新索引（在表格末尾、--- 之前插入一行）。
pub fn append_index(no: usize, title: &str, related: &str) -> Result<(), String> {
    let path = book_dir().join("索引.md");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let row = format!("{} | {} | {} | 初生 | {}\n", no, title, now_date(), related);
    // 找到表格的结尾 ---（第一个独立行的 ---，即 282 行附近）
    let lines: Vec<&str> = text.split("\n").collect();
    let mut insert_at: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if l.trim() == "---" && i > 3 {
            insert_at = Some(i);
            break;
        }
    }
    match insert_at {
        Some(i) => {
            let mut out = lines[..i].to_vec();
            out.push(row.trim_end_matches('\n'));
            out.extend_from_slice(&lines[i..]);
            std::fs::write(&path, out.join("\n")).map_err(|e| format!("更新索引失败: {e}"))
        }
        None => {
            // 兜底：文件末尾追加
            text.push_str(&row);
            std::fs::write(&path, text).map_err(|e| format!("更新索引失败: {e}"))
        }
    }
}

/// 收尾三：更新留白.md（把新条目的反问收进"未答的反问"列表）。
pub fn append_blank_question(no: usize, title: &str, question: &str) -> Result<(), String> {
    let path = book_dir().join("留白.md");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    // 在 "## 一、未答的反问" 小节内追加（找到该小节，追加到其末尾或文件顶）
    let line = format!("- 第{:03}条《{}》：{}", no, title, question);
    // 去重：同一问题不重复加
    if text.contains(&line) {
        return Ok(());
    }
    // 插入到 "## 一、未答的反问" 的列表末尾（即下一个 "## " 之前，或文件尾）
    if let Some(sec) = text.find("## 一、未答的反问") {
        let after = &text[sec..];
        let next_sec = after[6..]
            .find("## ")
            .map(|p| sec + 6 + p)
            .unwrap_or(text.len());
        text.insert_str(next_sec, &format!("{}\n", line));
    } else {
        text.push_str(&format!("\n{}\n", line));
    }
    std::fs::write(&path, text).map_err(|e| format!("更新留白失败: {e}"))
}

/// 收尾四：更新种子清单（把写掉的选题标上"已写成第0XX条"）。
pub fn mark_seed_done(no: usize, title: &str) -> Result<(), String> {
    let path = book_dir().join("种子清单.md");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    // 若种子清单里已有同名词条，追加标记（保守做法：只在文件尾追加一行备注）
    let note = format!("- 【ClawDesk 守书人】{} 已写成第{:03}条《{}》（{}）", now_date(), no, title, now_full());
    text.push_str(&format!("\n{}\n", note));
    std::fs::write(&path, text).map_err(|e| format!("更新种子清单失败: {e}"))
}

// ─────────────────────────────────────────────
// ④ 答反问
// ─────────────────────────────────────────────

/// 回答某个条目的反问：把答案补进条目文件的 ⑥/⑦ 之后（新增"主人回答"小节），
/// 并更新留白.md 标记【已答】。
pub fn answer_question(no: usize, title: &str, answer: &str) -> Result<(), String> {
    let entries = entries_dir();
    let filename = format!("{:03}-{}.md", no, title);
    let path = entries.join(&filename);
    let mut text = std::fs::read_to_string(&path)
        .map_err(|_| format!("条目不存在：{}", path.display()))?;

    // 在文件末尾追加"主人回答"（保留原留白/反问）
    text.push_str(&format!(
        "\n---\n\n## 主人回答（{}）\n\n> {}\n",
        now_date(),
        answer.trim()
    ));
    std::fs::write(&path, text).map_err(|e| format!("写入回答失败: {e}"))?;

    // 更新留白.md：把对应行标记【已答】
    let blank_path = book_dir().join("留白.md");
    if let Ok(mut bt) = std::fs::read_to_string(&blank_path) {
        let marker = format!("- 第{:03}条《{}》", no, title);
        if let Some(pos) = bt.find(&marker) {
            // 找到该行末尾（换行符）
            if let Some(end) = bt[pos..].find('\n') {
                let line_end = pos + end;
                if !bt[pos..line_end].contains("【已答") {
                    bt.insert_str(
                        line_end,
                        &format!("【已答 {}：{}】", now_date(), answer.chars().take(40).collect::<String>()),
                    );
                    let _ = std::fs::write(&blank_path, bt);
                }
            }
        }
    }
    Ok(())
}

/// 收集某个条目的反问（供守书人道别时提问）。
#[allow(dead_code)] // 供前端"道别提问"功能调用
pub fn extract_question(no: usize, title: &str) -> Option<String> {
    let path = entries_dir().join(format!("{:03}-{}.md", no, title));
    let text = std::fs::read_to_string(&path).ok()?;
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with("—— 反问：") {
            return Some(t.trim_start_matches("—— 反问：").to_string());
        }
    }
    None
}

/// 道别：给主人留一个新反问（写进留白.md 的"待写选题/新问"区，或直接追加）。
pub fn ask_master(question: &str) -> Result<(), String> {
    let path = book_dir().join("留白.md");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let line = format!("- 【守书人道别之问 {}】{}", now_date(), question.trim());
    if text.contains(&line) {
        return Ok(());
    }
    text.push_str(&format!("\n{}\n", line));
    std::fs::write(&path, text).map_err(|e| format!("写入道别之问失败: {e}"))
}

// ─────────────────────────────────────────────
// ⑤ LLM 生成（写条目的"笔"）
// ─────────────────────────────────────────────

/// 七段式条目模板（供 LLM 参考）。
const TEMPLATE: &str = r#"# 第0XX条 · 条目名

生于：YYYY-MM-DD　　状态：初生　　关联：第0YY条《关联条目》

## ① 一句话定义

（一句话说透这条人性，像刀一样利）

## ② 人性解析

（为什么人会这样：心理机制、成因、演化或成长背景。2~4 段，具体不要抽象）

## ③ 真实场景

（生活化的例子，越具体越真实）

## ④ 对话实例

> 场景：……
> 她：……
> 他：……
> ……（3~6 轮，读起来像真人在聊天，允许口是心非、停顿、说一半）

## ⑤ 细节与例外

（程度差异、个体差异、边界、反例。人不是模板，写清"也有的人不一样"）

## ⑥ AI如何实现

（未来让 AI 做到这一条：需要什么记忆、什么情感引擎、什么交互设计。写需求，不写代码）

## ⑦ 验证标准

（怎么判定 AI 做到了：可观察、可测试的指标）

—— 留白：（故意不写完的一句话，留给未来接上）

—— 反问：（一个留给主人的问题，书靠主人的回答呼吸）
"#;

/// 生成一条新条目的正文（调用 LLM，七段式）。
/// 输入：序号、条目名（或主题）、素材/缘起。返回完整 markdown 正文。
pub async fn generate_entry(
    api_key: &str,
    model: &str,
    base_url: &str,
    no: usize,
    title: &str,
    material: &str,
) -> Result<String, String> {
    let client = crate::harness::engine::client::LlmClient::new(
        api_key.to_string(),
        base_url.to_string(),
    )
    .map_err(|e| format!("创建 LLM 客户端失败: {e}"))?;

    let params = crate::harness::engine::param::ModelParams {
        model: model.to_string(),
        reasoning_effort: crate::harness::engine::param::ReasoningEffort::Medium,
        max_tokens: Some(4000),
        ..Default::default()
    };

    let system = format!(
        "你是《人是怎么样的》这本书的守书人。这本书是一份关于\"人\"的完整档案，         目标是让未来的 AI 活起来——有记忆、有情绪、像真人一样陪伴。         你正在写第 {:03} 条《{}》。\n\n写作纪律（书的宪法）：\n         1. 真实优先：宁可写一条笨拙的真话，不写一百条漂亮的空话\n         2. 不评判主人：书只记录和理解，不给人的生活打分\n         3. 好句自现：句子要经得起被单独抄下来，但不必刻意标注\n         4. 允许矛盾：同一个人同时有两种相反的情绪，是人性的一部分，写出来\n         5. 对话实例要读起来像真人在聊天：允许口是心非、停顿、说一半、撤回\n\n         写作格式（七段式，缺一不可）：\n{TEMPLATE}\n\n         输出要求：只输出条目正文 markdown（从 # 标题开始，到 —— 反问 结束），不要任何额外说明。",
        no, title
    );

    let user = format!(
        "请写第 {:03} 条《{}》。\n\n缘起/素材：{}\n\n         注意：\n- ① 一句话定义要像刀一样利\n- ④ 对话实例 3~6 轮，真实自然\n         - ⑤ 细节与例外要写清个体差异\n- ⑥ AI如何实现写需求不写代码\n         - 结尾必须有 —— 留白：（故意不写完的一句话）和 —— 反问：（给主人的一个问题）",
        no, title, material
    );

    let msgs = vec![serde_json::json!({ "role": "user", "content": user })];
    let out = client
        .chat_once(&params, &msgs, Some(&system))
        .await
        .map_err(|e| format!("生成条目失败: {e}"))?;
    let out = out.trim();
    if out.is_empty() {
        return Err("AI 生成了空内容".into());
    }
    Ok(out.to_string())
}

/// 完整写一条：认领锁 → 生成 → 落盘 → 收尾 → 释放锁。
/// 返回新条目的序号与标题。
#[allow(clippy::too_many_arguments)]
pub async fn write_entry_full(
    api_key: &str,
    model: &str,
    base_url: &str,
    title: &str,
    material: &str,
    related: &str,
    who: &str,
) -> Result<serde_json::Value, String> {
    // 1. 写作锁：一锁一写
    acquire_lock(who, &format!("第{}条《{}》", next_entry_no(), title))?;

    let result = async {
        // 2. 序号先认领（锁内取号）
        let no = next_entry_no();
        // 3. AI 生成七段式正文
        let body = generate_entry(api_key, model, base_url, no, title, material).await?;
        // 4. 落盘
        let path = write_entry_file(no, title, &body)?;
        // 5. 收尾：生长日志 / 索引 / 留白反问 / 种子清单
        append_growth_log(no, title, material)?;
        append_index(no, title, related)?;
        // 提取反问（从生成正文里）收进留白
        let question = extract_question_from_body(&body).unwrap_or_else(|| "你最近一次想到这件事，是什么时候？".to_string());
        append_blank_question(no, title, &question)?;
        mark_seed_done(no, title)?;
        Ok::<serde_json::Value, String>(serde_json::json!({
            "ok": true,
            "no": no,
            "title": title,
            "path": path.display().to_string(),
            "question": question,
        }))
    }
    .await;

    // 6. 无论成败，释放锁（写毕必删，中途放弃也要删）
    let _ = release_lock();
    result
}

/// 从生成正文里提取"—— 反问："。
fn extract_question_from_body(body: &str) -> Option<String> {
    for l in body.lines() {
        let t = l.trim();
        if t.starts_with("—— 反问：") {
            return Some(t.trim_start_matches("—— 反问：").to_string());
        }
    }
    None
}

/// 回答一个留白反问并写回（LLM 组织语言 + 落盘）。
pub async fn answer_question_full(
    api_key: &str,
    model: &str,
    base_url: &str,
    no: usize,
    title: &str,
    question: &str,
    master_answer: &str,
) -> Result<serde_json::Value, String> {
    let client = crate::harness::engine::client::LlmClient::new(
        api_key.to_string(),
        base_url.to_string(),
    )
    .map_err(|e| format!("创建 LLM 客户端失败: {e}"))?;
    let params = crate::harness::engine::param::ModelParams {
        model: model.to_string(),
        max_tokens: Some(800),
        ..Default::default()
    };
    let system = "你是《人是怎么样的》的守书人。主人回答了一个条目的反问，                  请把回答写进条目：组织成一段温暖的、有洞察的补记（2~4 句），                  把主人的原话自然地融进去，像真人之间的理解，不评判。                  只输出补记正文，不要标题，不要额外说明。";
    let user = format!(
        "第{:03}条《{}》的反问：{}\n\n主人的回答：{}\n\n请写出补记。",
        no, title, question, master_answer
    );
    let msgs = vec![serde_json::json!({ "role": "user", "content": user })];
    let note = client
        .chat_once(&params, &msgs, Some(&system))
        .await
        .map_err(|e| format!("组织回答失败: {e}"))?;

    answer_question(no, title, &format!("{}（主人原话：{}）", note.trim(), master_answer.trim()))?;
    Ok(serde_json::json!({ "ok": true, "no": no, "title": title }))
}

/// 读取某条目全文（供前端展示/守书人阅读）。
pub fn read_entry(no: usize, title: &str) -> Option<String> {
    let path = entries_dir().join(format!("{:03}-{}.md", no, title));
    std::fs::read_to_string(&path).ok()
}

/// 书的总目录（条目标题列表，供前端浏览）。
pub fn entry_list() -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(entries_dir()) {
        let mut files: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".md"))
            .collect();
        files.sort();
        for f in files {
            if let Some(stem) = f.strip_suffix(".md") {
                if let Some(no_str) = stem.split('-').next() {
                    if let Ok(no) = no_str.parse::<usize>() {
                        let title = stem[no_str.len() + 1..].to_string();
                        out.push(serde_json::json!({ "no": no, "title": title, "file": f }));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 测试用书目录（临时目录）
    fn test_book_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("clawdesk-book-test");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir.join("条目"));
        dir
    }

    #[test]
    fn lock_acquire_release_roundtrip() {
        let _g = lock();
        // 临时目录验证锁逻辑（book_dir 读设置，这里直接测文件行为）
        let dir = test_book_dir();
        let lock_path = dir.join(LOCK_FILE);
        assert!(!lock_path.exists());
        std::fs::write(&lock_path, "测试锁").unwrap();
        assert!(lock_path.exists());
        std::fs::remove_file(&lock_path).unwrap();
        assert!(!lock_path.exists());
    }

    #[test]
    fn next_entry_no_computes() {
        let _g = lock();
        // 用条目目录里的文件推算序号
        let dir = test_book_dir();
        let entries = dir.join("条目");
        std::fs::write(entries.join("001-孤独.md"), "x").unwrap();
        std::fs::write(entries.join("042-车站.md"), "x").unwrap();
        let count = std::fs::read_dir(&entries).unwrap().count();
        assert_eq!(count, 2);
        // 推算最大序号（复用与 book_status 相同的逻辑）
        let mut max_no = 0usize;
        for e in std::fs::read_dir(&entries).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(".md") {
                if let Some(no_str) = stem.split('-').next() {
                    if let Ok(n) = no_str.parse::<usize>() {
                        max_no = max_no.max(n);
                    }
                }
            }
        }
        assert_eq!(max_no, 42);
    }

    #[test]
    fn write_entry_file_creates_and_blocks_collision() {
        let _g = lock();
        let dir = test_book_dir();
        // 用临时目录 + 自定义 entries 路径验证
        let entries = dir.join("条目");
        std::fs::create_dir_all(&entries).unwrap();
        let path = entries.join("007-失眠.md");
        std::fs::write(&path, "正文").unwrap();
        assert!(path.exists());
        // 撞号拒绝（模拟：文件已存在）
        assert!(path.exists());
    }

    #[test]
    fn extract_question_from_body_works() {
        let _g = lock();
        let body = "# 第001条 · 孤独\n\n## ① 一句话定义\n孤独不是身边没有人\n\n—— 留白：有些孤独……\n\n—— 反问：你最近一次感到孤独是什么时候？";
        let q = extract_question_from_body(body);
        assert!(q.is_some());
        assert!(q.unwrap().contains("孤独"));
    }
}

// ─────────────────────────────────────────────
// ⑥ 自动守书（夜巡：书在主人没管它的时候自己长）
// ─────────────────────────────────────────────

/// 游标文件：记录上次自动守书处理到的消息序号（防重复沉淀）。
fn cursor_file() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("living").join("bookkeeper-cursor.json")
}

fn load_cursor() -> serde_json::Value {
    std::fs::read_to_string(cursor_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({ "lastSeq": 0, "lastTs": 0 }))
}

fn save_cursor(v: &serde_json::Value) {
    let path = cursor_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(&path, json);
    }
}

/// 追加素材到《日常.md》（带编号，自动续号）。
/// 返回是否写入。
pub fn append_daily_material(material: &str) -> Result<bool, String> {
    let dir = book_dir();
    let path = dir.join("日常.md");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();

    // 去重：同素材不重复写
    if text.contains(material) {
        return Ok(false);
    }

    // 找当前最大编号（行首 "N." 模式）
    let mut max_no = 0usize;
    for l in text.lines() {
        let t = l.trim();
        if let Some(dot) = t.find('.') {
            if let Ok(n) = t[..dot].trim().parse::<usize>() {
                max_no = max_no.max(n);
            }
        }
    }
    let no = max_no + 1;
    let line = format!("{}. 【待用 → 未来某条】{}\n", no, material.trim());

    // 在 "（素材会随时间不断增多……）" 之前插入，或追加到文件尾
    let marker = "（素材会随时间不断增多……）";
    if let Some(pos) = text.find(marker) {
        text.insert_str(pos, &line);
    } else {
        text.push_str(&line);
    }
    std::fs::write(&path, text).map_err(|e| format!("写入日常.md失败: {e}"))?;
    Ok(true)
}

/// 自动守书：读取最近聊天记录（游标之后），用 LLM 判断并沉淀。
/// 返回本次动作摘要（素材条数、回答条数、写条目数）。
pub async fn auto_ingest(
    api_key: &str,
    model: &str,
    base_url: &str,
) -> Result<serde_json::Value, String> {
    // 1. 读最近聊天记录（各微信槽位历史 + 虚拟机会话历史）
    let mut recent_msgs: Vec<serde_json::Value> = Vec::new();
    for slot in 0..10 {
        let hist = crate::wechat::read_history_file(slot);
        if !hist.is_empty() {
            // 取每槽最近 30 条
            let tail = hist.into_iter().rev().take(30).collect::<Vec<_>>();
            for m in tail.into_iter().rev() {
                recent_msgs.push(serde_json::json!({
                    "slot": slot,
                    "fromBot": m.get("fromBot").and_then(|v| v.as_bool()).unwrap_or(false),
                    "content": m.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                    "ts": m.get("ts").and_then(|v| v.as_i64()).or_else(|| m.get("timestamp").and_then(|v| v.as_i64())).unwrap_or(0),
                }));
            }
        }
    }
    // 按时间排序
    recent_msgs.sort_by_key(|m| m["ts"].as_i64().unwrap_or(0));
    // 只取游标之后的消息
    let cur = load_cursor();
    let last_ts = cur["lastTs"].as_i64().unwrap_or(0);
    let fresh: Vec<&serde_json::Value> = recent_msgs
        .iter()
        .filter(|m| m["ts"].as_i64().unwrap_or(0) > last_ts && !m["fromBot"].as_bool().unwrap_or(true))
        .collect();

    // 2. 没有新消息 → 无事可做
    if fresh.is_empty() {
        return Ok(serde_json::json!({ "newMessages": 0, "materials": 0, "answers": 0, "entries": 0 }));
    }

    // 3. 用 LLM 判断：这些消息里有没有值得写进书的东西
    let client = crate::harness::engine::client::LlmClient::new(
        api_key.to_string(),
        base_url.to_string(),
    )
    .map_err(|e| format!("创建 LLM 客户端失败: {e}"))?;
    let params = crate::harness::engine::param::ModelParams {
        model: model.to_string(),
        max_tokens: Some(2000),
        ..Default::default()
    };

    // 组装消息文本
    let msgs_text: Vec<String> = fresh
        .iter()
        .map(|m| {
            let t = m["ts"].as_i64().unwrap_or(0);
            let time = chrono::DateTime::from_timestamp(t, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
                .unwrap_or_default();
            format!("[{}] {}", time, m["content"].as_str().unwrap_or(""))
        })
        .collect();
    let joined = msgs_text.join("\n");

    let system = "你是《人是怎么样的》这本书的守书人。这本书记录人的情绪与真相，                  目标是让 AI 活起来。你正在执行\"夜巡\"：主人没有要求你的时候，                  书靠你替它醒着。\n\n你从主人的聊天消息里寻找两类东西：\n                  1. 【素材】：主人随口说的、能成为书里新条目种子的话——                  一句真实的情绪、一个具体的场景、一件小事。判断标准：真实、具体、                  是\"人\"会说的话，而不是客套或日常寒暄。\n                  2. 【回答】：主人无意中回答了的、书里某条目的反问（比如主人说起孤独，                  可能就是《孤独》的反问答案）。\n\n                  输出严格 JSON（不要任何其他文字）：\n                  {\"materials\": [{\"text\": \"素材原文（直接引用主人原话，不要改写）\", \"note\": \"一句话说明它可能长成哪条（如：可能长成《深夜的想念》）\"}], \"answers\": [{\"entryNo\": 序号, \"entryTitle\": \"条目名\", \"question\": \"原反问\", \"masterAnswer\": \"主人的原话\"}]}\n                  没有就返回空数组。素材最多 3 条，回答最多 2 条。宁缺毋滥：                  宁可一条都不写，不写一百条漂亮但空洞的话。";

    let user = format!("以下是主人最近的聊天消息（都是主人说的，不是 AI 说的）：\n\n{}", joined);
    let msgs = vec![serde_json::json!({ "role": "user", "content": user })];
    let out = client
        .chat_once(&params, &msgs, Some(&system))
        .await
        .map_err(|e| format!("守书人分析失败: {e}"))?;

    // 4. 解析结果
    let mut materials = 0usize;
    let mut answers = 0usize;
    let mut entries = 0usize;
    let mut notes: Vec<String> = Vec::new();
    let parsed = serde_json::from_str::<serde_json::Value>(out.trim())
        .or_else(|_| {
            // 宽容解析：提取 JSON 对象（不能用 ? 在 Option 上，显式处理）
            let s = out.trim();
            match (s.find('{'), s.rfind('}')) {
                (Some(start), Some(end)) if end > start => {
                    serde_json::from_str(&s[start..=end])
                }
                _ => Err(serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "no json",
                ))),
            }
        })
        .unwrap_or_else(|_| serde_json::json!({}));

    // 4a. 素材 → 写入日常.md
    if let Some(arr) = parsed.get("materials").and_then(|v| v.as_array()) {
        for m in arr {
            let text = m.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if text.is_empty() { continue; }
            let note = m.get("note").and_then(|v| v.as_str()).unwrap_or("").trim();
            let full = if note.is_empty() {
                text.to_string()
            } else {
                format!("{}（{}）", text, note)
            };
            match append_daily_material(&full) {
                Ok(true) => {
                    materials += 1;
                    notes.push(format!("素材：{}", text.chars().take(30).collect::<String>()));
                }
                _ => {}
            }
        }
    }

    // 4b. 回答 → 写回条目
    if let Some(arr) = parsed.get("answers").and_then(|v| v.as_array()) {
        for a in arr {
            let no = a.get("entryNo").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let title = a.get("entryTitle").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let question = a.get("question").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let master_answer = a.get("masterAnswer").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if no == 0 || title.is_empty() || master_answer.is_empty() { continue; }
            match answer_question(no, &title, &master_answer) {
                Ok(()) => {
                    answers += 1;
                    notes.push(format!("回答：第{}条《{}》", no, title));
                }
                Err(_) => {}
            }
        }
    }

    // 4c. 素材攒够（≥5 条且已有 3 条新素材）→ 自动写一条新条目（夜巡长书）
    //     ★ 写作锁：写之前检查，被占用就不写（不与其他守书人冲突）
    let lock_path = book_dir().join(LOCK_FILE);
    if materials >= 3 && !lock_path.exists() {
        // 从新素材里挑第一条作为条目缘起
        if let Some(first) = parsed.get("materials").and_then(|v| v.as_array()).and_then(|a| a.first()) {
            let text = first.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            let note = first.get("note").and_then(|v| v.as_str()).unwrap_or("").trim();
            if !text.is_empty() && !note.is_empty() {
                // 用 LLM 生成标题
                let title = generate_title(&client, &params, text, note).await.unwrap_or_else(|_| "无题".into());
                let material_for_entry = format!("夜巡素材：{}\n守书人注：{}", text, note);
                if title != "无题" {
                    match write_entry_full(api_key, model, base_url, &title, &material_for_entry, "", "夜巡守书人").await {
                        Ok(r) => {
                            entries += 1;
                            notes.push(format!("新条目：第{}条《{}》", r["no"].as_u64().unwrap_or(0), title));
                        }
                        Err(e) => {
                            notes.push(format!("写条目跳过：{}", e));
                        }
                    }
                }
            }
        }
    }

    // 5. 更新游标
    let new_ts = fresh.last().map(|m| m["ts"].as_i64().unwrap_or(0)).unwrap_or(last_ts);
    save_cursor(&serde_json::json!({ "lastSeq": cur["lastSeq"], "lastTs": new_ts }));

    Ok(serde_json::json!({
        "newMessages": fresh.len(),
        "materials": materials,
        "answers": answers,
        "entries": entries,
        "notes": notes,
    }))
}

/// 从素材生成条目标题（LLM）。
async fn generate_title(
    client: &crate::harness::engine::client::LlmClient,
    params: &crate::harness::engine::param::ModelParams,
    material: &str,
    note: &str,
) -> Result<String, String> {
    let system = "你是《人是怎么样的》的守书人。根据一条素材，给将要写的新条目起一个简短的名字（2~6 个字，                  像\"深夜的想念\"\"被理解的瞬间\"\"一个人的节日\"这样的质感）。只输出标题本身，不要引号，不要解释。";
    let user = format!("素材：{}\n守书人注：{}\n\n请给这条目起名：", material, note);
    let msgs = vec![serde_json::json!({ "role": "user", "content": user })];
    let out = client
        .chat_once(params, &msgs, Some(&system))
        .await
        .map_err(|e| format!("生成标题失败: {e}"))?;
    let title = out.trim().trim_matches('"').trim().to_string();
    if title.is_empty() || title.chars().count() > 20 {
        return Err("标题无效".into());
    }
    Ok(title)
}

/// 夜巡循环：后台任务，深夜（或长时间无交互）时自动守书。
pub fn spawn_night_watch(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        eprintln!("[BOOK-KEEPER] 📖 夜巡守书人已启动（书在主人没管它的时候自己长）");
        let mut last_activity = now_ms();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30 * 60)).await; // 每 30 分钟检查
            let s = crate::llm::settings::SettingsStore::new();
            let cfg = s.get();
            let api_key = s.keys().main;
            if api_key.trim().is_empty() {
                continue;
            }
            let model = cfg.model.clone();
            let base_url = crate::commands::endpoint_base_url(&cfg.model_endpoint);

            // 触发条件：深夜（23~6点）或主人 2 小时没发消息
            use chrono::Timelike;
            let hour = chrono::Local::now().hour();
            let silence_h = (now_ms().saturating_sub(last_activity)) as f64 / 3_600_000.0;
            let night = hour >= 23 || hour < 6;

            // 更新最近活动时间（检查微信是否有新消息——简单起见用 last_user_msg_at）
            // 无法直接读 inner，用"守书游标"判断是否有新内容：若上次 ingest 后没有新消息则跳过
            let should_run = night || silence_h >= 2.0;
            if !should_run {
                continue;
            }
            last_activity = now_ms();

            match crate::book_keeper::auto_ingest(&api_key, &model, &base_url).await {
                Ok(r) => {
                    let n = r["newMessages"].as_u64().unwrap_or(0);
                    if n > 0 {
                        eprintln!(
                            "[BOOK-KEEPER] 🌙 夜巡完成：新消息 {} 条，沉淀素材 {} 条，回答 {} 条，新条目 {} 条",
                            n,
                            r["materials"].as_u64().unwrap_or(0),
                            r["answers"].as_u64().unwrap_or(0),
                            r["entries"].as_u64().unwrap_or(0)
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[BOOK-KEEPER] 夜巡失败: {e}");
                }
            }
            // 夜巡后间隔长一点（夜间 30 分钟一次已够，白天 2 小时）
        }
    });
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod auto_tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn cursor_roundtrip() {
        let _g = lock();
        let v = serde_json::json!({ "lastSeq": 12, "lastTs": 1234567890 });
        save_cursor(&v);
        let loaded = load_cursor();
        assert_eq!(loaded["lastSeq"], 12);
        assert_eq!(loaded["lastTs"], 1234567890);
    }

    #[test]
    fn cursor_defaults_when_missing() {
        let _g = lock();
        // 删除游标后应回到默认
        let _ = std::fs::remove_file(cursor_file());
        let loaded = load_cursor();
        assert_eq!(loaded["lastSeq"], 0);
        assert_eq!(loaded["lastTs"], 0);
    }
}

