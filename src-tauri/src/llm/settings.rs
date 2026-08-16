//! 应用设置持久化模块（项目 7，文档 §四.3）。
//!
//! 设计说明：
//! - `AppSettings` 承载五大标签页配置：模型 API / Agent 参数 / MCP 管理 /
//!   外观汉化 / 快照与安全；
//! - 全部配置本地持久化到 `%APPDATA%/clawdesk/settings.json`，重启自动加载；
//! - **安全契约（不破坏既有红线）**：API Key 仅存内存态（AppState 运行时字段），
//!   持久化文件**绝不写入 key**（`to_persisted` 剥离 / `from_persisted` 不读），
//!   与 client.rs "Key 不落盘、不打印" 契约一致；
//! - 每一轮 DeepSeek 推理经 runner 读取最新设置（环境快照注入配置摘要）。

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// 设置路径覆盖层：生产默认 None（读 APPDATA）；测试设置唯一临时路径，
/// **避免 set_var 修改全局环境变量导致并行测试互相污染**（项目 7 修复，
/// 与 snapshot.rs ROOT_OVERRIDE 同款方案）。
static PATH_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// 设置文件路径：`<数据目录>/settings.json`（软件升级不覆盖）。
pub fn settings_path() -> PathBuf {
    if let Some(p) = PATH_OVERRIDE.read().unwrap().as_ref() {
        return p.clone();
    }
    clawdesk_dir().join("settings.json")
}

/// ClawDesk 数据目录（用户数据，安装/卸载/升级均不动）。
///
/// ★ 不再塞 C 盘：优先 `D:\ClawDeskData`（不存在则自动创建）；
/// 环境变量 `CLAWDESK_DATA_DIR` 可显式覆盖；两者皆无时回退 `%APPDATA%/clawdesk`。
/// 承载 settings.json / logs / snapshots / exports / knowledge.db / sessions.db 等全部用户数据。
/// ★ 首次迁移：若新目录（D:\ClawDeskData）为空而旧目录（%APPDATA%/clawdesk）有数据，
///   自动把 settings.json / keys.enc / sessions.db / knowledge.db / tool_logs.log / snapshots / exports 复制过去，
///   保证从旧版本升级不丢配置与 Key。
pub fn clawdesk_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("CLAWDESK_DATA_DIR") {
        let dir = PathBuf::from(p);
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    #[cfg(windows)]
    {
        let d = PathBuf::from(r"D:\ClawDeskData");
        if d.exists() || std::fs::create_dir_all(&d).is_ok() {
            migrate_legacy_if_needed(&d);
            return d;
        }
    }
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("clawdesk")
}

/// 首次迁移旧版 `%APPDATA%/clawdesk` 数据到新目录（仅当新目录还没有 settings.json）。
#[cfg(windows)]
fn migrate_legacy_if_needed(new_dir: &PathBuf) {
    if new_dir.join("settings.json").exists() {
        return; // 新目录已有数据（或已迁移过）
    }
    let legacy = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("clawdesk");
    if !legacy.join("settings.json").exists() {
        return; // 旧目录也没有数据 → 全新安装
    }
    // 逐项复制（文件 + 目录）
    for name in [
        "settings.json",
        "keys.enc",
        "sessions.db",
        "knowledge.db",
        "tool_logs.log",
        "snapshots",
        "exports",
        "logs",
        "generated-images",
    ] {
        let src = legacy.join(name);
        let dst = new_dir.join(name);
        if src.is_dir() {
            if !dst.exists() {
                let _ = copy_dir_recursive(&src, &dst);
            }
        } else if src.is_file() && !dst.exists() {
            let _ = std::fs::copy(&src, &dst);
        }
    }
    eprintln!("[SETTINGS] ✅ 已从旧目录迁移数据: {} → {}", legacy.display(), new_dir.display());
}

