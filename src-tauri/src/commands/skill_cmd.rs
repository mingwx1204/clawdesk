//! 技能管理 IPC 命令（skillhub，项目 16）。

use tauri::{Manager, State};

use crate::commands::AppState;

/// 列出全部技能（source: skillhub，项目 16）。
/// 聚合「注册表中在册的」+「技能目录中存在的（含已禁用未注册的）」，
/// 保证禁用后的技能仍出现在列表中（enabled=false），可在 UI 上重新启用。
#[tauri::command]
pub fn skills_list(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Vec<serde_json::Value> {
    let disabled = state.settings.get().disabled_skills;
    let mut map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    // 注册表中在册的（含 builtin / 自进化生成等不在目录中的）
    for d in state.registry.list() {
        if d.id.starts_with("skillhub:") {
            map.insert(d.id, d.description);
        }
    }
    // 技能目录中存在的（覆盖描述；即使已被禁用卸载也能列出）
    if let Ok(dir) = app.path().app_data_dir() {
        for (id, desc) in crate::adapters::skillhub::list_skill_meta(&dir.join("skills")) {
            map.insert(id, desc);
        }
    }
    map.into_iter()
        .map(|(id, description)| {
            serde_json::json!({
                "id": id,
                "description": description,
                "enabled": !disabled.contains(&id),
            })
        })
        .collect()
}

/// 从设置应用禁用技能（启动 / 重扫 / 启用后调用）。
fn apply_disabled_skills(state: &AppState) {
    let disabled = state.settings.get().disabled_skills;
    for id in &disabled {
        if state.registry.unregister(id).is_some() {
            eprintln!("[SKILLHUB] 已禁用技能: {}", id);
        }
    }
}

/// 重新扫描用户技能目录：先卸载全部 skillhub 技能再重扫，
/// 干净反映新增 / 删除 / 启用状态（扫描 `app_data_dir/skills`）。
#[tauri::command]
pub fn skills_reload(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取应用数据目录失败: {e}"))?
        .join("skills");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建技能目录失败: {e}"))?;
    for def in state.registry.list_by_source("skillhub") {
        let _ = state.registry.unregister(&def.id);
    }
    let n = crate::adapters::skillhub::register_from_dir(&state.registry, &dir)
        .map_err(|e| format!("技能目录扫描失败: {e}"))?;
    apply_disabled_skills(&state);
    eprintln!("[SKILLHUB] 技能目录重扫完成，注册 {} 个技能", n);
    Ok(n)
}

/// 启用 / 禁用技能（立即生效 + 持久化）：
/// 禁用 → 从注册表卸载；启用 → 重扫技能目录重新注册。
#[tauri::command]
pub fn skills_set_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    skill_id: String,
    enabled: bool,
) -> Result<bool, String> {
    let mut disabled = state.settings.get().disabled_skills;
    if enabled {
        disabled.retain(|x| x != &skill_id);
    } else if !disabled.contains(&skill_id) {
        disabled.push(skill_id.clone());
    }
    let _ = state.settings.apply(serde_json::json!({ "disabledSkills": disabled }))?;

    if enabled {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("获取应用数据目录失败: {e}"))?
            .join("skills");
        let _ = crate::adapters::skillhub::register_from_dir(&state.registry, &dir)
            .map_err(|e| format!("技能目录扫描失败: {e}"))?;
        apply_disabled_skills(&state);
    } else {
        state.registry.unregister(&skill_id);
    }
    eprintln!(
        "[SKILLHUB] 技能 `{}` 已{}",
        skill_id,
        if enabled { "启用" } else { "禁用" }
    );
    Ok(enabled)
}
