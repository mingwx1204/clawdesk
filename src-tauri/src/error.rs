//! ClawDesk 统一错误类型：所有命令返回 Result<T, AppError>，
//! 前端拿到的是人类可读的中文错误信息，而不是裸 panic。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("路径不存在或不可访问: {0}")]
    PathNotFound(String),

    #[error("终端会话不存在或已退出")]
    TerminalNotFound,

    #[error("终端启动失败: {0}")]
    TerminalSpawn(String),

    #[error("目录监听失败: {0}")]
    Watch(String),

    #[error("截屏失败: {0}")]
    Screenshot(String),

    #[error("{0}")]
    Other(String),
}

// Tauri 命令要求错误可序列化
impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
