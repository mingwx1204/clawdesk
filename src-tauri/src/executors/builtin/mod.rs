//! builtin 源工具集 —— 内置执行器（阶段 1）。
//!
//! 每个工具一个模块文件，本文件仅做聚合注册。
//! 新增内置工具：新增文件 + 在 `register_all` 追加一行，**不得改动 core**。

pub mod analyze_image;
pub mod attachment;
pub mod browser;
pub mod calculate;
pub mod chart;
pub mod echo;
pub mod email;
pub mod file_ops;
pub mod generate_image;
pub mod get_time;
pub mod git_status;
pub mod knowledge;
pub mod ocr;
pub mod python_sandbox;
pub mod search_text;
pub mod snapshot;
pub mod subtask;
pub mod terminal;
pub mod web_search;
pub mod window;
pub mod window_screenshot;
pub mod wechat_ui;
pub mod vm;

use std::sync::Arc;

use crate::core::tool::error::ToolError;
use crate::core::tool::registry::ToolRegistry;

/// 注册全部内置工具。
pub fn register_all(registry: &Arc<ToolRegistry>) -> Result<(), ToolError> {
    attachment::register(registry)?;
    analyze_image::register(registry)?;
    browser::register(registry)?;
    chart::register(registry)?;
    echo::register(registry)?;
    email::register(registry)?;
    get_time::register(registry)?;
    calculate::register(registry)?;
    generate_image::register(registry)?;
    knowledge::register(registry)?;
    ocr::register(registry)?;
    file_ops::register(registry)?;
    python_sandbox::register(registry)?;
    terminal::register(registry)?;
    search_text::register(registry)?;
    window_screenshot::register(registry)?;
    git_status::register(registry)?;
    snapshot::register(registry)?;
    subtask::register(registry)?;
    web_search::register(registry)?;
    wechat_ui::register(registry)?;
    vm::register(registry)?;
    Ok(())
}
