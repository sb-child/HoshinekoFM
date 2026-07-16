//! Mesh 统一通信层。
//!
//! ## 架构
//!
//! ```text
//! WindowProxy_0 ──┐
//! WindowProxy_1 ──┤── Mesh (路由表) ── InstanceBus ──► 其他实例
//! WindowProxy_2 ──┘
//! ```
//!
//! - 同进程内：通过内存 dispatch 直投到目标 window handler，零序列化
//! - 跨实例：通过 InstanceBus 的 InstanceService tarpc 转发

mod envelope;
mod id;
pub mod proxy;
pub mod server;

pub use envelope::Envelope;
pub use id::{MeshId, ServiceId, WindowId};
pub use proxy::{WindowProxy, dispatch_message};

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use crate::instance_bus::InstanceBus;
use crate::ipc::protocol::{DragOp, TabState, WindowMessage};

// ---------------------------------------------------------------------------
// WindowMessageHandler
// ---------------------------------------------------------------------------

/// 每个窗口的消息处理器。
pub trait WindowMessageHandler: Send + Sync + 'static {
    fn on_dnd_session_active(&self, session_id: u64, files: Vec<String>, operation: DragOp);
    fn on_dnd_session_completed(&self, session_id: u64);
    fn on_tab_attached(&self, tab: TabState);
}

// ---------------------------------------------------------------------------
// Mesh
// ---------------------------------------------------------------------------

/// 实例级 Mesh 路由器。
///
/// 管理窗口中继表和跨实例转发。
#[derive(Clone)]
pub struct Mesh {
    inner: Arc<MeshInner>,
}

struct MeshInner {
    instance_id: u64,
    window_counter: AtomicU64,
    windows: RwLock<std::collections::HashMap<u64, Arc<dyn WindowMessageHandler>>>,
    instance_bus: Arc<InstanceBus>,
}

impl Mesh {
    pub fn new(instance_id: u64, instance_bus: Arc<InstanceBus>) -> Self {
        Self {
            inner: Arc::new(MeshInner {
                instance_id,
                window_counter: AtomicU64::new(0),
                windows: RwLock::new(std::collections::HashMap::new()),
                instance_bus,
            }),
        }
    }

    /// 本实例 ID。
    pub fn instance_id(&self) -> u64 {
        self.inner.instance_id
    }

    /// 注册新窗口，返回其通信句柄。
    pub fn create_window(
        &self,
        handler: Arc<dyn WindowMessageHandler>,
    ) -> WindowProxy {
        let window_id = self.inner.window_counter.fetch_add(1, Ordering::Relaxed);
        self.inner.windows.write().unwrap().insert(window_id, handler.clone());
        proxy::new_proxy(self.clone(), window_id)
    }

    /// 移除窗口。
    pub(super) fn remove_window(&self, window_id: u64) {
        self.inner.windows.write().unwrap().remove(&window_id);
    }

    /// 向指定窗口发送消息。
    pub fn send_to(&self, target_id: u64, msg: WindowMessage) {
        let windows = self.inner.windows.read().unwrap();
        let target_instance = self.inner.instance_bus.window_instance(target_id);

        if target_instance == Some(self.inner.instance_id) {
            if let Some(handler) = windows.get(&target_id) {
                let handler = handler.clone();
                drop(windows);
                dispatch_message(handler.as_ref(), &msg);
            }
        } else if let Some(remote_instance) = target_instance {
            drop(windows);
            let instance_bus = self.inner.instance_bus.clone();
            tokio::spawn(async move {
                use crate::ipc::protocol::InstanceMessage;
                let _ = instance_bus
                    .send_to(
                        remote_instance,
                        &InstanceMessage::ForwardToWindow {
                            window_id: target_id,
                            msg,
                        },
                    )
                    .await;
            });
        }
    }

    /// 向所有本地窗口广播。
    pub fn broadcast_local(&self, msg: WindowMessage) {
        let handlers: Vec<_> = self
            .inner
            .windows
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect();
        for handler in handlers {
            dispatch_message(handler.as_ref(), &msg);
        }
    }

    /// InstanceBus 的引用（用于向后兼容）。
    pub fn instance_bus(&self) -> &Arc<InstanceBus> {
        &self.inner.instance_bus
    }

    /// 分发消息到指定窗口（公开给 InstanceBusHandler 回调使用）。
    pub fn dispatch_to(&self, window_id: u64, msg: &WindowMessage) {
        if let Some(handler) = self.inner.windows.read().unwrap().get(&window_id) {
            dispatch_message(handler.as_ref(), msg);
        }
    }
}
