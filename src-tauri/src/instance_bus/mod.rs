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
pub mod server;
pub mod watcher;

use std::collections::HashMap;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use futures::prelude::*;
use tarpc::server::Channel;
use tracing::{debug, error, info, warn};

use crate::ipc::protocol::{InstanceMessage, InstanceService};
use connection::PeerConnection;
use server::InstanceBusHandler;

pub struct InstanceBus {
    self_id: u64,
    peers: RwLock<HashMap<u64, PeerConnection>>,
    /// window_id → instance_id
    routes: RwLock<HashMap<u64, u64>>,
    /// App 层消息处理器（listen 前必须设置）
    handler: RwLock<Option<Arc<dyn InstanceBusHandler>>>,
}

impl InstanceBus {
    /// 启动：创建总线实例（不扫描 peer，由 watch_instances 统一发现）。
    pub async fn start(self_id: u64) -> Result<Arc<Self>, String> {
        let bus = Arc::new(Self {
            self_id,
            peers: RwLock::new(HashMap::new()),
            routes: RwLock::new(HashMap::new()),
            handler: RwLock::new(None),
        });

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
        self.routes
            .write()
            .unwrap()
            .retain(|_, v| *v != instance_id);
    }

    pub fn self_id(&self) -> u64 {
        self.self_id
    }

    /// 设置 app 层消息处理器。必须在 `listen()` 前调用。
    pub fn set_handler(&self, handler: Arc<dyn InstanceBusHandler>) {
        *self.handler.write().unwrap() = Some(handler);
    }

    /// 启动 tarpc server 接受循环。
    ///
    /// 每个进入的连接创建一个 `InstanceBusServer`（实现 `InstanceService`），
    /// 由 handler 处理需要 app 层逻辑的 RPC。
    pub async fn listen(self: &Arc<Self>, listener: tokio::net::UnixListener) {
        info!("accepting instance connections...");
        let handler = self
            .handler
            .read()
            .unwrap()
            .clone()
            .expect("set_handler() must be called before listen()");

        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    debug!("instance connection from {addr:?}");
                    let bus = self.clone();
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        let transport = tarpc::serde_transport::new(
                            crate::ipc::frame_stream(stream),
                            tarpc::tokio_serde::formats::Bincode::default(),
                        );
                        let server = server::InstanceBusServer::new(bus, handler);

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

// ---------------------------------------------------------------------------
// 目录发现
// ---------------------------------------------------------------------------

pub fn instances_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(format!("{home}/.cache/hnfm/instances"))
}

/// 扫描实例目录，返回 `(instance_id, socket_path)` 列表。
///
/// 通过 lock 文件 flock 判断存活，同时清理残骸。
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
            if let Ok(id) = pid_str.parse::<u64>() {
                if is_instance_alive(id) {
                    result.push((id, path));
                } else {
                    warn!("discover_sockets: stale socket for instance {id}, removing {path:?}");
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(&lock_path(id));
                }
            }
        }
    }

    result
}

/// 检查 PID 是否存活。
fn pid_alive(pid: u64) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

// ---------------------------------------------------------------------------
// 对外工具函数（供 appreuse 和 app/mod.rs 使用）
// ---------------------------------------------------------------------------

/// 实例的 lock 文件路径。
pub fn lock_path(instance_id: u64) -> PathBuf {
    instances_dir().join(format!("instance_{instance_id}.lock"))
}

/// 检查实例是否存活。
///
/// 优先通过 lock 文件的 flock 判断，无 lock 文件时回退到 `pid_alive()`
/// 以兼容旧版本实例（未持有 lock 文件）。
fn is_instance_alive(instance_id: u64) -> bool {
    let lock = lock_path(instance_id);
    if let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create(false)
        .open(&lock)
    {
        let fd = file.as_raw_fd();
        let held = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) != 0 };
        if !held {
            unsafe { libc::flock(fd, libc::LOCK_UN) };
        }
        drop(file);
        return held;
    }
    pid_alive(instance_id)
}

/// 公开的存活检查，供 `main.rs` 早期冲突检测使用。
pub fn instance_exists(instance_id: u64) -> bool {
    is_instance_alive(instance_id)
}

/// 本实例的 socket 文件路径。
pub fn socket_path(instance_id: u64) -> PathBuf {
    instances_dir().join(format!("instance_{instance_id}.sock"))
}

/// 绑定本实例的 Unix Domain Socket（含竞态保护）。
pub fn bind_socket(instance_id: u64) -> io::Result<tokio::net::UnixListener> {
    let dir = instances_dir();
    std::fs::create_dir_all(&dir)?;

    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(lock_path(instance_id))?;
    let fd = lock_file.as_raw_fd();
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("instance {instance_id} is already running"),
        ));
    }
    std::mem::forget(lock_file);

    let path = socket_path(instance_id);
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    info!("instance {instance_id} listening on {path:?}");
    Ok(listener)
}

/// 退出时删除本实例的 socket 和 lock 文件。
pub fn cleanup_socket(instance_id: u64) {
    let path = socket_path(instance_id);
    if let Err(e) = std::fs::remove_file(&path) {
        warn!("failed to cleanup socket {path:?}: {e}");
    } else {
        info!("cleaned up socket {path:?}");
    }
    let lock = lock_path(instance_id);
    let _ = std::fs::remove_file(&lock);
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
    let rx = watcher::watch_dir(dir);

    info!("watching instances dir via notify (inotify)");

    while let Ok(event) = rx.recv().await {
        match event {
            watcher::WatchEvent::Init(sockets) => {
                for (id, path) in sockets {
                    if id == self_id {
                        continue;
                    }
                    if !is_instance_alive(id) {
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
