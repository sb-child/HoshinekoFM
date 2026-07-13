//! Per-window 窗口通信总线。
//!
//! 每个窗口持有一个 `WindowBus` 实例，通过两对 `tokio::io::duplex` pair 与 `BusMaster` 通信：
//!
//! ```text
//! WindowBus_0 ──duplex pair────┐
//! WindowBus_1 ──duplex pair────┤── BusMaster ──InstanceBus──→ 其他实例
//! WindowBus_2 ──duplex pair────┘
//! ```
//!
//! - `BusMasterService` (窗→主): WindowBus 调用 BusMaster 发送/广播消息
//! - `WindowMessageReceiver` (主→窗): BusMaster 推送消息到 WindowBus
//!
//! ## 公开方法
//!
//! - `send_to()` — 发消息给指定窗口（本地或远程，由 BusMaster 路由）
//! - `broadcast_local()` — 广播给所有本地窗口
//! - `unregister()` — 注销窗口
//! - `window_id()` — 返回本窗口的全局唯一 ID

pub mod bus_master;
pub mod commands;

use tracing::warn;

use crate::ipc::protocol::{DragOp, TabState, WindowMessage};

// ---------------------------------------------------------------------------
// WindowMessageHandler — 每个窗口的消息回调
// ---------------------------------------------------------------------------

/// 每个窗口注册的消息处理器。
///
/// BusMaster 通过 `WindowMessageReceiver` tarpc service 调用这些方法。
pub trait WindowMessageHandler: Send + Sync + 'static {
    fn on_dnd_session_active(&self, session_id: u64, files: Vec<String>, operation: DragOp);
    fn on_dnd_session_completed(&self, session_id: u64);
    fn on_tab_attached(&self, tab: TabState);
}

// ---------------------------------------------------------------------------
// tarpc service 定义
// ---------------------------------------------------------------------------

/// 窗 → 主：发送请求。
///
/// WindowBus 调用此 service 将消息交给 BusMaster 路由。
#[tarpc::service]
pub trait BusMasterService {
    /// 发送消息到指定窗口（BusMaster 自动判断本地/远程）。
    async fn send_to(target_id: u64, msg: WindowMessage);

    /// 广播消息给所有本地窗口。
    async fn broadcast_local(msg: WindowMessage);

    /// 注销当前窗口（BusMaster 清理路由 + 广播 WindowUnregistered）。
    async fn unregister(window_id: u64);
}

/// 主 → 窗：推送消息。
///
/// BusMaster 调用此 service 将消息投递到 WindowBus。
#[tarpc::service]
pub trait WindowMessageReceiver {
    async fn on_dnd_session_active(session_id: u64, files: Vec<String>, operation: DragOp);
    async fn on_dnd_session_completed(session_id: u64);
    async fn on_tab_attached(tab: TabState);
}

// ---------------------------------------------------------------------------
// WindowBus — per-window 通信句柄
// ---------------------------------------------------------------------------

/// Per-window 窗口通信实例。
///
/// 通过 `BusMaster::spawn_window()` 创建。
#[derive(Clone)]
pub struct WindowBus {
    window_id: u64,
    master_client: BusMasterServiceClient,
}

impl WindowBus {
    pub(crate) fn new(window_id: u64, master_client: BusMasterServiceClient) -> Self {
        Self {
            window_id,
            master_client,
        }
    }

    /// 本窗口的全局唯一 ID。
    pub fn window_id(&self) -> u64 {
        self.window_id
    }

    /// 发送消息给指定窗口。
    ///
    /// BusMaster 自动判断目标在本地还是远程，并路由到正确的目的地。
    pub async fn send_to(&self, target_id: u64, msg: &WindowMessage) {
        if let Err(e) = self
            .master_client
            .send_to(tarpc::context::current(), target_id, msg.clone())
            .await
        {
            warn!("send_to({target_id}) failed: {e}");
        }
    }

    /// 广播消息给所有本地窗口。
    pub async fn broadcast_local(&self, msg: &WindowMessage) {
        if let Err(e) = self
            .master_client
            .broadcast_local(tarpc::context::current(), msg.clone())
            .await
        {
            warn!("broadcast_local failed: {e}");
        }
    }

    /// 注销窗口。
    ///
    /// BusMaster 会清理路由表 + 广播 `WindowUnregistered` + 清理本地 registry。
    pub async fn unregister(&self) {
        if let Err(e) = self
            .master_client
            .unregister(tarpc::context::current(), self.window_id)
            .await
        {
            warn!("unregister failed: {e}");
        }
    }
}
