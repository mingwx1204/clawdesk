//! 导出交付接口（项目 17，文档 §十五.3）。
//!
//! 一键导出完整项目成果：生成代码 / 生成图片 / 修改快照 / 任务执行报告 /
//! Token 消耗报表，打包为 zip 压缩包保存至本地（`%APPDATA%/clawdesk/exports/`）。
//! 说明：zip 打包复用系统 PowerShell `Compress-Archive`（不引入新依赖）。

use std::path::PathBuf;

/// 导出根目录：`<数据目录>/exports/`。
pub fn export_dir() -> PathBuf {
    crate::llm::settings::clawdesk_dir().join("exports")
}

/// 执行导出：收集成果文件 → 打包 zip → 返回压缩包路径。
pub fn export_all() -> Result<String, String> {
    let out_dir = export_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建导出目录失败: {}", e))?;

    // 1) 收集来源目录
    let base = crate::llm::settings::clawdesk_dir();

    // 工作目录：快照 + 日志 + 设置（存在才收集）
    let mut items: Vec<(&str, PathBuf)> = Vec::new();
    let snap_dir = base.join("snapshots");
    let log_dir = base.join("logs");
    let img_dir = std::env::temp_dir().join("clawdesk-generated");
    let db_path = base.join("sessions.db");

    if snap_dir.exists() {
        items.push(("snapshots", snap_dir));
    }
    if log_dir.exists() {
        items.push(("logs", log_dir));
    }
    if img_dir.exists() {
        items.push(("images", img_dir));
    }
    if db_path.exists() {
        items.push(("sessions.db", db_path));
    }

    // 2) 生成任务执行报告（文本摘要）
    let report = build_report(&items);
    let report_path = out_dir.join("task_report.txt");
    std::fs::write(&report_path, &report).map_err(|e| format!("写入报告失败: {}", e))?;
    items.push(("task_report.txt", report_path));

    // 3) 打包 zip（PowerShell Compress-Archive）
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let zip_path = out_dir.join(format!("clawdesk_export_{}.zip", stamp));
    let zip_str = zip_path.to_string_lossy().to_string();

    // 构造源路径参数（引号包裹，支持中文/空格路径）
    let sources = items
        .iter()
        .map(|(_, p)| format!("'{}'", p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(", ");

    let script = format!(
        "Compress-Archive -Path {} -DestinationPath '{}' -Force",
        sources, zip_str
    );

    let mut cmd = std::process::Command::new("powershell");
    crate::executors::builtin::terminal::hide_console(&mut cmd);
    let out = cmd
        .args(["-NoProfile", "-Command", &script])
        .output()
        .map_err(|e| format!("启动打包失败: {}", e))?;

    if out.status.success() && zip_path.exists() {
        crate::llm::logging::audit("export", &format!("导出交付包: {}", zip_str));
        Ok(zip_str)
    } else {
        Err(format!(
            "打包失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// 生成任务执行报告摘要。
fn build_report(items: &[(&str, PathBuf)]) -> String {
    let mut lines = vec![
        format!("ClawDesk 项目成果导出报告"),
        format!("导出时间: {}", chrono::Local::now().to_rfc3339()),
        format!("包含内容: {}", items.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")),
        String::new(),
    ];
    for (name, path) in items {
        let size = std::fs::metadata(path)
            .map(|m| m.len())
            .unwrap_or(0);
        lines.push(format!("- {}: {} ({} bytes)", name, path.display(), size));
    }
    lines.push(String::new());
    lines.push("由 ClawDesk 自动生成，可用于项目归档与分发。".into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_base<T>(f: impl FnOnce() -> T) -> T {
        let _g = crate::llm::logging::test_env_lock();
        let dir = std::env::temp_dir().join(format!("clawdesk-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("APPDATA").ok();
        std::env::set_var("APPDATA", &dir);
        let result = f();
        match old {
            Some(v) => std::env::set_var("APPDATA", v),
            None => std::env::remove_var("APPDATA"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn report_contains_items() {
        with_temp_base(|| {
            let items = vec![
                ("logs", PathBuf::from("D:\\logs")),
                ("sessions.db", PathBuf::from("D:\\s.db")),
            ];
            let report = build_report(&items);
            assert!(report.contains("logs"));
            assert!(report.contains("sessions.db"));
        });
    }

    #[test]
    fn export_dir_created() {
        with_temp_base(|| {
            let dir = export_dir();
            std::fs::create_dir_all(&dir).unwrap();
            assert!(dir.exists());
        });
    }
}
