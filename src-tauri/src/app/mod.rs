//! Tauri 应用主入口。
//!
//! `run_app()`:
//! 1. 绑定实例 socket
//! 2. 启动 InstanceBus (Mesh 发现 + 连接)
//! 3. 创建 AppStateManager（含 TabManager）
//! 4. 后台监听实例间连接
//! 5. 构建 Tauri，程序化创建首窗口，注册命令和状态

pub mod commands;
pub mod fs_service;
pub mod state;
pub mod tabs;
pub mod ui_service;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Emitter, Manager};
use tokio::{
    net::UnixListener,
    sync::mpsc,
};
use tracing::{debug, error, info, warn};

use crate::{
    instance_bus::{self, InstanceBus},
    ipc::protocol::{ClipboardState, InstanceService, TabState, WindowMessage},
    window_bus::WindowBus,
};

use self::{commands::TabEvent, state::AppStateManager, tabs::TabManager};

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

pub fn run_app(opts: RunOpts) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(run_app_async(opts));
}

async fn run_app_async(opts: RunOpts) {
    let instance_id = opts.instance_id;

    // 1. 绑定本地 socket
    let listener = match instance_bus::bind_socket(instance_id) {
        Ok(l) => l,
        Err(e) => {
            error!("failed to bind socket: {e}");
            return;
        }
    };

    // 2. 启动 InstanceBus（发现 + 连接所有 peer）
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

    // 4. 创建 AppStateManager
    let mgr = Arc::new(AppStateManager::new(tab_manager, instance_bus.clone()));

    // 4b. 创建 UIService（per-instance 协调器）
    let ui = Arc::new(ui_service::UIService::new(mgr.clone()));

    // 5. 创建 tab 事件通道
    let (tab_event_tx, mut tab_event_rx) = mpsc::unbounded_channel::<TabEvent>();
    tokio::spawn(async move {
        while let Some(event) = tab_event_rx.recv().await {
            debug!("tab event: {:?}", event);
        }
    });

    // 6. 后台监听实例间连接
    let ui_for_server = ui.clone();
    tokio::spawn(accept_instance_connections(listener, instance_bus.clone(), ui_for_server));

    // 7. 构建 Tauri
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_win = shutdown.clone();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(mgr.clone())
        .manage(ui.clone())
        .manage(instance_bus.clone())
        .manage(tab_event_tx.clone())
        .invoke_handler(tauri::generate_handler![
            crate::drag::commands::start_drag,
            commands::list_tabs,
            commands::add_tab,
            commands::close_tab,
            commands::tab_event_sink,
            commands::new_window,
            commands::move_tab,
            commands::move_tab_force,
            commands::init_state,
            crate::window_bus::commands::get_window_id,
        ])
        .setup(move |_app| {
            let handle = _app.handle().clone();

            // 程序化创建首窗口
            {
                let mgr = {
                    let state = handle.state::<Arc<AppStateManager>>();
                    state.inner().clone()
                };
                let instance_bus = {
                    let state = handle.state::<Arc<InstanceBus>>();
                    state.inner().clone()
                };

                let label = mgr.next_label();
                match commands::create_window(&handle, &label, &opts.paths) {
                    Ok(window) => {
                        let mgr_clone = mgr.clone();
                        let label_clone = label.clone();
                        tokio::spawn(async move {
                            let bus = WindowBus::init(instance_bus, window, mgr_clone.clone()).await;
                            mgr_clone.register(label_clone, bus);
                        });
                    }
                    Err(e) => {
                        error!("failed to create initial window: {e}");
                    }
                }
            }

            // Ctrl+C — 保存 tabs + 清理 socket（仅一次）
            let mgr_save = mgr.clone();
            let shutdown_ctrlc = shutdown.clone();
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                if shutdown_ctrlc.swap(true, Ordering::Relaxed) {
                    return; // 已经由另一路径触发退出
                }
                info!("saving tabs before exit (ctrl+c)");
                mgr_save.tabs.lock().unwrap().save_to_disk();
                instance_bus::cleanup_socket(mgr_save.instance_bus.self_id());
                std::process::exit(0);
            });

            Ok(())
        })
        .on_window_event(move |window, event| {
            use tauri::WindowEvent;
            if let WindowEvent::Destroyed = event {
                let label = window.label().to_string();
                if let Some(mgr) = window
                    .app_handle()
                    .try_state::<Arc<AppStateManager>>()
                    .map(|s| s.inner().clone())
                {
                    let window_bus = {
                        let windows = mgr.windows.lock().unwrap();
                        windows.get(&label).map(|s| &s.window_bus).cloned()
                    };

                    if let Some(bus) = window_bus {
                        mgr.unregister(&label);

                        let shutdown_destroyed = shutdown_win.clone();
                        tokio::spawn(async move {
                            bus.unregister().await;
                            let remaining = mgr.registry_count();
                            if remaining == 0 {
                                if shutdown_destroyed.swap(true, Ordering::Relaxed) {
                                    return; // 已由 ctrl+c 触发退出
                                }
                                info!("last window destroyed, exiting");
                                instance_bus::cleanup_socket(mgr.instance_bus.self_id());
                                std::process::exit(0);
                            }
                        });
                    }
                }
            }
        });

    // 8. 运行
    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------------------------------------------------------------------------
