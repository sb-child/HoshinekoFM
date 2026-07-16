//! Mesh 进程边界：唯一 `#[tarpc::service]` — InstanceService。
//!
//! `InstanceBusServer` 实现 tarpc `InstanceService` trait，
//! 将 RPC 调用转换为 `InstanceMsg` 后通过 `Mesh` 分发。

use std::sync::Arc;

use tracing::{debug, info};

use crate::mesh::types::instance::{InstanceMsg, InstanceService};
use crate::mesh::types::ui::{ClipboardState, TabState};
use crate::mesh::types::window::WindowMsg;

use crate::mesh::Mesh;

/// tarpc server 载体，持有 `Mesh` 引用。
///
/// 收到跨实例 RPC 后转为 `InstanceMsg` 枚举并通过 `Mesh::dispatch_instance` 分发，
/// `InstanceHandler` 由 app 层注册到 Mesh。
#[derive(Clone)]
pub struct InstanceBusServer {
    mesh: Arc<Mesh>,
}

impl InstanceBusServer {
    pub fn new(mesh: Arc<Mesh>) -> Self {
        Self { mesh }
    }
}

impl InstanceService for InstanceBusServer {
    // ── InstanceBus 内部处理 ──────────────────────────────────────────────

    async fn window_register(
        self,
        _ctx: tarpc::context::Context,
        window_id: u64,
        instance_id: u64,
    ) {
        debug!("window_register: {window_id} → instance {instance_id}");
        self.mesh
            .instance_bus()
            .upsert_route(window_id, instance_id);
    }

    async fn window_unregister(self, _ctx: tarpc::context::Context, window_id: u64) {
        debug!("window_unregister: {window_id}");
        self.mesh.instance_bus().remove_route(window_id);
    }

    async fn ping(self, _ctx: tarpc::context::Context) -> bool {
        true
    }

    // ── InstanceMsg 分发 ──────────────────────────────────────────────────

    async fn open_window(self, _ctx: tarpc::context::Context, paths: Vec<String>) {
        info!("received open_window: {paths:?}");
        self.mesh
            .dispatch_instance(&InstanceMsg::OpenWindow { paths });
    }

    async fn transfer_tab(self, _ctx: tarpc::context::Context, tab: TabState) {
        info!("received transfer_tab: id={}", tab.id);
        self.mesh
            .dispatch_instance(&InstanceMsg::TransferTab { tab });
    }

    async fn clipboard_sync(self, _ctx: tarpc::context::Context, state: ClipboardState) {
        debug!("clipboard_sync: {:?}", state.operation);
        self.mesh
            .dispatch_instance(&InstanceMsg::ClipboardSync { state });
    }

    async fn forward(self, _ctx: tarpc::context::Context, window_id: u64, msg: WindowMsg) {
        debug!("forward: window {window_id}, msg {msg:?}");
        self.mesh
            .dispatch_instance(&InstanceMsg::ForwardWindowMsg { window_id, msg });
    }
}
