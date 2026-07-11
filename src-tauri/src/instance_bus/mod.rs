//! 实例间 Mesh 通信层。
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
//! 每个实例发现目录中所有 socket → 直连 → 持久 PeerConnection。
//! 窗口路由表通过 fan-out 广播保持同步。

pub mod connection;
pub mod watcher;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tracing::{error, info, warn};

use crate::ipc::protocol::InstanceMessage;
use connection::PeerConnection;

pub struct InstanceBus {
    self_id: u64,
    peers: RwLock<HashMap<u64, PeerConnection>>,
    /// window_id → instance_id
    routes: RwLock<HashMap<u64, u64>>,
}

impl InstanceBus {
    /// 启动：发现并连接所有 peer。
    pub async fn start(self_id: u64) -> Result<Arc<Self>, String> {
        let bus = Arc::new(Self {
            self_id,
            peers: RwLock::new(HashMap::new()),
            routes: RwLock::new(HashMap::new()),
        });

        let sockets = discover_sockets();
        for (instance_id, path) in sockets {
            if instance_id == self_id {
                continue;
            }
            match PeerConnection::connect(instance_id, &path.to_string_lossy()).await {
                Ok(conn) => {
                    info!("connected to instance {instance_id}");
                    bus.peers.write().unwrap().insert(instance_id, conn);
                }
                Err(e) => {
                    warn!("failed to connect to instance {instance_id}: {e}");
                }
            }
        }

        Ok(bus)
    }

    /// 添加 peer 连接。
    pub fn add_peer(&self, conn: PeerConnection) {
        let id = conn.instance_id();
        if id == self.self_id {
            return;
        }
        let mut peers = self.peers.write().unwrap();
        peers.entry(id).or_insert_with(|| {
            info!("new peer connected: instance {id}");
            conn
        });
    }

    /// 移除 peer 连接。
    pub fn remove_peer(&self, instance_id: u64) {
        self.peers.write().unwrap().remove(&instance_id);
        self.remove_instance_routes(instance_id);
        info!("peer disconnected: instance {instance_id}");
    }

    /// 发送消息给指定实例。
    ///
    /// 锁内取出 PeerConnection，锁外执行 RPC（避免持锁 await）。
    pub async fn send_to(&self, instance_id: u64, msg: &InstanceMessage) {
        let mut conn = {
            let mut peers = self.peers.write().unwrap();
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
                self.peers.write().unwrap().insert(instance_id, conn);
            }
        } else {
            if conn.is_some() {
                error!("send_to instance {instance_id} failed: removed dead connection");
            }
            self.remove_instance_routes(instance_id);
        }
    }

    /// 广播消息给所有已连接实例。
    ///
    /// 锁内收集所有连接，释放锁后并行 RPC，最后统一回收。
    pub async fn broadcast(&self, msg: &InstanceMessage) {
        let peers: Vec<(u64, PeerConnection)> = {
            let mut peers = self.peers.write().unwrap();
            peers.drain().collect()
        };

        let mut results = Vec::with_capacity(peers.len());
        for (id, mut conn) in peers {
            let ok = conn.send(msg).await.is_ok();
            results.push((id, conn, ok));
        }

        let mut dead = Vec::new();
        {
            let mut peers = self.peers.write().unwrap();
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

    /// 查询窗口所在实例。
    pub fn window_instance(&self, window_id: u64) -> Option<u64> {
        self.routes.read().unwrap().get(&window_id).copied()
    }

    /// 更新路由表。
    pub fn upsert_route(&self, window_id: u64, instance_id: u64) {
        self.routes.write().unwrap().insert(window_id, instance_id);
    }

    /// 删除路由。
    pub fn remove_route(&self, window_id: u64) {
        self.routes.write().unwrap().remove(&window_id);
    }

    /// 删除某实例的所有窗口路由（实例断连时调用）。
    pub fn remove_instance_routes(&self, instance_id: u64) {
        self.routes.write().unwrap().retain(|_, v| *v != instance_id);
    }

    pub fn self_id(&self) -> u64 {
        self.self_id
    }
}

// ---------------------------------------------------------------------------
// 目录发现
// ---------------------------------------------------------------------------

pub fn instances_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(format!("{home}/.cache/hnfm/instances"))
}

/// 扫描实例目录，返回 `(instance_id, socket_path)` 列表。
pub fn discover_sockets() -> Vec<(u64, PathBuf)> {
    let dir = instances_dir();
    let mut result = Vec::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(pid_str) = name
            .strip_prefix("instance_")
            .and_then(|s| s.strip_suffix(".sock"))
        {
            if let Ok(pid) = pid_str.parse::<u64>() {
                result.push((pid, path));
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// 对外工具函数（供 appreuse 和 app/mod.rs 使用）
// ---------------------------------------------------------------------------

/// 本实例的 socket 文件路径。
pub fn socket_path(instance_id: u64) -> PathBuf {
    instances_dir().join(format!("instance_{instance_id}.sock"))
}

/// 绑定本实例的 Unix Domain Socket。
pub fn bind_socket(instance_id: u64) -> io::Result<tokio::net::UnixListener> {
    let dir = instances_dir();
    std::fs::create_dir_all(&dir)?;
    let path = socket_path(instance_id);
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    info!("instance {instance_id} listening on {path:?}");
    Ok(listener)
}

/// 退出时删除本实例的 socket 文件（best effort，SIGKILL 无效）。
pub fn cleanup_socket(instance_id: u64) {
    let path = socket_path(instance_id);
    if let Err(e) = std::fs::remove_file(&path) {
        warn!("failed to cleanup socket {path:?}: {e}");
    } else {
        info!("cleaned up socket {path:?}");
    }
}

// ---------------------------------------------------------------------------
// 实例目录监听（notify，Linux: inotify / macOS: FSEvents）
// ---------------------------------------------------------------------------

/// 后台监听 `~/.cache/hnfm/instances/`，自动连接新实例、清理断开实例。
///
/// 内部使用 notify（Linux inotify）即时感知，不再轮询。
/// 阻塞 I/O（`std::fs::*`、notify 回调）在专用 std 线程，
/// tokio 侧只做 RPC（连接 peer），不阻塞 worker 线程。
pub async fn watch_instances(bus: Arc<InstanceBus>) {
    let dir = instances_dir();
    std::fs::create_dir_all(&dir).ok();

    let self_id = bus.self_id();
    let mut rx = watcher::watch_dir(dir);

    info!("watching instances dir via notify (inotify)");

    while let Some(event) = rx.recv().await {
        match event {
            watcher::WatchEvent::Init(sockets) => {
                for (id, path) in sockets {
                    if id == self_id {
                        continue;
                    }
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
