//! App 层 Mesh 回调实现 -- UiMeshHandler。
//!
//! 实现 `WindowHandler` 和 `InstanceHandler`，将 Mesh 消息
//! 转换为 UIService 调用。通过 channel 或直接持有 UIService/Mesh 引用完成桥接。
//!
//! 符合 §4 设计：Mesh 只看到 `dyn Handler` trait，不知道 `UIService` 的存在。

use std::sync::Arc;

use crate::app::ui_service::UIService;
use crate::mesh::types::ui::{ClipboardState, DragOp, TabState};
use crate::mesh::types::window::WindowMsg;
use crate::mesh::{InstanceHandler, Mesh, WindowHandler};

/// Mesh 到 UIService 的回调桥接器。
///
/// 实现所有 endpoint handler trait，在 app 层注册到 Mesh。
pub struct UiMeshHandler {
    pub ui: Arc<UIService>,
    pub mesh: Arc<Mesh>,
}

// ── WindowHandler ─────────────────────────────────────────────────────────

impl WindowHandler for UiMeshHandler {
    fn on_dnd_active(&self, session_id: u64, files: Vec<String>, op: DragOp) {
        // DnD 活跃 -- 通过 WindowMsg 广播到所有窗口
        self.mesh.broadcast_windows(WindowMsg::DndSessionActive {
            session_id,
            files,
            operation: op,
        });
    }

    fn on_dnd_completed(&self, session_id: u64) {
        self.mesh
            .broadcast_windows(WindowMsg::DndSessionCompleted { session_id });
    }

    fn on_tab_attached(&self, tab: TabState) {
        // Tab 已附加 -- 前端通过 hf:tabs 事件感知
        let _ = tab;
    }
}

// ── InstanceHandler ───────────────────────────────────────────────────────

impl InstanceHandler for UiMeshHandler {
    fn on_open_window(&self, paths: Vec<String>) {
        let ui = self.ui.clone();
        let handle = tokio::spawn(async move {
            ui.open_window(paths).await;
        });
        tokio::spawn(async move {
            if let Err(e) = handle.await {
                tracing::error!("on_open_window task panicked: {e}");
            }
        });
    }

    fn on_transfer_tab(&self, tab: TabState) {
        let ui = self.ui.clone();
        let handle = tokio::spawn(async move {
            ui.receive_transfer_tab(tab).await;
        });
        tokio::spawn(async move {
            if let Err(e) = handle.await {
                tracing::error!("on_transfer_tab task panicked: {e}");
            }
        });
    }

    fn on_clipboard_sync(&self, state: ClipboardState) {
        self.ui.clipboard_sync(state);
    }

    fn on_forward_window_msg(&self, window_id: u64, msg: WindowMsg) {
        // 跨实例转发 -- 调回 Mesh 本地投递
        self.mesh.dispatch_window_local(window_id, &msg);
    }
}