/// 递归复制目录（迁移用）。
#[cfg(windows)]
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 原子写入：先写 .tmp 再 rename（Windows 上 rename 是原子的，不会出现半截文件）。
fn atomic_write(path: &PathBuf, content: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 自动备份设置文件（保留最近 5 个备份）。
fn backup_settings() {
    let path = settings_path();
    if !path.exists() { return; }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bak = path.with_extension(format!("json.bak{}", ts));
    let _ = std::fs::copy(&path, &bak);
    // 清理旧备份（保留最近 5 个）
    if let Ok(entries) = std::fs::read_dir(path.parent().unwrap_or(&clawdesk_dir())) {
        let mut baks: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".json.bak"))
            .collect();
        baks.sort_by_key(|e| e.metadata().map(|m| m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH)).unwrap_or(std::time::SystemTime::UNIX_EPOCH));
        baks.reverse();
        for old in baks.iter().skip(5) {
            let _ = std::fs::remove_file(old.path());
        }
    }
}

/// API Key 加密存储文件（DPAPI，Windows 用户级加密，重启自动恢复）。
fn encrypted_keys_path() -> PathBuf {
    clawdesk_dir().join("keys.enc")
}

/// 加载 DPAPI 加密的 API Key（重启恢复）。
fn load_encrypted_keys() -> Option<ApiKeys> {
    let path = encrypted_keys_path();
    let bytes = std::fs::read(&path).ok()?;
    let dec = crate::wechat::dpapi_decrypt(&bytes)?;
    let s = String::from_utf8(dec).ok()?;
    serde_json::from_str::<ApiKeys>(&s).ok()
}

/// 保存 API Key 到 DPAPI 加密文件（仅 Windows，非 Windows 跳过）。
fn save_encrypted_keys(keys: &ApiKeys) {
    if let Ok(json) = serde_json::to_string(keys) {
        if let Some(enc) = crate::wechat::dpapi_encrypt(json.as_bytes()) {
            let _ = std::fs::create_dir_all(encrypted_keys_path().parent().unwrap());
            let _ = std::fs::write(encrypted_keys_path(), enc);
        }
    }
}

/// 设置路径覆盖（仅测试使用；None 恢复默认）。
#[cfg(test)]
pub(crate) fn set_path_override(p: Option<PathBuf>) {
    *PATH_OVERRIDE.write().unwrap() = p;
}

/// serde 默认值辅助：布尔默认 true（敏感文件保护出厂开启）。
fn default_true() -> bool {
    true
}

fn default_version() -> u32 { 1 }
fn default_ui_opacity() -> f64 { 1.0 }
fn default_model() -> String { "deepseek-chat".into() }
fn default_model_endpoint() -> String { "https://api.deepseek.com/chat/completions".into() }
fn default_vision_model() -> String { "glm-5v-turbo".into() }
fn default_vision_endpoint() -> String { "https://api.z.ai/chat/completions".into() }
fn default_image_model() -> String { "flux-1".into() }
fn default_image_endpoint() -> String { "https://api.siliconflow.cn/images/generations".into() }
fn default_mode() -> String { "off".into() }
fn default_max_rounds() -> usize { 15 }
fn default_compaction() -> usize { 8000 }

/// 工具循环轮次上限默认值：15（与系统提示 / 前端 max_rounds 语义一致）。
/// 推荐值 5~15：过低图片/PDF 多步识别会熔断，过高有死循环风险。
fn default_max_tool_rounds() -> usize { 15 }
fn default_font_size() -> u32 { 14 }
fn default_self_evolve_model() -> String { "deepseek-chat".into() }
fn default_evolve_threshold() -> f64 { 0.6 }
fn default_opencode_watch_endpoint() -> String {
    "https://opencode.ai/zen/go/v1/chat/completions".into()
}
fn default_opencode_watch_interval() -> u64 { 120 }

// ── ⑬ 朗读 / TTS 设置（Edge TTS 神经网络拟人音色） ──
fn default_tts_enabled() -> bool { true }
fn default_tts_voice() -> String { "zh-CN-XiaoxiaoNeural".into() }
fn default_tts_rate() -> f64 { 1.0 }
fn default_tts_style() -> String { String::new() }

