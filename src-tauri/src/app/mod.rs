//! Tauri 应用主入口。
//!
//! `run_app()`:
//! 1. 在当前 tokio runtime 中做 async 初始化（bind socket + InstanceBus::start）
//! 2. 通过 `tauri::async_runtime::set()` 告知 Tauri 共用同一 runtime
//! 3. 构建 Tauri，程序化创建首窗口，注册命令和状态

pub mod commands;
pub mod fs_service;
pub mod state;
pub mod tabs;
pub mod ui_service;

use crate::lock::LockSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;
use tracing::{error, info, Instrument};

use crate::instance_bus::{self, InstanceBus};
use crate::mesh::Mesh;
use crate::mesh::callback::UiMeshHandler;

use self::{state::AppStateManager, tabs::TabManager};

// --
// RunOpts
// --

pub struct RunOpts {
    pub instance_id: u64,
    pub paths: Vec<String>,
}

// --
// run_app
// --

/// 启动 Tauri 应用（必须在 tokio runtime 内调用）。
pub async fn run_app(opts: RunOpts) {
    let instance_id = opts.instance_id;

    // 1. 绑定本地 socket
    let listener = match crate::mesh::discovery::bind_socket(instance_id) {
        Ok(l) => l,
        Err(e) => {
            error!("failed to bind socket: {e}");
            return;
        }
    };

    // 2. 启动 InstanceBus
    let instance_bus = match InstanceBus::start(instance_id).await {
        Ok(ib) => ib,
        Err(e) => {
            error!("failed to start InstanceBus: {e}");
            return;
        }
    };

    // 2b. 后台监听实例目录变化（双向拓扑）
    let instance_bus_watch = instance_bus.clone();
    tokio::spawn(
        async move {
            instance_bus::watch_instances(instance_bus_watch).await;
        }
        .instrument(tracing::info_span!("watch_instances"))
    );

    // 3. 创建 Mesh（包装 InstanceBus）
    let mesh = Arc::new(Mesh::new(instance_id, instance_bus.clone()));

    // 4. 初始化 TabManager
    let mut tab_manager = TabManager::load_from_disk();
    if !opts.paths.is_empty() {
        for path in &opts.paths {
            tab_manager.add_tab(path.clone());
        }
    }

    // 5. 创建 AppStateManager / UIService
    let mgr = Arc::new(AppStateManager::new(tab_manager, mesh.clone()));
    let ui = Arc::new(ui_service::UIService::new(mgr.clone()));

    // 6. 注册 Mesh handler + 启动 InstanceBus listen
    let ui_handler = Arc::new(UiMeshHandler {
        ui: ui.clone(),
        mesh: mesh.clone(),
    });
    mesh.register_instance_handler(ui_handler);

    let ib_listen = instance_bus.clone();
    let mesh_listen = mesh.clone();
    tokio::spawn(
        async move {
            ib_listen.listen(listener, mesh_listen).await;
        }
        .instrument(tracing::info_span!("instance_bus_listen"))
    );

    // 7. 告知 Tauri 共用当前 tokio runtime
    tauri::async_runtime::set(tokio::runtime::Handle::current());

    // 8. 构建 Tauri
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_win = shutdown.clone();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(mgr.clone())
        .manage(ui.clone())
        .manage(mesh.clone())
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
            commands::import_files,
            commands::realpath,
            commands::get_window_id,
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
    if let Err(e) = builder.run(tauri::generate_context!()) {
        error!("error while running tauri application: {e}");
    }
}

// --
// run_app 辅助函数
// --

/// `.setup()` 闭包的实际逻辑：注入 AppHandle + 创建首窗口。
fn setup_first_window(handle: &tauri::AppHandle, mgr: &Arc<AppStateManager>, paths: &[String]) {
    let label = mgr.next_label();
    match commands::create_window(handle, &label, paths) {
        Ok(window) => {
            let mgr_c = mgr.clone();
            tokio::spawn(
                async move {
                    mgr_c.register_window(window, label).await;
                }
                .instrument(tracing::info_span!("register_window"))
            );
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
    tokio::spawn(
        async move {
            tokio::signal::ctrl_c().await.ok();
            if shutdown.swap(true, Ordering::Relaxed) {
                return;
            }
            info!("saving tabs before exit (ctrl+c)");
            mgr.tabs.lock_safe().save_to_disk();
            let instance_id = mgr.mesh.instance_bus().self_id();
            crate::mesh::discovery::cleanup_socket(instance_id);
            std::process::exit(0);
        }
        .instrument(tracing::info_span!("ctrl_c_handler"))
    );
}

/// `.on_window_event()` 闭包中 Destroyed 事件的实际处理。
fn handle_window_destroyed(mgr: &Arc<AppStateManager>, label: &str, shutdown: &Arc<AtomicBool>) {
    let label = label.to_string();
    let window_proxy = {
        let windows = mgr.windows.lock_safe();
        windows.get(&label).map(|s| s.window_proxy.clone())
    };
    if let Some(proxy) = window_proxy {
        mgr.unregister(&label);
        let mgr = mgr.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(
            async move {
                proxy.unregister();
                let remaining = mgr.registry_count();
                if remaining == 0 && !shutdown.swap(true, Ordering::Relaxed) {
                    info!("last window destroyed, exiting");
                    let instance_id = mgr.mesh.instance_bus().self_id();
                    crate::mesh::discovery::cleanup_socket(instance_id);
                    std::process::exit(0);
                }
            }
            .instrument(tracing::info_span!("window_destroyed"))
        );
    }
}
