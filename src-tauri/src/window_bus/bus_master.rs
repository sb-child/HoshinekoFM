//! BusMaster — per-instance 窗口消息路由器。
//!
//! 每个实例持有一个 `BusMaster`。它为每个窗口创建两对 duplex pair 走 tarpc：
//!
//! ```text
//! WindowBus ──BusMasterService──→ BusMaster ──→ 本地窗口 / InstanceBus
//! WindowBus ←─WindowMessageReceiver── BusMaster
//! ```
//!
//! 所有窗口间消息必须经 BusMaster 转发，不允许直连。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use futures::prelude::*;
use tarpc::server::Channel;
use tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec;
use tokio::io::DuplexStream;
use tokio_util::codec::Framed;
use tracing::{info, warn};

use crate::app::state::AppStateManager;
use crate::instance_bus::InstanceBus;
use crate::ipc::protocol::{InstanceMessage, WindowMessage};

use super::{
    BusMasterService, BusMasterServiceClient, WindowBus, WindowMessageHandler,
    WindowMessageReceiver, WindowMessageReceiverClient,
};

// ---------------------------------------------------------------------------
// BusMaster — 核心路由器
// ---------------------------------------------------------------------------

/// Per-instance 窗口消息路由器。
///
/// 持有所有本地窗口的 `WindowMessageReceiver` 客户端，
/// 负责消息路由（本地直投 / 远程走 InstanceBus）。
pub struct BusMaster {
    instance_bus: Arc<InstanceBus>,
    mgr: Arc<AppStateManager>,
    /// window_id → WindowMessageReceiver 客户端（用于推送消息到窗口）
    windows: RwLock<HashMap<u64, WindowMessageReceiverClient>>,
}

impl BusMaster {
    /// 创建 BusMaster 实例。
    pub fn new(instance_bus: Arc<InstanceBus>, mgr: Arc<AppStateManager>) -> Arc<Self> {
        Arc::new(Self {
            instance_bus,
            mgr,
            windows: RwLock::new(HashMap::new()),
        })
    }

    /// 获取 InstanceBus 引用。
    pub fn instance_bus(&self) -> &Arc<InstanceBus> {
        &self.instance_bus
    }

    // ── 窗口生命周期 ─────────────────────────────────────────────────────

    /// 为指定 window_id 创建全套 tarpc 连接，返回 `WindowBus`。
    ///
    /// 内部创建两对 duplex pair：
    /// - Pair A: `BusMasterService`（窗→主）
    /// - Pair B: `WindowMessageReceiver`（主→窗）
    pub async fn spawn_window(
        self: &Arc<Self>,
        window_id: u64,
        handler: Arc<dyn WindowMessageHandler>,
    ) -> WindowBus {
        // Pair A: BusMasterService (window → master)
        let (client_a, server_a) = tokio::io::duplex(4096);
        // Master 端：BusMasterService server
        let bus_clone = self.clone();
        tokio::spawn(run_bus_master_server(server_a, bus_clone));
        // Window 端：BusMasterService client
        let master_client = make_bus_master_client(client_a);

        // Pair B: WindowMessageReceiver (master → window)
        let (client_b, server_b) = tokio::io::duplex(4096);
        // Window 端：WindowMessageReceiver server
        tokio::spawn(run_window_receiver_server(server_b, handler));
        // Master 端：WindowMessageReceiver client
        let receiver_client = make_window_receiver_client(client_b);

        // 注册到 windows 表
        self.windows
            .write()
            .unwrap()
            .insert(window_id, receiver_client);

        // 注册到 InstanceBus 路由表
        self.instance_bus
            .upsert_route(window_id, self.instance_bus.self_id());

        // 注册到本地 registry（供 InstanceBusHandler::on_forward 使用）
        // 注意：WebviewWindow 由调用方负责插入 window_registry

        // 广播给所有实例以同步路由
        self.instance_bus
            .broadcast(&InstanceMessage::WindowRegistered {
                window_id,
                instance_id: self.instance_bus.self_id(),
            })
            .await;

        info!("bus_master: spawned window {window_id}");

        WindowBus::new(window_id, master_client)
    }

    /// 注销窗口：清理路由 + 广播 WindowUnregistered + 清理本地 registry。
    pub async fn unregister_window(&self, window_id: u64) {
        // 从 BusMaster windows 表移除
        self.windows.write().unwrap().remove(&window_id);

        // 从 InstanceBus 路由表移除
        self.instance_bus.remove_route(window_id);

        // 广播给所有实例
        self.instance_bus
            .broadcast(&InstanceMessage::WindowUnregistered { window_id })
            .await;

        // 从本地 window_registry 移除
        self.mgr.window_registry.lock().unwrap().remove(&window_id);

        info!("bus_master: unregistered window {window_id}");
    }

    // ── 消息路由 ─────────────────────────────────────────────────────────

    /// 向指定窗口投递消息。
    ///
    /// 由 `InstanceBusHandler::on_forward` 调用，用于跨实例消息投递。
    pub fn dispatch_to(&self, window_id: u64, msg: &WindowMessage) {
        let windows = self.windows.read().unwrap();
        if let Some(client) = windows.get(&window_id) {
            let mut client = client.clone();
            let msg = msg.clone();
            tokio::spawn(async move {
                if let Err(e) = dispatch_to_receiver(&mut client, &msg).await {
                    warn!("dispatch_to window {window_id} failed: {e}");
                }
            });
        } else {
            warn!("dispatch_to: window {window_id} not found in BusMaster");
        }
    }
}

// ---------------------------------------------------------------------------
// BusMasterService 实现（窗→主）
// ---------------------------------------------------------------------------

