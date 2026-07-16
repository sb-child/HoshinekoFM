//! Window 端点：窗口间消息与 Handler trait。
//!
//! `WindowMsg` 是窗口间（同实例或跨实例）通信的可序列化消息。
//! `WindowHandler` 是接收窗口消息的 trait。任何想接收窗口消息的
//! 端点实现此 trait 并注册到 Mesh 即可。

use serde::{Deserialize, Serialize};

use super::ui::{DragOp, TabState};

// ---------------------------------------------------------------------------
// WindowMsg
// ---------------------------------------------------------------------------

/// 窗口间消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WindowMsg {
    DndSessionActive {
        session_id: u64,
        files: Vec<String>,
        operation: DragOp,
    },
    DndSessionCompleted {
        session_id: u64,
    },
    TabAttached {
        tab: TabState,
    },
}

// ---------------------------------------------------------------------------
// WindowHandler
// ---------------------------------------------------------------------------

/// 窗口消息处理器。
///
/// 实现此 trait 的端点可以接收其他窗口发来的 `WindowMsg`。
/// 通常由 App 层窗口 handler 实现，注册到 Mesh 的窗口路由中。
pub trait WindowHandler: Send + Sync + 'static {
    fn on_dnd_active(&self, session_id: u64, files: Vec<String>, op: DragOp);
    fn on_dnd_completed(&self, session_id: u64);
    fn on_tab_attached(&self, tab: TabState);
}

// ---------------------------------------------------------------------------
// dispatch — 由宏生成
// ---------------------------------------------------------------------------

crate::endpoint_dispatch!(
    /// 分发 `WindowMsg` 到 `WindowHandler`。
    WindowMsg -> WindowHandler,
    dispatch: dispatch_window_msg,
    DndSessionActive { session_id, files, operation } => on_dnd_active,
    DndSessionCompleted { session_id } => on_dnd_completed,
    TabAttached { tab } => on_tab_attached,
);
