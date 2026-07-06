//! Tauri 应用主入口。
//!
//! `run_app()`:
//! 1. 初始化 AppState (TabManager + FsWorkerPool)
//! 2. 如果是 primary 实例，启动实例间连接监听
//! 3. 构建 Tauri，注册命令和状态

pub mod commands;
pub mod state;
pub mod tabs;

use std::sync::Arc;

use tokio::{
    net::UnixListener,
    sync::{mpsc, Mutex},
};
use tracing::{debug, error, info, warn};

use crate::{
    appreuse,
    drag,
    ipc::protocol::{InstanceService, TabState},
};

use self::{commands::TabEvent, state::AppState, tabs::TabManager};

// ---------------------------------------------------------------------------
// 窗口计数器（最后窗口关闭时退出应用）
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};
pub(crate) static WINDOW_COUNT: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// RunOpts
// ---------------------------------------------------------------------------

/// 传递给 `run_app()` 的启动选项。
pub struct RunOpts {
    /// 是否为 primary 实例
    pub is_primary: bool,
    /// 当前实例 ID
    pub instance_id: u64,
    /// 命令行传入的初始路径
    pub paths: Vec<String>,
}

// ---------------------------------------------------------------------------
// run_app
// ---------------------------------------------------------------------------

/// 启动 Tauri 应用。
///
/// 此函数不会返回（Tauri 接管主循环）。
pub fn run_app(opts: RunOpts) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    rt.block_on(async move {
        // 1. 初始化 tab 管理器（从磁盘恢复）
        let mut tab_manager = TabManager::load_from_disk();

        // 如果命令行传了初始路径，添加初始 tab
        if !opts.paths.is_empty() {
            for path in &opts.paths {
                tab_manager.add_tab(path.clone());
            }
        }

        let is_primary = opts.is_primary;
        let instance_id = opts.instance_id;

        // 2. 创建 AppState
        let app_state = AppState::new(is_primary, instance_id, tab_manager);

        // 3. 创建通道：用于后台任务与 Tauri commands 之间的消息传递
        let (tab_event_tx, mut tab_event_rx) = mpsc::unbounded_channel::<TabEvent>();

        // 后台任务：处理 TabEvent（forward 到前端 or 日志）
        tokio::spawn(async move {
            while let Some(event) = tab_event_rx.recv().await {
                debug!("tab event: {:?}", event);
            }
        });

        let state_for_setup = Arc::new(Mutex::new(app_state));

        // 4. 如果是 primary 实例，启动实例间连接监听
        let instance_listener = if is_primary {
            info!("starting primary listener for instance {instance_id}");
            match appreuse::start_primary_listener(instance_id).await {
                Ok(listener) => Some(listener),
                Err(e) => {
                    error!("failed to start primary listener: {e}");
                    None
                }
            }
        } else {
            warn!("running as secondary instance (not primary)");
            None
        };

        // 5. 构建 Tauri
        let builder = tauri::Builder::default()
            .plugin(tauri_plugin_opener::init())
            .manage(state_for_setup.clone())
            .manage(tab_event_tx.clone())
            .invoke_handler(tauri::generate_handler![
                drag::commands::start_drag,
                commands::list_tabs,
                commands::add_tab,
                commands::close_tab,
                commands::tab_event_sink,
                commands::new_window,
                commands::get_window_label,
            ])
            .setup(move |_app| {
                let handle = _app.handle().clone();

                // 初始窗口计数：默认由 tauri.conf.json 创建的主窗口
                WINDOW_COUNT.store(1, Ordering::SeqCst);

                // 后台任务 1：实例间连接监听（仅 primary）
                if let Some(listener) = instance_listener {
                    tokio::spawn(accept_instance_connections(listener, handle.clone()));
                }

                // 后台任务 2：保存 tabs 到磁盘（退出时）
                let state = state_for_setup.clone();
                let is_primary_for_exit = is_primary;
                let instance_id_for_exit = instance_id;
                tokio::spawn(async move {
                    tokio::signal::ctrl_c().await.ok();
                    info!("saving tabs before exit");
                    let app_state = state.lock().await;
                    app_state.tabs.lock().await.save_to_disk();
                    if is_primary_for_exit {
                        appreuse::release_primary(instance_id_for_exit);
                    }
                    std::process::exit(0);
                });

                Ok(())
            })
            .on_window_event({
                let is_primary = is_primary;
                let instance_id = instance_id;
                move |window, event| {
                use tauri::WindowEvent;
                let label = window.label().to_string();
                match event {
                    WindowEvent::Destroyed => {
                        debug!("window destroyed: {label}");
                        if WINDOW_COUNT.fetch_sub(1, Ordering::SeqCst) == 1 {
                            info!("last window destroyed, exiting");
                            if is_primary {
                                appreuse::release_primary(instance_id);
                            }
                            std::process::exit(0);
                        }
                    }
                    _ => {}
                }
            }});

        // 6. 运行
        builder
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    });
}

// ---------------------------------------------------------------------------
// 后台任务
// ---------------------------------------------------------------------------

/// 接受实例间连接。
async fn accept_instance_connections(listener: UnixListener, app_handle: tauri::AppHandle) {
    info!("accepting instance connections...");
    let server = InstanceServer {
        app_handle: app_handle.clone(),
    };
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!("instance connection from {addr:?}");
                let server = server.clone();
                tokio::spawn(handle_instance_connection(stream, server));
            }
            Err(e) => {
                error!("instance listener error: {e}");
                break;
            }
        }
    }
}

/// 处理一个实例间连接。
///
/// 使用 `LengthDelimitedCodec` + `Bincode` 传输层接收 tarpc RPC 请求，
/// 由 `InstanceServer` 处理。
async fn handle_instance_connection(stream: tokio::net::UnixStream, server: InstanceServer) {
    use tarpc::server::{BaseChannel, Channel};
    use futures::prelude::*;

    let transport = tarpc::serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );

    async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }

    tokio::spawn(
        BaseChannel::with_defaults(transport)
            .execute(server.serve())
            .for_each(spawn),
    );
}

/// InstanceService server 实现。
///
/// 持有 `AppHandle` 以创建新窗口和操作 tab。
#[derive(Clone)]
struct InstanceServer {
    app_handle: tauri::AppHandle,
}

impl InstanceService for InstanceServer {
    async fn open_tabs(
        self,
        _ctx: tarpc::context::Context,
        paths: Vec<String>,
    ) {
        info!("received open_tabs request (new window): {paths:?}");
        match commands::create_window(&self.app_handle, paths) {
            Ok(label) => info!("opened new window: {label}"),
            Err(e) => tracing::error!("failed to open window: {e}"),
        }
    }

    async fn transfer_tab(
        self,
        _ctx: tarpc::context::Context,
        tab: TabState,
    ) {
        info!("received transfer_tab request: id={}", tab.id);
        // TODO: 将 tab 加入当前实例的 TabManager，然后创建新窗口加载此 tab
        // 实现步骤：
        //   1. 通过 AppState 获取 TabManager，调用 tabs.transfer_in(tab)
        //   2. create_window(&self.app_handle, vec![tab.path.to_string_lossy().to_string()])
    }

    async fn ping(self, _ctx: tarpc::context::Context) -> bool {
        true
    }
}
