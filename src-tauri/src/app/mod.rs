//! Tauri 应用主入口。
//!
//! `run_app()`:
//! 1. 在当前 tokio runtime 中做 async 初始化（bind socket + InstanceBus::start）
//! 2. 通过 `tauri::async_runtime::set()` 告知 Tauri 共用同一 runtime
//! 3. 构建 Tauri，程序化创建首窗口，注册命令和状态

pub mod commands;
pub mod fs_service;
pub mod mesh_server;
pub mod state;
pub mod tabs;
pub mod ui_service;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;
use tracing::{error, info};

use crate::instance_bus::{self, InstanceBus};

use self::{state::AppStateManager, tabs::TabManager};

// ---------------------------------------------------------------------------
// RunOpts
// ---------------------------------------------------------------------------

pub struct RunOpts {
    pub instance_id: u64,
    pub paths: Vec<String>,
}

// ---------------------------------------------------------------------------
// run_app
// ---------------------------------------------------------------------------

/// 启动 Tauri 应用（必须在 tokio runtime 内调用）。
pub async fn run_app(opts: RunOpts) {
    let instance_id = opts.instance_id;

    // 1. 绑定本地 socket
    let listener = match instance_bus::bind_socket(instance_id) {
        Ok(l) => l,
        Err(e) => {
            error!("failed to bind socket: {e}");
            return;
        }
    };

    // 2. 启动 InstanceBus（不扫描 peer，由 watch_instances 统一发现）
    let instance_bus = InstanceBus::start(instance_id)
        .await
        .expect("failed to start InstanceBus");

    // 2b. 后台监听实例目录变化（双向拓扑）
    let instance_bus_watch = instance_bus.clone();
    tokio::spawn(async move {
        instance_bus::watch_instances(instance_bus_watch).await;
    });

    // 3. 初始化 TabManager
    let mut tab_manager = TabManager::load_from_disk();
    if !opts.paths.is_empty() {
        for path in &opts.paths {
            tab_manager.add_tab(path.clone());
        }
    }

    // 4. 创建 AppStateManager / UIService
    let mgr = Arc::new(AppStateManager::new(tab_manager, instance_bus.clone()));
    let ui = Arc::new(ui_service::UIService::new(mgr.clone()));

    // 5. 后台监听实例间连接
    let ui_for_server = ui.clone();
    tokio::spawn(mesh_server::accept_instance_connections(
        listener,
        instance_bus.clone(),
        ui_for_server,
    ));

    // 6. 告知 Tauri 共用当前 tokio runtime
    tauri::async_runtime::set(tokio::runtime::Handle::current());

    // 7. 构建 Tauri
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_win = shutdown.clone();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(mgr.clone())
        .manage(ui.clone())
        .manage(instance_bus.clone())
        .invoke_handler(tauri::generate_handler![
            crate::drag::commands::start_drag,
            commands::ready,
            commands::list_tabs,
            commands::new_tab,
            commands::close_tab,
            commands::switch_tab,
            commands::new_window,
            commands::move_tab,
            commands::move_tab_force,
            commands::nav_to,
            commands::nav_back,
            commands::nav_forward,
            commands::select_files,
            commands::refresh_tab,
            commands::create_entry,
            commands::rename_entry,
            commands::move_files,
            commands::copy_files,
            commands::elevate_tab,
            crate::window_bus::commands::get_window_id,
        ])
        .setup({
            let mgr_s = mgr.clone();
            let paths_s = opts.paths.clone();
            let shutdown_s = shutdown.clone();
            move |_app| {
                let handle = _app.handle().clone();
                mgr_s.set_app_handle(handle.clone());
                setup_first_window(&handle, &mgr_s, &paths_s);
                spawn_ctrlc_handler(&mgr_s, &shutdown_s);
                Ok(())
            }
        })
        .on_window_event({
            let shutdown_e = shutdown_win.clone();
            move |window, event| {
                if let tauri::WindowEvent::Destroyed = event {
                    let app = window.app_handle();
                    if let Some(mgr_win) = app
                        .try_state::<Arc<AppStateManager>>()
                        .map(|s| s.inner().clone())
                    {
                        handle_window_destroyed(&mgr_win, window.label(), &shutdown_e);
                    }
                    if let Some(ui) = app
                        .try_state::<Arc<ui_service::UIService>>()
                        .map(|s| s.inner().clone())
                    {
                        ui.shutdown_watch(window.label());
                    }
                }
            }
        });

    // 9. 运行（同步阻塞，共用我们的 tokio runtime）
    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------------------------------------------------------------------------
// run_app 辅助函数
// ---------------------------------------------------------------------------

/// `.setup()` 闭包的实际逻辑：注入 AppHandle + 创建首窗口。
fn setup_first_window(handle: &tauri::AppHandle, mgr: &Arc<AppStateManager>, paths: &[String]) {
    let ib = handle.state::<Arc<InstanceBus>>().inner().clone();
    let label = mgr.next_label();
    match commands::create_window(handle, &label, paths) {
        Ok(window) => {
            let (tx, rx) = crate::channel::oneshot::oneshot();
            let mgr_c = mgr.clone();
            tokio::spawn(async move {
                let bus = mgr_c.register_window(ib, window, label).await;
                let _ = tx.send(bus);
            });
            rx.recv().expect("register_window dropped sender");
        }
        Err(e) => {
            error!("failed to create initial window: {e}");
        }
    }
}

/// `.setup()` 闭包的另一部分：Ctrl+C 保存退出 handler。
fn spawn_ctrlc_handler(mgr: &Arc<AppStateManager>, shutdown: &Arc<AtomicBool>) {
    let mgr = mgr.clone();
    let shutdown = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        if shutdown.swap(true, Ordering::Relaxed) {
            return;
        }
        info!("saving tabs before exit (ctrl+c)");
        mgr.tabs.lock().unwrap().save_to_disk();
        instance_bus::cleanup_socket(mgr.instance_bus.self_id());
        std::process::exit(0);
    });
}

/// `.on_window_event()` 闭包中 Destroyed 事件的实际处理。
fn handle_window_destroyed(mgr: &Arc<AppStateManager>, label: &str, shutdown: &Arc<AtomicBool>) {
    let label = label.to_string();
    let window_bus = {
        let windows = mgr.windows.lock().unwrap();
        windows.get(&label).map(|s| s.window_bus.clone())
    };
    if let Some(bus) = window_bus {
        mgr.unregister(&label);
        let mgr = mgr.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            bus.unregister().await;
            let remaining = mgr.registry_count();
            if remaining == 0 && !shutdown.swap(true, Ordering::Relaxed) {
                info!("last window destroyed, exiting");
                instance_bus::cleanup_socket(mgr.instance_bus.self_id());
                std::process::exit(0);
            }
        });
    }
}
