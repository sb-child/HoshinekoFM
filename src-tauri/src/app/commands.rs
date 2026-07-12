//! Tauri Commands —— 前端可调用的后端函数。
//!
//! 通过 `invoke()` 调用。前端只发意图，结果通过 event 推送。

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::{command, State, Window};

use crate::app::state::AppStateManager;
use crate::app::ui_service::{self, UIService};
use crate::ipc::protocol::{ContextId, EntryKind, NavTarget};

// ---------------------------------------------------------------------------
// 多窗口命令
// ---------------------------------------------------------------------------

/// 创建新窗口（使用给定 label）。
pub fn create_window(
    app: &tauri::AppHandle,
    label: &str,
    _paths: &[String],
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

    Ok(window)
}

/// 创建新窗口（Tauri 命令）。
#[command]
pub async fn new_window(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<AppStateManager>>,
    paths: Option<Vec<String>>,
) -> Result<u64, String> {
    let paths = paths.unwrap_or_default();

    let label = mgr.next_label();
    let window = create_window(&app, &label, &paths)?;

    let bus = mgr
        .inner()
        .register_window(mgr.instance_bus.clone(), window, label)
        .await;
    let window_id = bus.window_id();

    Ok(window_id)
}

// ---------------------------------------------------------------------------
// Ready — 前端就绪
// ---------------------------------------------------------------------------

/// 前端 mount 后调用。后端重建运行时状态 + 推送初始事件。
#[command]
pub async fn ready(
    window: Window,
    ui: State<'_, Arc<UIService>>,
) -> Result<(), String> {
    ui.ready(&window).await
}

// ---------------------------------------------------------------------------
// UIService Tab 生命周期命令
// ---------------------------------------------------------------------------

/// 获取所有 tab 列表。
#[command]
pub fn list_tabs(
    mgr: State<'_, Arc<AppStateManager>>,
) -> Result<Vec<crate::ipc::protocol::TabState>, String> {
    let tabs = mgr.tabs.lock().unwrap();
    Ok(tabs.get_all().to_vec())
}

/// 切换到指定 tab。
#[command]
pub async fn switch_tab(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
) -> Result<(), String> {
    ui.switch_tab(&window, tab_id).await
}

/// 创建新 tab（自动切换为活跃）。
#[command]
pub async fn new_tab(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    path: Option<String>,
) -> Result<u64, String> {
    ui.new_tab(&window, path).await
}

/// 关闭 tab。
#[command]
pub fn close_tab(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
) -> Result<(), String> {
    ui.close_tab(&window, tab_id);
    Ok(())
}

/// 尝试移动 tab，若阻塞返回 Blocked。
#[command]
pub fn move_tab(
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    target: u64,
) -> Result<MoveTabCmdResult, String> {
    match ui.move_tab(tab_id, target) {
        ui_service::MoveTabResult::Ok => Ok(MoveTabCmdResult::Ok),
        ui_service::MoveTabResult::Blocked { tasks } => Ok(MoveTabCmdResult::Blocked {
            tasks: tasks
                .into_iter()
                .map(|t| MoveTabTaskInfo {
                    task_id: t.task_id,
                    description: t.description,
                })
                .collect(),
        }),
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

// ---------------------------------------------------------------------------
// UIService 导航命令
// ---------------------------------------------------------------------------

/// 导航到指定目标。
#[command]
pub async fn nav_to(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    target: NavTarget,
) -> Result<(), String> {
    ui.nav_to(&window, tab_id, target).await
}

/// 导航后退。
#[command]
pub async fn nav_back(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
) -> Result<(), String> {
    ui.nav_back(&window, tab_id).await
}

/// 导航前进。
#[command]
pub async fn nav_forward(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
) -> Result<(), String> {
    ui.nav_forward(&window, tab_id).await
}

/// 更新选中文件，emit hf:selection。
#[command]
pub fn select_files(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    selected: Vec<String>,
) -> Result<(), String> {
    ui.update_selection(&window, tab_id, selected);
    Ok(())
}

// ---------------------------------------------------------------------------
// UIService 文件操作命令
// ---------------------------------------------------------------------------

/// 创建文件或目录。
#[command]
pub async fn create_entry(
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    path: String,
    kind: EntryKind,
) -> Result<(), String> {
    let ctx = ContextId::Tab(tab_id);
    ui.create(tab_id, Path::new(&path), kind, ctx).await
}

/// 重命名文件或目录。
#[command]
pub async fn rename_entry(
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    path: String,
    new_name: String,
) -> Result<(), String> {
    let ctx = ContextId::Tab(tab_id);
    ui.rename(tab_id, Path::new(&path), &new_name, ctx).await
}

// ---------------------------------------------------------------------------
// UIService 权限变更命令
// ---------------------------------------------------------------------------

/// 将 tab 权限切换到指定 uid。
#[command]
pub async fn elevate_tab(
    window: Window,
    ui: State<'_, Arc<UIService>>,
    tab_id: u64,
    target_uid: u32,
) -> Result<(), String> {
    ui.elevate_tab(&window, tab_id, target_uid).await
}
