//! Instance 端点：跨实例消息与 Handler trait。
//!
//! `InstanceMsg` 是跨实例通信的可序列化消息（经 tarpc UDS 传输）。
//! `InstanceHandler` 是接收实例消息的 trait。任何想接收来自其他实例
//! 的消息的端点实现此 trait 并注册到 Mesh 即可。
//!
//! `InstanceMsg::ForwardWindowMsg` 是 `WindowMsg` 跨实例传输的桥接变体。

use serde::{Deserialize, Serialize};

use super::ui::{ClipboardState, TabState};
use super::window::WindowMsg;

// --
// InstanceMsg
// --

/// 跨实例消息（唯一需经 tarpc 序列化的消息类型）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstanceMsg {
    OpenWindow {
        paths: Vec<String>,
    },
    TransferTab {
        tab: TabState,
    },
    ClipboardSync {
        state: ClipboardState,
    },
    /// `WindowMsg` 跨实例转发桥接。
    ForwardWindowMsg {
        window_id: u64,
        msg: WindowMsg,
    },
}

// --
// InstanceHandler
// --

/// 实例消息处理器。
///
/// 实现此 trait 的端点可以接收来自其他实例的 `InstanceMsg`。
/// 通常由 App 层 handler（如 `UiMeshHandler`）实现，注册到 Mesh。
pub trait InstanceHandler: Send + Sync + 'static {
    fn on_open_window(&self, paths: Vec<String>);
    fn on_transfer_tab(&self, tab: TabState);
    fn on_clipboard_sync(&self, state: ClipboardState);
    fn on_forward_window_msg(&self, window_id: u64, msg: WindowMsg);
}

// --
// dispatch -- 由宏生成
// --

crate::endpoint_dispatch!(
    /// 分发 `InstanceMsg` 到 `InstanceHandler`。
    InstanceMsg -> InstanceHandler,
    dispatch: dispatch_instance_msg,
    OpenWindow { paths } => on_open_window,
    TransferTab { tab } => on_transfer_tab,
    ClipboardSync { state } => on_clipboard_sync,
    ForwardWindowMsg { window_id, msg } => on_forward_window_msg,
);

// --
// tarpc InstanceService -- 进程边界 RPC
// --

/// 实例间 RPC 服务。
///
/// Mesh 全连接：每个实例既是 server（监听连接）又是 client（连接其他实例）。
/// 所有方法都是 P2P 直连，不经过中转。
///
/// 这是整个项目中**唯一**的 `#[tarpc::service]`，定义了进程边界的传输接口。
/// 所有同进程内通信走纯 Rust trait（`WindowHandler` / `InstanceHandler` / `FsServiceHandler`）。
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

    /// 转发跨实例消息到当前实例的指定窗口。
    async fn forward(window_id: u64, msg: WindowMsg);

    /// 剪贴板同步（fan-out 广播给所有实例）。
    async fn clipboard_sync(state: ClipboardState);

    /// 存活检查。
    async fn ping() -> bool;
}