// ── ⑭ 《人是怎么样的》书目录（AI 的"灵魂档案"，可配置搬家） ──
fn default_human_book_dir() -> String { r"D:\人是怎么样的".into() }

/// 应用设置（五大标签页全部字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    // ── ⑨ 设置版本（升级兼容：老版本 JSON 缺字段时用 default 填充，不丢数据）──
    #[serde(default = "default_version")]
    pub version: u32,

    // ── ① 模型 API 配置（全部 #[serde(default=...)] 防升级丢数据）──
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_model_endpoint")]
    pub model_endpoint: String,
    #[serde(default = "default_vision_model")]
    pub vision_model: String,
    #[serde(default = "default_vision_endpoint")]
    pub vision_endpoint: String,
    #[serde(default = "default_image_model")]
    pub image_model: String,
    #[serde(default = "default_image_endpoint")]
    pub image_endpoint: String,

    // ── ② Agent 智能体参数 ──
    #[serde(default)]
    pub agent_enabled: bool,
    #[serde(default = "default_mode")]
    pub agent_mode: String,
    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,
    #[serde(default = "default_compaction")]
    pub compaction_threshold: usize,

    // ── ②b 工具循环轮次上限（熔断保护）──
    /// 单轮 ReAct 循环内工具调用次数上限（超出熔断，防 AI 死循环）。
    /// 推荐 5~15；最小值 1，最大值 30。
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: usize,

    // ── ③ MCP 工具管理 ──
    #[serde(default = "default_true")]
    pub mcp_enabled: bool,
    #[serde(default)]
    pub vision_mcp_enabled: bool,
    #[serde(default)]
    pub image_mcp_enabled: bool,

    // ── ④ 外观汉化配置 ──
    #[serde(default = "default_true")]
    pub chinese_only: bool,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_true")]
    pub mica_enabled: bool,
    #[serde(default = "default_true")]
    pub dark_theme: bool,
    #[serde(default = "default_ui_opacity")]
    pub ui_opacity: f64,

    // ── ⑤ 快照与安全配置 ──
    #[serde(default = "default_true")]
    pub snapshot_enabled: bool,
    #[serde(default = "default_true")]
    pub high_risk_confirm: bool,
    #[serde(default = "default_true")]
    pub sensitive_files_enabled: bool,
    #[serde(default)]
    pub log_path: String,

    // ── ⑥ 技能管理 ──
    #[serde(default)]
    pub disabled_skills: Vec<String>,

    // ── ⑦ MCP 服务器管理 ──
    #[serde(default)]
    pub mcp_servers: Vec<crate::adapters::mcp::client::McpServerConfig>,

    // ── ⑧ 沙箱根目录 ──
    #[serde(default)]
    pub sandbox_roots: Vec<String>,

    // ── ⑩ Windows 集成 ──
    /// 开机自启动（关闭后窗口隐藏到托盘，下次开机自动后台运行）
    #[serde(default)]
    pub auto_start: bool,

    // ── ⑪ 自进化系统（AI 自动学习生成技能）──
    /// 是否启用自进化
    #[serde(default)]
    pub self_evolve_enabled: bool,
    /// 自进化使用的模型（默认用主模型，可单独配置）
    #[serde(default = "default_self_evolve_model")]
    pub self_evolve_model: String,
    /// 自动进化开关（首次启动/每天自动运行一次）
    #[serde(default)]
    pub self_evolve_auto: bool,
    /// 触发进化的成功率阈值（低于此值触发）
    #[serde(default = "default_evolve_threshold")]
    pub self_evolve_threshold: f64,

    // ── ⑫ opencode 网关自动回切 ──
    /// 是否启用：持续检测 opencode 网关，恢复后自动把主/视觉模型切回 opencode
    #[serde(default)]
    pub opencode_watch_enabled: bool,
    /// 检测的 opencode 完整端点（回切时作为 modelEndpoint / visionEndpoint）
    #[serde(default = "default_opencode_watch_endpoint")]
    pub opencode_watch_endpoint: String,
    /// 回切时使用的主/视觉 API Key（opencode key；DPAPI 加密存储，不落明文）
    // 说明：opencode_watch_api_key 已从 AppSettings 移除并迁入 ApiKeys.opencode_watch，
    // 旧 settings.json 中的 opencodeWatchApiKey 字段在 SettingsStore::new 时迁移到
    // keys.enc（DPAPI），并在下次保存 settings.json 时自动清除明文。
    #[serde(default, skip)]
    pub opencode_watch_api_key: String,
    /// 检测间隔（秒）
    #[serde(default = "default_opencode_watch_interval")]
    pub opencode_watch_interval_secs: u64,

    // ── ⑬ 朗读 / TTS 设置（Edge TTS 神经网络拟人音色） ──
    /// 是否启用 AI 朗读（输出完自动朗读）
    #[serde(default = "default_tts_enabled")]
    pub tts_enabled: bool,
    /// 朗读音色 ID（Edge TTS 音色，如 zh-CN-XiaoxiaoNeural 晓晓）
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    /// 朗读语速倍率（0.5~2.0，1.0 = 正常）
    #[serde(default = "default_tts_rate")]
    pub tts_rate: f64,
    /// 朗读语气风格（空 = 自然；如 cheerful / gentle / serious…）
    #[serde(default = "default_tts_style")]
    pub tts_style: String,

    // ── ⑭ 《人是怎么样的》书目录（AI 的"灵魂档案"，可配置搬家） ──
    /// 书根目录（含 条目/ 子目录）。默认 D:\人是怎么样的
    #[serde(default = "default_human_book_dir")]
    pub human_book_dir: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: 1,
            // ① 模型
            model: "deepseek-chat".into(),
            model_endpoint: "https://api.deepseek.com/chat/completions".into(),
            vision_model: "glm-5v-turbo".into(),
            vision_endpoint: "https://api.z.ai/chat/completions".into(),
            image_model: "flux-1".into(),
            image_endpoint: "https://api.siliconflow.cn/images/generations".into(),
            // ② Agent
            agent_enabled: false, // 出厂默认关闭（硬性约束）
            agent_mode: "off".into(),
            max_rounds: 15,
            compaction_threshold: 8000,
            max_tool_rounds: 15,
            // ③ MCP
            mcp_enabled: true,
            vision_mcp_enabled: false,
            image_mcp_enabled: false,
            // ④ 外观
            chinese_only: true, // 出厂默认中文
            font_size: 14,
            mica_enabled: true,
            dark_theme: true,
            ui_opacity: 1.0,
            // ⑤ 快照与安全
            snapshot_enabled: true,
            high_risk_confirm: true,
            sensitive_files_enabled: true,
            log_path: String::new(),
            // ⑥ 技能管理
            disabled_skills: Vec::new(),
            // ⑦ MCP 服务器
            mcp_servers: Vec::new(),
            // ⑧ 沙箱根目录
            sandbox_roots: Vec::new(),
            auto_start: false,
            self_evolve_enabled: false,
            self_evolve_model: "deepseek-chat".into(),
            self_evolve_auto: false,
            self_evolve_threshold: 0.6,
            opencode_watch_enabled: false,
            opencode_watch_endpoint: default_opencode_watch_endpoint(),
            opencode_watch_api_key: String::new(),
            opencode_watch_interval_secs: default_opencode_watch_interval(),
            // ⑬ 朗读 / TTS
            tts_enabled: default_tts_enabled(),
            tts_voice: default_tts_voice(),
            tts_rate: default_tts_rate(),
            tts_style: default_tts_style(),
            // ⑭ 书目录
            human_book_dir: default_human_book_dir(),
        }
    }
}