// 实例间连接处理
// ---------------------------------------------------------------------------

async fn accept_instance_connections(
    listener: UnixListener,
    bus: Arc<InstanceBus>,
    ui: Arc<ui_service::UIService>,
) {
    info!("accepting instance connections...");

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!("instance connection from {addr:?}");
                let bus = bus.clone();
                let ui = ui.clone();
                tokio::spawn(handle_connection(stream, bus, ui));
            }
            Err(e) => {
                error!("instance listener error: {e}");
                break;
            }
        }
    }
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    bus: Arc<InstanceBus>,
    ui: Arc<ui_service::UIService>,
) {
    use futures::prelude::*;
    use tarpc::server::Channel;

    let transport = tarpc::serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );

    let server = MeshServer { bus, ui };

    async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }

    tokio::spawn(
        tarpc::server::BaseChannel::with_defaults(transport)
            .execute(server.serve())
            .for_each(spawn),
    );
}

/// Mesh 对等 InstanceService 实现。
#[derive(Clone)]
struct MeshServer {
    bus: Arc<InstanceBus>,
    ui: Arc<ui_service::UIService>,
}

impl InstanceService for MeshServer {
    async fn window_register(
        self,
        _ctx: tarpc::context::Context,
        window_id: u64,
        instance_id: u64,
    ) {
        debug!("window_register: {window_id} → instance {instance_id}");
        self.bus.upsert_route(window_id, instance_id).await;
    }

    async fn window_unregister(self, _ctx: tarpc::context::Context, window_id: u64) {
        debug!("window_unregister: {window_id}");
        self.bus.remove_route(window_id).await;
    }

    async fn open_tabs(self, _ctx: tarpc::context::Context, paths: Vec<String>) {
        info!("received open_tabs: {paths:?}");
        self.ui.open_tabs(paths);
    }

    async fn transfer_tab(self, _ctx: tarpc::context::Context, tab: TabState) {
        info!("received transfer_tab: id={}", tab.id);
        self.ui.receive_transfer_tab(tab);
    }

    async fn clipboard_sync(
        self,
        _ctx: tarpc::context::Context,
        state: ClipboardState,
    ) {
        debug!("clipboard_sync: {:?}", state.operation);
        self.ui.clipboard_sync(state);
    }

    async fn forward(self, _ctx: tarpc::context::Context, window_id: u64, msg: WindowMessage) {
        debug!("forward: window {window_id}, msg {msg:?}");
        let event = crate::window_bus::msg_to_event(&msg);
        let reg = self.ui.mgr.window_registry.lock().unwrap();
        if let Some(window) = reg.get(&window_id) {
            let _ = window.emit(event, &msg);
        } else {
            warn!("forward: window {window_id} not found locally");
        }
    }

    async fn ping(self, _ctx: tarpc::context::Context) -> bool {
        true
    }
}
