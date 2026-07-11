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

    // 5. 创建 tab 事件通道
    let (tab_event_tx, mut tab_event_rx) = mpsc::unbounded_channel::<TabEvent>();
    tokio::spawn(async move {
        while let Some(event) = tab_event_rx.recv().await {
            debug!("tab event: {:?}", event);
        }
    });

    // 6. 后台监听实例间连接
    let ui_for_server = ui.clone();
    tokio::spawn(accept_instance_connections(
        listener,
        instance_bus.clone(),
        ui_for_server,
    ));

    // 7. 告知 Tauri 共用当前 tokio runtime
    tauri::async_runtime::set(tokio::runtime::Handle::current());

    // 8. 构建 Tauri
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
                    if let Some(mgr_win) = window
                        .app_handle()
                        .try_state::<Arc<AppStateManager>>()
                        .map(|s| s.inner().clone())
                    {
                        handle_window_destroyed(&mgr_win, window.label(), &shutdown_e);
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
fn setup_first_window(
    handle: &tauri::AppHandle,
    mgr: &Arc<AppStateManager>,
    paths: &[String],
) {
    let ib = handle.state::<Arc<InstanceBus>>().inner().clone();
    let label = mgr.next_label();
    match commands::create_window(handle, &label, paths) {
        Ok(window) => {
            let (tx, rx) = std::sync::mpsc::channel();
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
fn handle_window_destroyed(
    mgr: &Arc<AppStateManager>,
    label: &str,
    shutdown: &Arc<AtomicBool>,
) {
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
        self.bus.upsert_route(window_id, instance_id);
    }

    async fn window_unregister(self, _ctx: tarpc::context::Context, window_id: u64) {
        debug!("window_unregister: {window_id}");
        self.bus.remove_route(window_id);
    }

    async fn open_window(self, _ctx: tarpc::context::Context, paths: Vec<String>) {
        info!("received open_window: {paths:?}");
        self.ui.open_window(paths).await;
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

    async fn forward(
        self,
        _ctx: tarpc::context::Context,
        window_id: u64,
        msg: WindowMessage,
    ) {
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