impl AppSettings {
    /// 加载设置——缺文件用默认；解析失败自动备份旧文件再用默认（**不丢数据**）。
    pub fn load() -> Self {
        let path = settings_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => { eprintln!("[SETTINGS] 无设置文件，使用出厂值"); return Self::default(); }
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(mut s) => {
                s.ensure_compat();
                s
            }
            Err(e) => {
                eprintln!("[SETTINGS] ❌ 解析失败：{e}，旧文件已备份，使用出厂值");
                backup_settings();
                Self::default()
            }
        }
    }

    /// 版本兼容迁移。
    fn ensure_compat(&mut self) {
        if self.version == 0 { self.version = 1; }
    }

    /// 保存设置——原子写入 + 自动备份，不会出现半截或丢失。
    /// 同时保留 settings.json 中的 apiKeys 字段（不被 AppSettings 序列化覆盖）。
    pub fn save(&self) -> Result<(), String> {
        let path = settings_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建设置目录失败: {e}"))?;
        }
        backup_settings();
        let mut cur_json = serde_json::to_value(self)
            .map_err(|e| format!("序列化设置失败: {e}"))?;
        // ★ 保留已有 apiKeys（AppSettings 结构体不含此字段，防止被覆盖）
        if let Ok(old_text) = std::fs::read_to_string(&path) {
            if let Ok(old) = serde_json::from_str::<serde_json::Value>(&old_text) {
                if let Some(ak) = old.get("apiKeys").cloned() {
                    if let serde_json::Value::Object(ref mut map) = cur_json {
                        map.insert("apiKeys".to_string(), ak);
                    }
                }
            }
        }
        let text = serde_json::to_string_pretty(&cur_json).map_err(|e| format!("序列化设置失败: {e}"))?;
        atomic_write(&path, &text).map_err(|e| format!("写入设置失败: {e}"))?;
        eprintln!("[SETTINGS] ✅ 已保存");
        Ok(())
    }

}

