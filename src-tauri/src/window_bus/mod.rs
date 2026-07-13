//! Per-window 窗口通信总线。
//!
//! 每个窗口持有一个 WindowBus 实例。窗口 ID 是全局唯一的 u64，
//! 在 `init()` 时通过抢占（冲突检测 + broadcast `WindowRegistered`）获取。
//!
//! ## 公开方法
//!
//! - `init()` — 抢占 window_id，注册到 AppStateManager + InstanceBus 路由表
//! - `window_id()` — 返回本窗口的全局唯一 ID
//! - `send_to()` — 发消息给指定窗口（本地直接 emit，远程走 InstanceBus P2P）
//! - `broadcast_local()` — 广播给所有本地窗口
//! - `unregister()` — 注销 + broadcast WindowUnregistered + 清理 registry

pub mod commands;

use std::sync::Arc;

use tauri::Emitter;
use tracing::warn;

use crate::app::state::AppStateManager;
use crate::instance_bus::InstanceBus;
use crate::ipc::protocol::{InstanceMessage, WindowMessage};

/// Per-window 窗口通信实例。
///
/// 窗口创建后调用 `WindowBus::init()` 获取实例。
#[derive(Clone)]
pub struct WindowBus {
    window_id: u64,
    bus: Arc<InstanceBus>,
    mgr: Arc<AppStateManager>,
}

impl WindowBus {
    /// 初始化 per-window bus：抢占全局唯一 window_id，注册到路由表和本地 registry。
    pub async fn init(
        bus: Arc<InstanceBus>,
        window: tauri::WebviewWindow,
        mgr: Arc<AppStateManager>,
    ) -> Self {
        let window_id = mgr.claim_window_id(&bus).await;

        // 加入全局路由表
        bus.upsert_route(window_id, bus.self_id());

        // 注册到本地 registry（供本地 emit 和 MeshServer::forward 查询）
        mgr.window_registry
            .lock()
            .unwrap()
            .insert(window_id, window);

        // 广播给所有实例以同步路由
        bus.broadcast(&InstanceMessage::WindowRegistered {
            window_id,
            instance_id: bus.self_id(),
        })
        .await;

        tracing::info!("window_bus: claimed window_id={window_id}");

        Self {
            window_id,
            bus,
            mgr,
        }
    }

    /// 本窗口的全局唯一 ID。
    pub fn window_id(&self) -> u64 {
        self.window_id
    }

    /// 获取底层 InstanceBus 引用。
    pub fn instance_bus(&self) -> &Arc<InstanceBus> {
        &self.bus
    }

    /// 发送消息给指定窗口。
    ///
    /// 本地窗口直接从 registry emit；远程窗口走 InstanceBus P2P 转发。
    pub async fn send_to(&self, target_id: u64, msg: &WindowMessage) {
        // 本地优先
        {
            let reg = self.mgr.window_registry.lock().unwrap();
            if let Some(window) = reg.get(&target_id) {
                let event = msg_to_event(msg);
                let _ = window.emit(event, msg);
                return;
            }
        }

        // 远程
        match self.bus.window_instance(target_id) {
            Some(instance_id) => {
                self.bus
                    .send_to(
                        instance_id,
                        &InstanceMessage::ForwardToWindow {
                            window_id: target_id,
                            msg: msg.clone(),
                        },
                    )
                    .await;
            }
            None => {
                warn!("send_to: window {target_id} not found in route table");
            }
        }
    }

    /// 广播消息给所有本地窗口。
    pub fn broadcast_local(&self, msg: &WindowMessage) {
        let event = msg_to_event(msg);
        let reg = self.mgr.window_registry.lock().unwrap();
        for window in reg.values() {
            let _ = window.emit(event, msg);
        }
    }

    /// 向本窗口 emit 消息（供 MeshServer::forward 使用）。
    pub fn emit_self(&self, msg: &WindowMessage) {
        let event = msg_to_event(msg);
        let reg = self.mgr.window_registry.lock().unwrap();
        if let Some(window) = reg.get(&self.window_id) {
            let _ = window.emit(event, msg);
        }
    }

    /// 注销：移除路由 + broadcast WindowUnregistered + 清理本地 registry。
    pub async fn unregister(&self) {
        self.bus.remove_route(self.window_id);
        self.bus
            .broadcast(&InstanceMessage::WindowUnregistered {
                window_id: self.window_id,
            })
            .await;
        self.mgr
            .window_registry
            .lock()
            .unwrap()
            .remove(&self.window_id);
        tracing::info!("window_bus: unregistered window_id={}", self.window_id);
    }
}

// ---------------------------------------------------------------------------
// 工具函数
// ---------------------------------------------------------------------------

/// 将 WindowMessage 映射到 Tauri 事件名。
pub fn msg_to_event(msg: &WindowMessage) -> &'static str {
    match msg {
        WindowMessage::DndSessionActive { .. } => "dnd:session_active",
        WindowMessage::DndSessionCompleted { .. } => "dnd:session_completed",
        WindowMessage::TabAttached { .. } => "tab:attached",
    }
}
