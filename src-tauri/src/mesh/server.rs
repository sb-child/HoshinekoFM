//! Mesh 进程边界：唯一 `#[tarpc::service]` — InstanceService。
//!
//! `InstanceBusServer` 实现 tarpc `InstanceService` trait，
//! `InstanceBusHandler` 供 app 层注册跨实例事件回调。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::{debug, info};

use crate::instance_bus::InstanceBus;
use crate::ipc::protocol::{ClipboardState, InstanceService, TabState, WindowMessage};

/// App 层消息处理器。
///
/// InstanceBus 收到需要 app 层处理的 RPC 时，通过此 trait 回调。
/// `window_register` / `window_unregister` / `ping` 由 `InstanceBus` 内部处理，不经过此 trait。
pub trait InstanceBusHandler: Send + Sync {
    /// 另一个实例请求打开新窗口（CLI `hnfm /path` 复用已有实例）。
    fn on_open_window(&self, paths: Vec<String>) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// 另一个实例转移了一个 tab 到本实例。
    fn on_transfer_tab(&self, tab: TabState) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// 剪贴板状态从另一个实例同步过来。
    fn on_clipboard_sync(&self, state: ClipboardState);

    /// 来自另一个实例的 `WindowMessage`，需要投递到本地指定窗口。
    fn on_forward(&self, window_id: u64, msg: WindowMessage);
}

/// tarpc server 载体，持有 `InstanceBus` 引用 + app 层 handler。
///
/// 由 `InstanceBus::listen()` 内部创建，外部无需直接使用。
#[derive(Clone)]
pub struct InstanceBusServer {
    bus: Arc<InstanceBus>,
    handler: Arc<dyn InstanceBusHandler>,
}

impl InstanceBusServer {
    pub fn new(bus: Arc<InstanceBus>, handler: Arc<dyn InstanceBusHandler>) -> Self {
        Self { bus, handler }
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
        self.bus.upsert_route(window_id, instance_id);
    }

    async fn window_unregister(self, _ctx: tarpc::context::Context, window_id: u64) {
        debug!("window_unregister: {window_id}");
        self.bus.remove_route(window_id);
    }

    async fn ping(self, _ctx: tarpc::context::Context) -> bool {
        true
    }

    // ── 转发给 handler ───────────────────────────────────────────────────

    async fn open_window(self, _ctx: tarpc::context::Context, paths: Vec<String>) {
        info!("received open_window: {paths:?}");
        self.handler.on_open_window(paths).await;
    }

    async fn transfer_tab(self, _ctx: tarpc::context::Context, tab: TabState) {
        info!("received transfer_tab: id={}", tab.id);
        self.handler.on_transfer_tab(tab).await;
    }

    async fn clipboard_sync(self, _ctx: tarpc::context::Context, state: ClipboardState) {
        debug!("clipboard_sync: {:?}", state.operation);
        self.handler.on_clipboard_sync(state);
    }

    async fn forward(self, _ctx: tarpc::context::Context, window_id: u64, msg: WindowMessage) {
        debug!("forward: window {window_id}, msg {msg:?}");
        self.handler.on_forward(window_id, msg);
    }
}
