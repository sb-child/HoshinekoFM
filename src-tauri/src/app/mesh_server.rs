//! Mesh 对等 InstanceService 实现。
//!
//! 处理实例间连接（accept loop）并提供 MeshServer 的 tarpc 实现。

use std::sync::Arc;

use futures::prelude::*;
use tarpc::server::Channel;
use tauri::Emitter;
use tokio::net::UnixListener;
use tracing::{debug, error, info, warn};

use crate::{
    app::ui_service,
    instance_bus::InstanceBus,
    ipc::protocol::{ClipboardState, InstanceService, TabState, WindowMessage},
};

/// 后台接受实例间连接并派发到 MeshServer。
pub async fn accept_instance_connections(
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
    let transport = tarpc::serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );

    let server = MeshServer { bus, ui };

    async fn spawn(fut: impl std::future::Future<Output = ()> + Send + 'static) {
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
        self.ui.receive_transfer_tab(tab).await;
    }

    async fn clipboard_sync(self, _ctx: tarpc::context::Context, state: ClipboardState) {
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
