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
//!
//! ## 公开方法
//!
//! - `start()` — 发现 + 连接
//! - `send_to()` — 发消息给指定实例
//! - `broadcast()` — 发消息给所有实例
//! - `window_instance()` — 查 window 所在实例
//! - `upsert_route()` / `remove_route()` / `remove_instance_routes()` — 路由表维护

pub mod connection;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
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
                    bus.peers.write().await.insert(instance_id, conn);
                }
                Err(e) => {
                    warn!("failed to connect to instance {instance_id}: {e}");
                }
            }
        }

        Ok(bus)
    }

    /// 添加 peer 连接（当 inotify 检测到新实例时使用）。
    pub async fn add_peer(&self, conn: PeerConnection) {
        let id = conn.instance_id();
        if id == self.self_id {
            return;
        }
        let mut peers = self.peers.write().await;
        peers.entry(id).or_insert_with(|| {
            info!("new peer connected: instance {id}");
            conn
        });
    }

    /// 移除 peer 连接（当 inotify 检测到实例退出时使用）。
    pub async fn remove_peer(&self, instance_id: u64) {
        self.peers.write().await.remove(&instance_id);
        self.remove_instance_routes(instance_id).await;
        info!("peer disconnected: instance {instance_id}");
    }

    /// 发送消息给指定实例。
    ///
    /// 锁内取出 PeerConnection，锁外执行 RPC（避免持锁 await）。
    pub async fn send_to(&self, instance_id: u64, msg: &InstanceMessage) {
        // 1. 锁内取出连接
        let mut conn = {
            let mut peers = self.peers.write().await;
            peers.remove(&instance_id)
        };

        let mut ok = false;
        if let Some(ref mut conn) = conn {
            ok = conn.send(msg).await.is_ok();
        } else {
            warn!("send_to: instance {instance_id} not connected");
        }

        // 2. 锁内放回 / 清理
        if ok {
            if let Some(conn) = conn {
                self.peers.write().await.insert(instance_id, conn);
            }
        } else {
            // 连接失败或不存在的 peer：放回失败（重连），否则清理路由
            if conn.is_some() {
                error!("send_to instance {instance_id} failed: removed dead connection");
            }
            self.remove_instance_routes(instance_id).await;
        }
    }

    /// 广播消息给所有已连接实例。
    ///
    /// 锁内收集所有连接，释放锁后并行 RPC，最后统一回收。
    pub async fn broadcast(&self, msg: &InstanceMessage) {
        // 1. 锁内取出所有连接
        let peers: Vec<(u64, PeerConnection)> = {
            let mut peers = self.peers.write().await;
            peers.drain().collect()
        };

        // 2. 无锁并行发送
        let mut results = Vec::with_capacity(peers.len());
        for (id, mut conn) in peers {
            let ok = conn.send(msg).await.is_ok();
            results.push((id, conn, ok));
        }

        // 3. 锁内放回成功 / 清理失败
        let mut dead = Vec::new();
        {
            let mut peers = self.peers.write().await;
            for (id, conn, ok) in results {
                if ok {
                    peers.insert(id, conn);
                } else {
                    error!("broadcast to instance {id} failed: removed dead connection");
                    dead.push(id);
                }
            }
        }

        // 4. 锁外清理路由
        for id in dead {
            self.remove_instance_routes(id).await;
        }
    }

    /// 查询窗口所在实例。
    pub async fn window_instance(&self, window_id: u64) -> Option<u64> {
        self.routes.read().await.get(&window_id).copied()
    }

    /// 更新路由表。
    pub async fn upsert_route(&self, window_id: u64, instance_id: u64) {
        self.routes.write().await.insert(window_id, instance_id);
    }

    /// 删除路由。
    pub async fn remove_route(&self, window_id: u64) {
        self.routes.write().await.remove(&window_id);
    }

    /// 删除某实例的所有窗口路由（实例断连时调用）。
    pub async fn remove_instance_routes(&self, instance_id: u64) {
        self.routes.write().await.retain(|_, v| *v != instance_id);
    }

    pub fn self_id(&self) -> u64 {
        self.self_id
    }
}

// ---------------------------------------------------------------------------
// 目录发现
// ---------------------------------------------------------------------------

fn instances_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(format!("{home}/.cache/hnfm/instances"))
}

/// 扫描实例目录，返回 `(instance_id, socket_path_string)` 列表。
fn discover_sockets() -> Vec<(u64, PathBuf)> {
    let dir = instances_dir();
    let mut result = Vec::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // instance_<pid>.sock
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
    let _ = std::fs::remove_file(&path); // 清理上次残留
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
// 实例目录轮询（双向拓扑）
// ---------------------------------------------------------------------------

/// 后台轮询 `~/.cache/hnfm/instances/`，自动连接新实例、清理断开实例。
pub async fn watch_instances(bus: Arc<InstanceBus>) {
    let dir = instances_dir();
    std::fs::create_dir_all(&dir).ok();

    let self_id = bus.self_id();
    let mut known: HashSet<u64> = discover_sockets()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    // 已知集中不包含自己 — 将来可以包含，但 connect 时已有 self 判断

    info!("polling instances dir: {dir:?} (interval 3s)");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let sockets = discover_sockets();
        let current: HashSet<u64> = sockets.iter().map(|(id, _)| *id).collect();

        // 新实例
        for id in current.difference(&known) {
            if *id == self_id {
                continue;
            }
            if let Some((_, path)) = sockets.iter().find(|(i, _)| *i == *id) {
                match connection::PeerConnection::connect(*id, &path.to_string_lossy()).await {
                    Ok(conn) => {
                        bus.add_peer(conn).await;
                    }
                    Err(e) => {
                        warn!("poll: connect to instance {id} failed: {e}");
                    }
                }
            }
        }

        // 断开的实例
        for id in known.difference(&current) {
            if *id != self_id {
                bus.remove_peer(*id).await;
            }
        }

        known = current;
    }
}
