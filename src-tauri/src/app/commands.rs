//! Tauri Commands —— 前端可调用的后端函数。
//!
//! 通过 `invoke()` 调用，如 `invoke("list_tabs")`。

use std::sync::Arc;

use serde::Serialize;
use tauri::{command, Emitter, State};
use tokio::sync::mpsc;
use tracing::warn;

use crate::app::state::AppStateManager;
use crate::app::ui_service::{self, UIService};

/// Tab 变更事件（后端 → 前端推送）。
#[derive(Debug, Clone, serde::Serialize)]
pub enum TabEvent {
    Added(crate::ipc::protocol::TabState),
    Removed(crate::ipc::protocol::TabState),
}

/// 获取所有 tab 列表。
#[command]
pub fn list_tabs(
    mgr: State<'_, Arc<AppStateManager>>,
) -> Result<Vec<crate::ipc::protocol::TabState>, String> {
    let tabs = mgr.tabs.lock().unwrap();
    Ok(tabs.get_all().to_vec())
}

/// 添加一个 tab。
#[command]
pub fn add_tab(
    mgr: State<'_, Arc<AppStateManager>>,
    tx: State<'_, mpsc::UnboundedSender<TabEvent>>,
    path: String,
) -> Result<crate::ipc::protocol::TabState, String> {
    let mut tabs = mgr.tabs.lock().unwrap();
    let tab = tabs.add_tab(path);
    tabs.save_to_disk();

    let _ = tx.send(TabEvent::Added(tab.clone()));
    Ok(tab)
}

/// 关闭一个 tab。
#[command]
pub fn close_tab(
    mgr: State<'_, Arc<AppStateManager>>,
    tx: State<'_, mpsc::UnboundedSender<TabEvent>>,
    id: u64,
) -> Result<(), String> {
    let mut tabs = mgr.tabs.lock().unwrap();
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
pub fn tab_event_sink(
    _mgr: State<'_, Arc<AppStateManager>>,
    _tx: State<'_, mpsc::UnboundedSender<TabEvent>>,
) -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// 多窗口命令
// ---------------------------------------------------------------------------

/// 创建新窗口（使用给定 label）。
pub fn create_window(
    app: &tauri::AppHandle,
    label: &str,
    paths: &[String],
) -> Result<tauri::WebviewWindow, String> {
    use tauri::WebviewWindowBuilder;

    let builder =
        WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("index.html".into()))
            .title("HoshinekoFM")
            .inner_size(800.0, 600.0);

    let window = builder
        .build()
        .map_err(|e| format!("failed to create window: {e}"))?;

    tracing::info!("created new window: {label}");

    for p in paths {
        let _ = window.emit("navigate-to", p);
    }
    if paths.is_empty() {
        let _ = window.emit("navigate-to", "/");
    }

    Ok(window)
}

/// 创建新窗口（Tauri 命令）。
///
/// 1. AppStateManager 生成 label（"w0", "w1" ...）
/// 2. 创建 Tauri 窗口
/// 3. 统一注册（router + local map）
/// 4. 注册到 AppStateManager
#[command]
pub async fn new_window(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<AppStateManager>>,
    paths: Option<Vec<String>>,
) -> Result<u64, String> {
    let paths = paths.unwrap_or_default();

    let label = mgr.next_label();
    let window = create_window(&app, &label, &paths)?;

    let bus = mgr.inner().register_window(mgr.instance_bus.clone(), window, label).await;
    let window_id = bus.window_id();

    Ok(window_id)
}

// ---------------------------------------------------------------------------
// UIService 命令
// ---------------------------------------------------------------------------

/// 返回初始状态（tab 列表 + 激活 tab + 剪贴板状态）。
#[command]
pub fn init_state(
    ui: State<'_, Arc<UIService>>,
) -> Result<ui_service::InitState, String> {
    Ok(ui.init())
}

/// 尝试移动 tab。如果阻塞，返回 Blocked 让前端弹窗。
#[command]
pub fn move_tab(
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    target: u64,
) -> Result<MoveTabCmdResult, String> {
    match ui.move_tab(tab_id, target) {
        ui_service::MoveTabResult::Ok => Ok(MoveTabCmdResult::Ok),
        ui_service::MoveTabResult::Blocked { tasks } => {
            Ok(MoveTabCmdResult::Blocked {
                tasks: tasks
                    .into_iter()
                    .map(|t| MoveTabTaskInfo {
                        task_id: t.task_id,
                        description: t.description,
                    })
                    .collect(),
            })
        }
    }
}

/// 强制移动 tab（取消关联任务）。
#[command]
pub async fn move_tab_force(
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    target: u64,
) -> Result<(), String> {
    ui.move_tab_force(tab_id, target).await;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub enum MoveTabCmdResult {
    Ok,
    Blocked { tasks: Vec<MoveTabTaskInfo> },
}

#[derive(Debug, Clone, Serialize)]
pub struct MoveTabTaskInfo {
    pub task_id: u64,
    pub description: String,
}