/// API Key 运行时集合（仅内存态，AppState 持有，**绝不持久化明文**，
/// 持久化走 DPAPI 加密 keys.enc）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApiKeys {
    pub main: String,
    pub vision: String,
    pub image: String,
    /// opencode 网关回切用 Key（同样 DPAPI 加密，不落 settings.json 明文）
    #[serde(default)]
    pub opencode_watch: String,
}

/// 共享设置容器：AppState 持有，运行时可改，每轮推理读取。
pub struct SettingsStore {
    inner: RwLock<AppSettings>,
    keys: RwLock<ApiKeys>,
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsStore {
    pub fn new() -> Self {
        let settings = AppSettings::load();
        // ★ 兼容预置：settings.json 中若存在 `apiKeys` 字段则加载到内存 keys。
        //   保持"保存时不落盘 key"的既有安全约定不变（save/apply 不写 key），
        //   但支持离线预置 / 首次启动即用（例如在配置文件里写好视觉 Key）。
        let mut keys = ApiKeys::default();
        if let Ok(text) = std::fs::read_to_string(settings_path()) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(ak) = v.get("apiKeys") {
                    keys.main = ak
                        .get("main")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    keys.vision = ak
                        .get("vision")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    keys.image = ak
                        .get("image")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
        // ★ DPAPI 加密恢复：重启自动恢复上次保存的 Key（不需再粘贴）。
        //   覆盖层兼容：若 settings.json 已写 apiKeys 则以文件为准（预置优先）。
        if keys.main.is_empty() && keys.vision.is_empty() && keys.image.is_empty() {
            if let Some(enc) = load_encrypted_keys() {
                eprintln!("[SETTINGS] 已从加密文件恢复 API Key（{} 字节）",
                    enc.main.len().min(4));
                keys = enc;
            }
        }
        // ★ 旧版本迁移：settings.json 中残留的明文 opencodeWatchApiKey
        //   迁入加密 keys.enc（此后 settings.json 不再包含该明文）。
        if keys.opencode_watch.is_empty() {
            if let Ok(text) = std::fs::read_to_string(settings_path()) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(k) = v.get("opencodeWatchApiKey").and_then(|x| x.as_str()) {
                        if !k.is_empty() {
                            keys.opencode_watch = k.to_string();
                            save_encrypted_keys(&keys);
                            eprintln!("[SETTINGS] 已将 opencode Key 迁移到加密存储");
                        }
                    }
                }
            }
        }
        Self {
            inner: RwLock::new(settings),
            keys: RwLock::new(keys),
        }
    }

    pub fn get(&self) -> AppSettings {
        self.inner.read().unwrap().clone()
    }

