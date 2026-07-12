//! 实例间/窗口间通信类型和 RPC 接口定义。

use serde::{Deserialize, Serialize};

use super::ui::{ClipboardState, DragOp, TabState};

// ---------------------------------------------------------------------------
// 窗口间消息 / 实例间消息
// ---------------------------------------------------------------------------

/// 窗口间消息 — WindowBus 专用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WindowMessage {
    /// DnD 拖拽开始（广播给同实例所有窗口 + 跨实例转发）
    DndSessionActive {
        session_id: u64,
        files: Vec<String>,
        operation: DragOp,
    },
    /// DnD 放置完成（发回源窗口）
    DndSessionCompleted { session_id: u64 },
    /// Tab 已附加到目标窗口
    TabAttached { tab: TabState },
}

/// 实例间消息 — InstanceBus 专用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstanceMessage {
    /// 窗口注册广播
    WindowRegistered { window_id: u64, instance_id: u64 },
    /// 窗口注销广播
    WindowUnregistered { window_id: u64 },
    /// 剪贴板同步广播
    ClipboardSync { state: ClipboardState },
    /// Tab 转移到指定实例
    TransferTab { tab: TabState, to_instance: u64 },
    /// 转发 WindowMessage 到指定窗口
    ForwardToWindow { window_id: u64, msg: WindowMessage },
}

// ---------------------------------------------------------------------------
// 实例间 RPC: InstanceService
// ---------------------------------------------------------------------------

/// 实例间 RPC 服务。
///
/// Mesh 全连接：每个实例既是 server（监听连接）又是 client（连接其他实例）。
/// 所有方法都是 P2P 直连，不经过中转。
#[tarpc::service]
pub trait InstanceService {
    /// 请求在当前实例打开新窗口（CLI `hnfm /path` 复用已有实例时使用）。
    ///
    /// `paths` 为空时创建空窗口（导航至根目录）。
    async fn open_window(paths: Vec<String>);

    /// 将一个 tab 从其他实例转移到当前实例。
    async fn transfer_tab(tab: TabState);

    /// 窗口注册（fan-out 广播给所有实例）。
    async fn window_register(window_id: u64, instance_id: u64);

    /// 窗口注销（fan-out 广播给所有实例）。
    async fn window_unregister(window_id: u64);

    /// 转发 WindowMessage 到当前实例的指定窗口。
    async fn forward(window_id: u64, msg: WindowMessage);

    /// 剪贴板同步（fan-out 广播给所有实例）。
    async fn clipboard_sync(state: ClipboardState);

    /// 存活检查。
    async fn ping() -> bool;
}
