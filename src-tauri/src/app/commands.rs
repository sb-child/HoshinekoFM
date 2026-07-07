//! Tauri Commands —— 前端可调用的后端函数。
//!
//! 通过 `invoke()` 调用，如 `invoke("list_tabs")`。

use std::sync::Arc;

use tauri::{command, Emitter, State};
use tokio::sync::{mpsc, Mutex};
use tracing::warn;

use crate::app::tabs::TabState;

use super::state::AppState;

/// Tab 变更事件（后端 → 前端推送）。
#[derive(Debug, Clone, serde::Serialize)]
pub enum TabEvent {
    Added(TabState),
    Removed(TabState),
}

/// 获取所有 tab 列表。
#[command]
pub async fn list_tabs(state: State<'_, Arc<Mutex<AppState>>>) -> Result<Vec<TabState>, String> {
    let app_state = state.lock().await;
    let tabs = app_state.tabs.lock().await;
    Ok(tabs.get_all().to_vec())
}

/// 添加一个 tab。
#[command]
pub async fn add_tab(
    state: State<'_, Arc<Mutex<AppState>>>,
    tx: State<'_, mpsc::UnboundedSender<TabEvent>>,
    path: String,
) -> Result<TabState, String> {
    let app_state = state.lock().await;
    let mut tabs = app_state.tabs.lock().await;
    let tab = tabs.add_tab(path);
    tabs.save_to_disk();

    let _ = tx.send(TabEvent::Added(tab.clone()));
    Ok(tab)
}

/// 关闭一个 tab。
#[command]
pub async fn close_tab(
    state: State<'_, Arc<Mutex<AppState>>>,
    tx: State<'_, mpsc::UnboundedSender<TabEvent>>,
    id: u64,
) -> Result<(), String> {
    let app_state = state.lock().await;
    let mut tabs = app_state.tabs.lock().await;
    if let Some(removed) = tabs.remove_tab(id) {
        tabs.save_to_disk();
        let _ = tx.send(TabEvent::Removed(removed));
        Ok(())
    } else {
        warn!("tab {id} not found");
        Err(format!("tab {id} not found"))
    }
}

/// 前端注册事件监听（预留）。
#[command]
pub async fn tab_event_sink(
    _state: State<'_, Arc<Mutex<AppState>>>,
    _tx: State<'_, mpsc::UnboundedSender<TabEvent>>,
) -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// 多窗口命令
// ---------------------------------------------------------------------------

/// 创建新窗口（公共函数，供 command 和 InstanceServer 共用）。
///
/// 创建新窗口并 emit `navigate-to` 事件给每个 path。
/// 返回新窗口的 label。
pub fn create_window(app: &tauri::AppHandle, paths: Vec<String>) -> Result<String, String> {
    use tauri::WebviewWindowBuilder;

    let label = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        format!("window-{ts:x}")
    };

    let builder =
        WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::App("index.html".into()))
            .title("HoshinekoFM")
            .inner_size(800.0, 600.0);

    let window = builder
        .build()
        .map_err(|e| format!("failed to create window: {e}"))?;

    tracing::info!("created new window: {label}");

    // 递增全局窗口计数
    super::WINDOW_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    // 向新窗口发送导航路径，前端需要监听 "navigate-to" 创建 tab
    for p in &paths {
        let _ = window.emit("navigate-to", p);
    }
    if paths.is_empty() {
        // 默认打开 home
        let _ = window.emit("navigate-to", "/");
    }

    Ok(label)
}

/// 创建新窗口（Tauri 命令）。
///
/// 新窗口加载相同的前端（默认 URL），共享 AppState。
#[command]
pub async fn new_window(
    app: tauri::AppHandle,
    label: Option<String>,
    paths: Option<Vec<String>>,
) -> Result<String, String> {
    // label 参数暂未使用（未来可支持指定 label 创建具名窗口）
    let _ = label;
    let paths = paths.unwrap_or_default();
    create_window(&app, paths)
}

/// 获取当前窗口的 label。
#[command]
pub async fn get_window_label(window: tauri::Window) -> Result<String, String> {
    Ok(window.label().to_string())
}
