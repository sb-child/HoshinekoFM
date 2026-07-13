//! 窗口通信相关的 Tauri commands。
//!
//! 注意：不再有泛用的 `window_bus_send`/`broadcast`。
//! 具体的 DnD / Tab 操作由 `app/commands/` 中的专用命令处理。

use std::sync::Arc;

use tauri::State;
use tauri::Window;

use crate::app::state::AppStateManager;

/// 获取当前窗口的 window_id。
///
/// 查询 AppStateManager.windows[label] → WindowState.window_id。
#[tauri::command]
pub fn get_window_id(window: Window, mgr: State<'_, Arc<AppStateManager>>) -> Result<u64, String> {
    mgr.window_id_by_label(window.label())
        .ok_or_else(|| format!("window '{}' not registered", window.label()))
}
