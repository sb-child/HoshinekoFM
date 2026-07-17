//! Mesh 统一通信层 -- 端点导向路由。
//!
//! ## 架构
//!
//! ```text
//! Window_0 ──┐
//! Window_1 ──┤── Mesh (路由表) ── InstanceBus ──► 其他实例
//! FsService ─┘           │
//!                        ├─ 同进程: trait 直调，零序列化
//!                        └─ 跨实例: InstanceMsg::ForwardWindowMsg -> tarpc
//! ```
//!
//! ## 端点类型
//!
//! 每种端点（Window / Instance / FsService）有独立的:
//! - 消息枚举（`XxxMsg`）
//! - Handler trait（`XxxHandler`）
//! - dispatch 函数（由 `endpoint_dispatch!` 宏生成）
//!
//! 新增端点只需: 建 `types/new_endpoint.rs` -> 在 `MeshInner` 加一个 `HashMap`
//! -> 加 `send_to_xxx`/`register_xxx` 方法。不需要改已有端点文件。
//!
//! ## 跨实例 WindowMsg 透明路由
//!
//! ```text
//! mesh.send_to_window(id, msg)
//!   ├─ 本地 -> dispatch_window_msg(handler, &msg)
//!   └─ 远程 -> InstanceMsg::ForwardWindowMsg { window_id: id, msg }
//!              -> instance_bus.send_to(remote) -> tarpc
//!                -> mesh/server.rs -> handler.on_forward_window_msg(id, msg)
//!                  -> mesh.send_to_window(id, msg)  # 最终本地投递
//! ```

pub mod callback;
pub mod discovery;
mod envelope;
mod id;
pub mod proxy;
pub mod server;
pub mod transport;
pub mod types;

#[macro_use]
mod endpoint_dispatch;

pub use envelope::Envelope;
pub use id::{MeshId, ServiceId, WindowId};
pub use proxy::WindowProxy;
pub use types::{
    FsServiceHandler, FsServiceMsg, InstanceHandler, InstanceMsg, WindowHandler, WindowMsg,
    dispatch_fs_msg, dispatch_instance_msg, dispatch_window_msg,
};

use tracing::Instrument;

use crate::lock::{ReadSafe, WriteSafe};
use std::collections::HashMap;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use crate::instance_bus::InstanceBus;

// --
// Mesh
// --

/// 实例级 Mesh 路由器。
///
/// 管理端点注册表和跨实例转发。每个端点类型有独立的 handler HashMap。
#[derive(Clone)]
pub struct Mesh {
    inner: Arc<MeshInner>,
}

struct MeshInner {
    instance_id: u64,
    window_counter: AtomicU64,

    /// window_id -> handler（窗口端点）
    window_handlers: RwLock<HashMap<u64, Arc<dyn WindowHandler>>>,
    /// 全局实例级 handler（app 层注册，唯一）
    instance_handler: RwLock<Option<Arc<dyn InstanceHandler>>>,

    instance_bus: Arc<InstanceBus>,
}

impl Mesh {
    pub fn new(instance_id: u64, instance_bus: Arc<InstanceBus>) -> Self {
        Self {
            inner: Arc::new(MeshInner {
                instance_id,
                window_counter: AtomicU64::new(0),
                window_handlers: RwLock::new(HashMap::new()),
                instance_handler: RwLock::new(None),
                instance_bus,
            }),
        }
    }

    // ── Identity ──────────────────────────────────────────────────────────

    pub fn instance_id(&self) -> u64 {
        self.inner.instance_id
    }

    /// InstanceBus 的引用（向后兼容）。
    pub fn instance_bus(&self) -> &Arc<InstanceBus> {
        &self.inner.instance_bus
    }

    // ── Window 端点 ───────────────────────────────────────────────────────

    /// 注册新窗口端点，返回其通信句柄。
    pub fn create_window(&self, handler: Arc<dyn WindowHandler>) -> WindowProxy {
        let window_id = self.inner.window_counter.fetch_add(1, Ordering::Relaxed);
        self.inner
            .window_handlers
            .write_safe()
            .insert(window_id, handler);
        proxy::new_proxy(self.clone(), window_id)
    }

    /// 移除窗口端点。
    pub(super) fn remove_window(&self, window_id: u64) {
        self.inner
            .window_handlers
            .write_safe()
            .remove(&window_id);
    }

    /// 向指定窗口发送消息（自动判断本地/远程）。
    pub fn send_to_window(&self, target_id: u64, msg: WindowMsg) {
        let target_instance = self.inner.instance_bus.window_instance(target_id);

        if target_instance == Some(self.inner.instance_id) {
            // 本地直投
            if let Some(handler) = self.inner.window_handlers.read_safe().get(&target_id) {
                let handler = handler.clone();
                dispatch_window_msg(&msg, handler.as_ref());
            }
        } else if let Some(remote_instance) = target_instance {
            // 跨实例：包装成 InstanceMsg::ForwardWindowMsg
            let instance_bus = self.inner.instance_bus.clone();
            let instance_msg = InstanceMsg::ForwardWindowMsg {
                window_id: target_id,
                msg,
            };
            let h = tokio::spawn(async move {
                let _ = instance_bus.send_to(remote_instance, &instance_msg).await;
            }.instrument(tracing::info_span!("mesh::send_to_window")));
            tokio::spawn(async move {
                if let Err(e) = h.await {
                    tracing::error!(target_id, "send_to_window task panicked: {e}");
                }
            }.instrument(tracing::info_span!("mesh::send_to_window_monitor")));
        }
    }

    /// 向所有本地窗口广播。
    pub fn broadcast_windows(&self, msg: WindowMsg) {
        let handlers: Vec<_> = self
            .inner
            .window_handlers
            .read_safe()
            .values()
            .cloned()
            .collect();
        for handler in handlers {
            dispatch_window_msg(&msg, handler.as_ref());
        }
    }

    /// 分发消息到指定本地窗口（供跨实例转发使用）。
    pub fn dispatch_window_local(&self, window_id: u64, msg: &WindowMsg) {
        if let Some(handler) = self.inner.window_handlers.read_safe().get(&window_id) {
            dispatch_window_msg(msg, handler.as_ref());
        }
    }

    // ── Instance 端点 ─────────────────────────────────────────────────────

    /// 注册全局 Instance handler（app 层调用，唯一）。
    pub fn register_instance_handler(&self, handler: Arc<dyn InstanceHandler>) {
        *self.inner.instance_handler.write_safe() = Some(handler);
    }

    /// 向远程实例发送 InstanceMsg。
    pub fn send_to_instance(&self, instance_id: u64, msg: InstanceMsg) {
        let instance_bus = self.inner.instance_bus.clone();
        let h = tokio::spawn(async move {
            let _ = instance_bus.send_to(instance_id, &msg).await;
        }.instrument(tracing::info_span!("mesh::send_to_instance")));
        tokio::spawn(async move {
            if let Err(e) = h.await {
                tracing::error!(instance_id, "send_to_instance task panicked: {e}");
            }
        }.instrument(tracing::info_span!("mesh::send_to_instance_monitor")));
    }

    /// 分发 InstanceMsg 到已注册的 InstanceHandler（供 mesh/server.rs 回调）。
    pub fn dispatch_instance(&self, msg: &InstanceMsg) {
        if let Some(handler) = self.inner.instance_handler.read_safe().as_ref() {
            dispatch_instance_msg(msg, handler.as_ref());
        }
    }
}
