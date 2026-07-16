//! 统一错误类型 -- 替代裸 String 错误。
//!
//! AGENTS.md 要求使用结构化错误代替 `Result<String>`。

use std::fmt;

/// 应用层统一错误。
///
/// 用于 Tauri 命令、UIService、FsService 等返回给前端时提供可区分的错误码。
#[derive(Debug)]
pub enum AppError {
    /// 权限不足（需提升 UID）
    PermissionDenied(String),
    /// 文件/目录未找到
    NotFound(String),
    /// 路径已存在
    AlreadyExists(String),
    /// IO 错误
    Io(std::io::Error),
    /// 进程间通信错误
    Ipc(String),
    /// 操作被取消
    Cancelled(String),
    /// 名称冲突（多文件操作中）
    NameConflict(String),
    /// 一般错误
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::PermissionDenied(msg) => write!(f, "权限不足: {msg}"),
            AppError::NotFound(msg) => write!(f, "未找到: {msg}"),
            AppError::AlreadyExists(msg) => write!(f, "已存在: {msg}"),
            AppError::Io(e) => write!(f, "IO 错误: {e}"),
            AppError::Ipc(msg) => write!(f, "IPC 错误: {msg}"),
            AppError::Cancelled(msg) => write!(f, "已取消: {msg}"),
            AppError::NameConflict(msg) => write!(f, "名称冲突: {msg}"),
            AppError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::Other(s.to_string())
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::Other(s)
    }
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.to_string()
    }
}
