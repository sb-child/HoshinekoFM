//! 实例间 Mesh 通信层（传输后端）。
//!
//! ## 拓扑
//!
//! ```text
//! ~/.cache/hnfm/instances/
//! ├── instance_1024.sock  ← A (listen + connected to B,C)
//! ├── instance_2048.sock  ← B (listen + connected to A,C)
//! └── instance_3072.sock  ← C (listen + connected to A,B)
//! ```
//!
//! 每个实例发现目录中所有 socket -> 直连 -> 持久 PeerConnection。
//! 窗口路由表通过 fan-out 广播保持同步。
//!
//! 注意：实例发现函数（bind_socket/cleanup_socket/discover_sockets 等）
//! 已移至 `crate::mesh::discovery`。`watch_instances` 后台任务保留于此。

pub mod connection;
pub mod watcher;

use crate::lock::{ReadSafe, WriteSafe};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use futures::prelude::*;
use tarpc::server::Channel;
use tracing::{debug, error, info, warn};

use crate::mesh::Mesh;
use crate::mesh::types::instance::{InstanceMsg, InstanceService};
use connection::PeerConnection;

// Re-export discovery functions from mesh
pub use crate::mesh::discovery::{
    bind_socket, cleanup_socket, discover_sockets, instance_exists, instances_dir, lock_path,
    socket_path,
};

pub struct InstanceBus {
    self_id: u64,
    peers: RwLock<HashMap<u64, PeerConnection>>,
    /// window_id -> instance_id
    routes: RwLock<HashMap<u64, u64>>,
}

impl InstanceBus {
    pub async fn start(self_id: u64) -> Result<Arc<Self>, String> {
        let bus = Arc::new(Self {
            self_id,
            peers: RwLock::new(HashMap::new()),
            routes: RwLock::new(HashMap::new()),
        });
        Ok(bus)
    }

    pub fn add_peer(&self, conn: PeerConnection) {
        let id = conn.instance_id();
        if id == self.self_id {
            return;
        }
        let mut peers = self.peers.write_safe();
        peers.entry(id).or_insert_with(|| {
            info!("new peer connected: instance {id}");
            conn
        });
    }

    pub fn remove_peer(&self, instance_id: u64) {
        self.peers.write_safe().remove(&instance_id);
        self.remove_instance_routes(instance_id);
        info!("peer disconnected: instance {instance_id}");
    }

    pub async fn send_to(&self, instance_id: u64, msg: &InstanceMsg) {
        let mut conn = {
            let mut peers = self.peers.write_safe();
            peers.remove(&instance_id)
        };

        let mut ok = false;
        if let Some(ref mut conn) = conn {
            ok = conn.send(msg).await.is_ok();
        } else {
            warn!("send_to: instance {instance_id} not connected");
        }

        if ok {
            if let Some(conn) = conn {
                self.peers.write_safe().insert(instance_id, conn);
            }
        } else {
            if conn.is_some() {
                error!("send_to instance {instance_id} failed: removed dead connection");
            }
            self.remove_instance_routes(instance_id);
        }
    }

    pub async fn broadcast(&self, msg: &InstanceMsg) {
        let peers: Vec<(u64, PeerConnection)> = {
            let mut peers = self.peers.write_safe();
            peers.drain().collect()
        };

        let mut results = Vec::with_capacity(peers.len());
        for (id, mut conn) in peers {
            let ok = conn.send(msg).await.is_ok();
            results.push((id, conn, ok));
        }

        let mut dead = Vec::new();
        {
            let mut peers = self.peers.write_safe();
            for (id, conn, ok) in results {
                if ok {
                    peers.insert(id, conn);
                } else {
                    error!("broadcast to instance {id} failed: removed dead connection");
                    dead.push(id);
                }
            }
        }

        for id in dead {
            self.remove_instance_routes(id);
        }
    }

    pub fn window_instance(&self, window_id: u64) -> Option<u64> {
        self.routes.read_safe().get(&window_id).copied()
    }

    pub fn upsert_route(&self, window_id: u64, instance_id: u64) {
        self.routes.write_safe().insert(window_id, instance_id);
    }

    pub fn remove_route(&self, window_id: u64) {
        self.routes.write_safe().remove(&window_id);
    }

    pub fn remove_instance_routes(&self, instance_id: u64) {
        self.routes
            .write()
            .unwrap()
            .retain(|_, v| *v != instance_id);
    }

    pub fn self_id(&self) -> u64 {
        self.self_id
    }

    /// 启动 tarpc server 接受循环。
    ///
    /// 每个进入的连接创建一个 `InstanceBusServer`（实现 `InstanceService`），
    /// 由 Mesh 的 `InstanceHandler` 处理 app 层 RPC。
    pub async fn listen(self: &Arc<Self>, listener: tokio::net::UnixListener, mesh: Arc<Mesh>) {
        info!("accepting instance connections...");

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!("instance connection from {addr:?}");
                    let mesh = mesh.clone();
                    tokio::spawn(async move {
                        let transport = tarpc::serde_transport::new(
                            crate::mesh::transport::frame_stream(stream),
                            tarpc::tokio_serde::formats::Bincode::default(),
                        );
                        let server = crate::mesh::server::InstanceBusServer::new(mesh);

                        async fn spawn(
                            fut: impl std::future::Future<Output = ()> + Send + 'static,
                        ) {
                            tokio::spawn(fut);
                        }

                        tarpc::server::BaseChannel::with_defaults(transport)
                            .execute(server.serve())
                            .for_each(spawn)
                            .await;
                    });
                }
                Err(e) => {
                    error!("instance listener error: {e}");
                    break;
                }
            }
        }
    }
}

// --
// 实例目录监听（notify，Linux: inotify / macOS: FSEvents）
// --

/// 后台监听 `~/.cache/hnfm/instances/`，自动连接新实例、清理断开实例。
pub async fn watch_instances(bus: Arc<InstanceBus>) {
    let dir = instances_dir();
    std::fs::create_dir_all(&dir).ok();

    let self_id = bus.self_id();
    let rx = watcher::watch_dir(dir);

    info!("watching instances dir via notify (inotify)");

    while let Ok(event) = rx.recv().await {
        match event {
            watcher::WatchEvent::Init(sockets) => {
                for (id, path) in sockets {
                    if id == self_id {
                        continue;
                    }
                    if !crate::mesh::discovery::is_instance_alive(id) {
                        warn!("watch_instances: stale socket for instance {id}, removing {path:?}");
                        let _ = std::fs::remove_file(&path);
                        let _ = std::fs::remove_file(&lock_path(id));
                        continue;
                    }
                    debug!("watch_instances: connecting to instance {id}");
                    match connection::PeerConnection::connect(id, &path.to_string_lossy()).await {
                        Ok(conn) => bus.add_peer(conn),
                        Err(e) => warn!("init: connect to instance {id} failed: {e}"),
                    }
                }
            }
            watcher::WatchEvent::New(id, path) if id != self_id => {
                match connection::PeerConnection::connect(id, &path.to_string_lossy()).await {
                    Ok(conn) => bus.add_peer(conn),
                    Err(e) => warn!("connect to instance {id} failed: {e}"),
                }
            }
            watcher::WatchEvent::Gone(id) if id != self_id => {
                bus.remove_peer(id);
            }
            _ => {}
        }
    }
}