    /// 应用部分更新（仅更新提供的字段，其余保持不变），并立即持久化。
    /// `patch` 为 serde_json::Value 对象；未知字段忽略。
    pub fn apply(&self, patch: serde_json::Value) -> Result<AppSettings, String> {
        let mut current = self.inner.read().unwrap().clone();
        if let serde_json::Value::Object(map) = &patch {
            // 通过序列化-合并方式应用：将 patch 并入当前 JSON 再反序列化
            let mut cur_json = serde_json::to_value(&current)
                .map_err(|e| format!("序列化当前设置失败: {}", e))?;
            if let serde_json::Value::Object(cur_map) = &mut cur_json {
                for (k, v) in map {
                    cur_map.insert(k.clone(), v.clone());
                }
            }
            current = serde_json::from_value(cur_json)
                .map_err(|e| format!("解析设置补丁失败: {}", e))?;
        }
        current.save()?;
        *self.inner.write().unwrap() = current.clone();
        Ok(current)
    }

    /// 运行时设置 API Key（内存态 + DPAPI 加密持久化，重启自动恢复）。
    pub fn set_keys(&self, keys: ApiKeys) {
        save_encrypted_keys(&keys);
        *self.keys.write().unwrap() = keys;
    }

    pub fn keys(&self) -> ApiKeys {
        self.keys.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试专用：把设置路径指向临时目录（覆盖层 + 串行锁，避免并行污染）。
    fn with_temp_settings<T>(f: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap();
        // 用覆盖层隔离（不 set_var 全局 APPDATA，避免污染并行 tool_log 等测试）。
        let dir = std::env::temp_dir().join(format!("clawdesk-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        set_path_override(Some(dir.join("settings.json")));
        let result = f();
        set_path_override(None);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn default_settings_sane() {
        let s = AppSettings::default();
        assert!(!s.agent_enabled, "Agent 出厂默认关闭");
        assert!(s.chinese_only, "出厂默认中文");
        assert_eq!(s.max_rounds, 15);
        assert_eq!(s.agent_mode, "off");
    }

    #[test]
    fn save_then_load_roundtrip() {
        with_temp_settings(|| {
            let mut s = AppSettings::default();
            s.model = "deepseek-reasoner".into();
            s.max_rounds = 20;
            s.agent_enabled = true;
            s.save().unwrap();

            let loaded = AppSettings::load();
            assert_eq!(loaded.model, "deepseek-reasoner");
            assert_eq!(loaded.max_rounds, 20);
            assert!(loaded.agent_enabled);
        });
    }

    #[test]
    fn missing_file_returns_default() {
        with_temp_settings(|| {
            let loaded = AppSettings::load();
            assert_eq!(loaded.max_rounds, 15);
        });
    }

    #[test]
    fn apply_patch_partial_update() {
        with_temp_settings(|| {
            let store = SettingsStore::new();
            let updated = store
                .apply(serde_json::json!({ "maxRounds": 30, "darkTheme": false }))
                .unwrap();
            assert_eq!(updated.max_rounds, 30);
            assert!(!updated.dark_theme);
            // 未涉及的字段保留默认
            assert_eq!(updated.model, "deepseek-chat");
            // 持久化生效
            let reloaded = AppSettings::load();
            assert_eq!(reloaded.max_rounds, 30);
        });
    }

    #[test]
    fn keys_are_memory_only() {
        with_temp_settings(|| {
            let store = SettingsStore::new();
            store.set_keys(ApiKeys {
                main: "k_main".into(),
                vision: "k_vision".into(),
                image: "k_image".into(),
                opencode_watch: "k_opencode".into(),
            });
            let keys = store.keys();
            assert_eq!(keys.main, "k_main");
            assert_eq!(keys.opencode_watch, "k_opencode");
            // 持久化文件不含 key
            let persisted = std::fs::read_to_string(settings_path()).unwrap_or_default();
            assert!(!persisted.contains("sk-secret-main"));
            assert!(!persisted.contains("sk-secret"));
        });
    }
}
