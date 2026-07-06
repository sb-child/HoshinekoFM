//! 应用全局状态。
//!
//! 通过 Tauri 的 `.manage()` 注入，所有 Tauri command 可通过 `State<'_, AppState>` 访问。

use std::sync::Arc;

use tokio::sync::Mutex;

use super::tabs::TabManager;
use crate::fsworker::FsWorkerPool;

/// 主进程的全局运行状态。
pub struct AppState {
    /// Tab 标签页管理器
    pub tabs: Arc<Mutex<TabManager>>,
    /// FS Worker 实例池
    pub workers: Arc<Mutex<FsWorkerPool>>,
    /// 是否为 primary 实例
    pub is_primary: bool,
    /// 当前实例 ID
    pub instance_id: u64,
}

impl AppState {
    /// 创建新的应用状态。
    pub fn new(is_primary: bool, instance_id: u64, tab_manager: TabManager) -> Self {
        Self {
            tabs: Arc::new(Mutex::new(tab_manager)),
            workers: Arc::new(Mutex::new(FsWorkerPool::new())),
            is_primary,
            instance_id,
        }
    }
}