impl BusMasterService for Arc<BusMaster> {
    async fn send_to(self, _ctx: tarpc::context::Context, target_id: u64, msg: WindowMessage) {
        // 1. 尝试本地投递
        {
            let windows = self.windows.read().unwrap();
            if let Some(client) = windows.get(&target_id) {
                let mut client = client.clone();
                tokio::spawn(async move {
                    if let Err(e) = dispatch_to_receiver(&mut client, &msg).await {
                        warn!("local send_to({target_id}) failed: {e}");
                    }
                });
                return;
            }
        }

        // 2. 远程投递
        match self.instance_bus.window_instance(target_id) {
            Some(instance_id) => {
                self.instance_bus
                    .send_to(
                        instance_id,
                        &InstanceMessage::ForwardToWindow {
                            window_id: target_id,
                            msg,
                        },
                    )
                    .await;
            }
            None => {
                warn!("send_to: window {target_id} not found in route table");
            }
        }
    }

    async fn broadcast_local(self, _ctx: tarpc::context::Context, msg: WindowMessage) {
        let windows = self.windows.read().unwrap();
        for (id, client) in windows.iter() {
            let mut client = client.clone();
            let msg = msg.clone();
            let id = *id;
            tokio::spawn(async move {
                if let Err(e) = dispatch_to_receiver(&mut client, &msg).await {
                    warn!("broadcast_local to window {id} failed: {e}");
                }
            });
        }
    }

    async fn unregister(self, _ctx: tarpc::context::Context, window_id: u64) {
        self.unregister_window(window_id).await;
    }
}

// ---------------------------------------------------------------------------
// WindowMessageReceiver 分发
// ---------------------------------------------------------------------------

/// 将 `WindowMessage` 分发到 `WindowMessageReceiverClient`。
fn dispatch_to_receiver(
    client: &mut WindowMessageReceiverClient,
    msg: &WindowMessage,
) -> Pin<Box<dyn Future<Output = Result<(), tarpc::client::RpcError>> + Send>> {
    let ctx = tarpc::context::current();
    match msg.clone() {
        WindowMessage::DndSessionActive {
            session_id,
            files,
            operation,
        } => {
            let c = client.clone();
            Box::pin(async move {
                c.on_dnd_session_active(ctx, session_id, files, operation)
                    .await
            })
        }
        WindowMessage::DndSessionCompleted { session_id } => {
            let c = client.clone();
            Box::pin(async move { c.on_dnd_session_completed(ctx, session_id).await })
        }
        WindowMessage::TabAttached { tab } => {
            let c = client.clone();
            Box::pin(async move { c.on_tab_attached(ctx, tab).await })
        }
    }
}

// ---------------------------------------------------------------------------
// tarpc transport 工具
// ---------------------------------------------------------------------------

/// 为 DuplexStream 创建 LengthDelimitedCodec 帧编码传输层。
fn framed_duplex(stream: DuplexStream) -> Framed<DuplexStream, LengthDelimitedCodec> {
    LengthDelimitedCodec::builder().new_framed(stream)
}

/// 在 DuplexStream 上启动 BusMasterService server。
async fn run_bus_master_server(stream: DuplexStream, bus: Arc<BusMaster>) {
    let transport = tarpc::serde_transport::new(
        framed_duplex(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    tarpc::server::BaseChannel::with_defaults(transport)
        .execute(bus.serve())
        .for_each(|fut| async move {
            tokio::spawn(fut);
        })
        .await;
}

/// 在 DuplexStream 上启动 WindowMessageReceiver server。
async fn run_window_receiver_server(stream: DuplexStream, handler: Arc<dyn WindowMessageHandler>) {
    let transport = tarpc::serde_transport::new(
        framed_duplex(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    let server = WindowReceiver { handler };
    tarpc::server::BaseChannel::with_defaults(transport)
        .execute(server.serve())
        .for_each(|fut| async move {
            tokio::spawn(fut);
        })
        .await;
}

/// 从 DuplexStream 创建 BusMasterService client。
fn make_bus_master_client(stream: DuplexStream) -> BusMasterServiceClient {
    let transport = tarpc::serde_transport::new(
        framed_duplex(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    BusMasterServiceClient::new(tarpc::client::Config::default(), transport).spawn()
}

/// 从 DuplexStream 创建 WindowMessageReceiver client。
fn make_window_receiver_client(stream: DuplexStream) -> WindowMessageReceiverClient {
    let transport = tarpc::serde_transport::new(
        framed_duplex(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    WindowMessageReceiverClient::new(tarpc::client::Config::default(), transport).spawn()
}

// ---------------------------------------------------------------------------
// WindowMessageReceiver server 实现
// ---------------------------------------------------------------------------

/// WindowMessageReceiver server 载体。
#[derive(Clone)]
struct WindowReceiver {
    handler: Arc<dyn WindowMessageHandler>,
}

impl WindowMessageReceiver for WindowReceiver {
    async fn on_dnd_session_active(
        self,
        _ctx: tarpc::context::Context,
        session_id: u64,
        files: Vec<String>,
        operation: crate::ipc::protocol::DragOp,
    ) {
        self.handler
            .on_dnd_session_active(session_id, files, operation);
    }

    async fn on_dnd_session_completed(self, _ctx: tarpc::context::Context, session_id: u64) {
        self.handler.on_dnd_session_completed(session_id);
    }

    async fn on_tab_attached(
        self,
        _ctx: tarpc::context::Context,
        tab: crate::ipc::protocol::TabState,
    ) {
        self.handler.on_tab_attached(tab);
    }
}
