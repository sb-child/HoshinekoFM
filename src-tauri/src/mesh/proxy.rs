//! WindowProxy — 窗口通信句柄。替代旧的 `WindowBus`。

use std::sync::Arc;

use crate::mesh::types::window::WindowMsg;

use super::Mesh;

/// 窗口的通信句柄。
#[derive(Clone)]
pub struct WindowProxy {
    inner: Arc<Inner>,
    window_id: u64,
}

struct Inner {
    mesh: Mesh,
}

impl WindowProxy {
    /// 本窗口的全局 ID。
    pub fn window_id(&self) -> u64 {
        self.window_id
    }

    /// 发消息给指定窗口（Mesh 自动判断本地/远程）。
    pub fn send_to(&self, target_window_id: u64, msg: WindowMsg) {
        self.inner.mesh.send_to_window(target_window_id, msg);
    }

    /// 广播消息给所有本地窗口。
    pub fn broadcast_local(&self, msg: WindowMsg) {
        self.inner.mesh.broadcast_windows(msg);
    }

    /// 注销窗口。
    pub fn unregister(&self) {
        self.inner.mesh.remove_window(self.window_id);
    }
}

pub(super) fn new_proxy(mesh: Mesh, window_id: u64) -> WindowProxy {
    WindowProxy {
        inner: Arc::new(Inner { mesh }),
        window_id,
    }
}
