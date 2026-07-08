//! 文件系统 Worker 进程池与生命周期管理。
//!
//! ## 架构
//!
//! - [`FsWorkerPool`]：按 UID 管理 Worker 进程；分发 [`UidToken`]。
//! - [`UidToken`]：RAII 凭证。持有 [`WorkerConn`] 的 `Arc`，保证操作期间 Worker 存活。
//! - [`LeaseSentinel`]：token 的引用计数哨兵；最后一个 token drop 时通知 reaper。
//! - [`WorkerConn`]：一个 Worker 的双向连接（app→worker client + worker→app 回调路由）。
//! - [`CallbackRegistry`]：把 Worker 反向回调（watcher 增量 / 进度 / 冲突）路由回对应句柄。
//!
//! ## 生命周期
//!
//! ```text
//! request_token(uid)
//!   ├─ 已有存活 token → clone Arc，复用
//!   ├─ 冷却中(conn 还在) → 新建 sentinel 复活，复用 conn（避免 root worker 重新 pkexec）
//!   └─ 无 → spawn Worker（双 socketpair）→ 建 conn
//!
//! 某 uid 所有 token drop
//!   → LeaseSentinel::drop → reaper.send(uid)
//!   → reaper 标记 cooling_deadline = now + grace
//!   → grace 后仍无新 token → SIGTERM/SIGKILL + 移除 slot
//! ```

use std::{
    collections::HashMap,
    io,
    os::unix::{
        io::{AsRawFd, RawFd},
        net::UnixStream as StdUnixStream,
    },
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};

use nix::{
    fcntl::{fcntl, FcntlArg, FdFlag},
    unistd,
};
use tokio::{net::UnixStream, sync::mpsc, sync::oneshot};
use tracing::{debug, info, warn};

use crate::ipc::protocol::{
    AppCallbackService, ConflictItem, ConflictResolution, FsWorkerServiceClient, ProgressEvent,
    WatchDelta,
};

pub mod worker;

pub use worker::run_fs_worker;

/// 全局 worker_id 计数器（仅用于日志区分）。
static WORKER_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// WorkerOpts
// ---------------------------------------------------------------------------

/// Worker 启动选项（传递给 `hnfm fs-worker` 的命令行参数）。
#[derive(Debug, Clone)]
pub struct FsWorkerOpts {
    /// Worker ID
    pub worker_id: u64,
    /// 请求通道 fd（app→worker，Worker 作 server）
    pub fd: Option<i32>,
    /// 回调通道 fd（worker→app，Worker 作 client）
    pub cb_fd: Option<i32>,
}

// ---------------------------------------------------------------------------
// CallbackRegistry — 反向回调路由
// ---------------------------------------------------------------------------

/// 把 Worker 反向回调路由到对应 `Watcher` / `Progress` 的通道。
///
/// 每个 [`WorkerConn`] 一份。`watch_id` / `op_id` 全局分配（由 FsService）。
pub struct CallbackRegistry {
    /// watch_id → watcher 增量 sender
    watches: Mutex<HashMap<u64, mpsc::UnboundedSender<WatchDelta>>>,
    /// op_id → 进度 sender
    ops: Mutex<HashMap<u64, mpsc::UnboundedSender<ProgressEvent>>>,
    /// (op_id, conflict_id) → 冲突决策应答
    conflicts: Mutex<HashMap<(u64, u64), oneshot::Sender<ConflictResolution>>>,
}

impl CallbackRegistry {
    fn new() -> Self {
        Self {
            watches: Mutex::new(HashMap::new()),
            ops: Mutex::new(HashMap::new()),
            conflicts: Mutex::new(HashMap::new()),
        }
    }

    /// 注册一个 watcher，返回增量接收端。
    pub fn register_watch(&self, watch_id: u64) -> mpsc::UnboundedReceiver<WatchDelta> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.watches.lock().unwrap().insert(watch_id, tx);
        rx
    }

    /// 注销 watcher。
    pub fn unregister_watch(&self, watch_id: u64) {
        self.watches.lock().unwrap().remove(&watch_id);
    }

    /// 注册一个批处理操作，返回进度接收端。
    pub fn register_op(&self, op_id: u64) -> mpsc::UnboundedReceiver<ProgressEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.ops.lock().unwrap().insert(op_id, tx);
        rx
    }

    /// 注销操作及其残留冲突。
    pub fn unregister_op(&self, op_id: u64) {
        self.ops.lock().unwrap().remove(&op_id);
        self.conflicts
            .lock()
            .unwrap()
            .retain(|(oid, _), _| *oid != op_id);
    }

    /// 上层给出冲突决策，唤醒等待中的 `ask_conflict`。
    pub fn resolve_conflict(&self, op_id: u64, conflict_id: u64, res: ConflictResolution) {
        if let Some(tx) = self.conflicts.lock().unwrap().remove(&(op_id, conflict_id)) {
            let _ = tx.send(res);
        }
    }

    // --- 由回调 server 调用 ---

    fn push_watch_delta(&self, watch_id: u64, delta: WatchDelta) {
        if let Some(tx) = self.watches.lock().unwrap().get(&watch_id) {
            let _ = tx.send(delta);
        }
    }

    fn push_progress(&self, op_id: u64, ev: ProgressEvent) {
        if let Some(tx) = self.ops.lock().unwrap().get(&op_id) {
            let _ = tx.send(ev);
        }
    }

    async fn ask_conflict(
        &self,
        op_id: u64,
        conflict_id: u64,
        item: ConflictItem,
    ) -> ConflictResolution {
        // 先把冲突作为进度事件抛给上层
        self.push_progress(
            op_id,
            ProgressEvent::Conflict {
                conflict_id,
                item,
            },
        );
        let (tx, rx) = oneshot::channel();
        self.conflicts
            .lock()
            .unwrap()
            .insert((op_id, conflict_id), tx);
        // 等上层调用 resolve_conflict；若通道被丢弃（Progress 已析构）→ 取消整个操作
        rx.await.unwrap_or(ConflictResolution::CancelAll)
    }
}

// ---------------------------------------------------------------------------
// WorkerConn
// ---------------------------------------------------------------------------

/// 一个 Worker 的双向连接。归池所有，通过 `Arc` 借给 token。
pub struct WorkerConn {
    /// 目标 UID
    pub uid: u32,
    /// app→worker tarpc 客户端
    pub client: FsWorkerServiceClient,
    /// worker→app 回调路由
    pub registry: Arc<CallbackRegistry>,
    /// 子进程 PID
    pub pid: u32,
}

// ---------------------------------------------------------------------------
// LeaseSentinel / UidToken
// ---------------------------------------------------------------------------

/// token 的引用计数哨兵。最后一个 token drop 时通知 reaper。
pub struct LeaseSentinel {
    uid: u32,
    reaper_tx: mpsc::UnboundedSender<u32>,
}

impl Drop for LeaseSentinel {
    fn drop(&mut self) {
        // Drop 不能 async → 仅发信号，由 reaper 处理释放
        let _ = self.reaper_tx.send(self.uid);
    }
}

/// UID 访问凭证（RAII）。
///
/// 持有 [`WorkerConn`] 的 `Arc`，保证操作期间 Worker 存活并可直接取到 client。
/// 字段私有 → 上层无法借它篡改内部数据。所有该 uid 的 token drop 后，
/// 对应 Worker 进入冷却期，宽限期后被回收。
#[derive(Clone)]
pub struct UidToken {
    uid: u32,
    conn: Arc<WorkerConn>,
    _sentinel: Arc<LeaseSentinel>,
}

impl UidToken {
    /// 目标 UID。
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// 内部连接（仅 crate 内使用）。
    pub(crate) fn conn(&self) -> &Arc<WorkerConn> {
        &self.conn
    }
}

// ---------------------------------------------------------------------------
// PoolSlot / FsWorkerPool
// ---------------------------------------------------------------------------

struct PoolSlot {
    conn: Arc<WorkerConn>,
    /// 存活 token 的弱引用；`strong_count() == 0` 表示无 token
    sentinel: Weak<LeaseSentinel>,
    /// 冷却截止时间；`Some` 表示无 token、等待宽限期回收
    cooling_deadline: Option<Instant>,
    pid: u32,
}

/// Worker 进程池，按目标 UID 索引。
pub struct FsWorkerPool {
    slots: Arc<Mutex<HashMap<u32, PoolSlot>>>,
    reaper_tx: mpsc::UnboundedSender<u32>,
}

impl FsWorkerPool {
    /// 创建进程池并启动 reaper（必须在 tokio 运行时内调用）。
    pub fn new() -> Self {
        Self::with_grace(Duration::from_secs(20))
    }

    /// 指定宽限期创建。
    pub fn with_grace(grace: Duration) -> Self {
        let slots: Arc<Mutex<HashMap<u32, PoolSlot>>> = Arc::new(Mutex::new(HashMap::new()));
        let reaper_tx = start_reaper(slots.clone(), grace);
        Self { slots, reaper_tx }
    }

    /// 请求某 uid 的访问凭证。
    ///
    /// 找到/复活/创建对应 Worker，返回 [`UidToken`]。创建失败（如 pkexec 未通过）
    /// 时返回错误。
    pub async fn request_token(&self, uid: u32) -> io::Result<UidToken> {
        // 快路径：已有 slot（存活或冷却中）
        {
            let mut g = self.slots.lock().unwrap();
            if let Some(slot) = g.get_mut(&uid) {
                if let Some(sentinel) = slot.sentinel.upgrade() {
                    slot.cooling_deadline = None;
                    return Ok(UidToken {
                        uid,
                        conn: slot.conn.clone(),
                        _sentinel: sentinel,
                    });
                }
                // 冷却中但 conn 仍在 → 新建 sentinel 复活
                let sentinel = Arc::new(LeaseSentinel {
                    uid,
                    reaper_tx: self.reaper_tx.clone(),
                });
                slot.sentinel = Arc::downgrade(&sentinel);
                slot.cooling_deadline = None;
                debug!("revived cooling worker for uid {uid}");
                return Ok(UidToken {
                    uid,
                    conn: slot.conn.clone(),
                    _sentinel: sentinel,
                });
            }
        }

        // 慢路径：spawn 新 Worker（不持锁 await）
        let registry = Arc::new(CallbackRegistry::new());
        let (pid, client) = Self::spawn_worker(uid, registry.clone()).await?;
        let conn = Arc::new(WorkerConn {
            uid,
            client,
            registry,
            pid,
        });
        let sentinel = Arc::new(LeaseSentinel {
            uid,
            reaper_tx: self.reaper_tx.clone(),
        });

        let mut g = self.slots.lock().unwrap();
        // 并发双 spawn 检查：若他人已插入存活 slot，弃用本次 spawn
        if let Some(slot) = g.get_mut(&uid) {
            if let Some(existing) = slot.sentinel.upgrade() {
                warn!("concurrent spawn for uid {uid}, killing loser pid={pid}");
                kill_worker(uid, pid);
                slot.cooling_deadline = None;
                return Ok(UidToken {
                    uid,
                    conn: slot.conn.clone(),
                    _sentinel: existing,
                });
            }
        }
        g.insert(
            uid,
            PoolSlot {
                conn: conn.clone(),
                sentinel: Arc::downgrade(&sentinel),
                cooling_deadline: None,
                pid,
            },
        );
        Ok(UidToken {
            uid,
            conn,
            _sentinel: sentinel,
        })
    }

    /// 启动一个新的 FS Worker 子进程（双 socketpair）。
    async fn spawn_worker(
        target_uid: u32,
        registry: Arc<CallbackRegistry>,
    ) -> io::Result<(u32, FsWorkerServiceClient)> {
        let worker_id = WORKER_ID.fetch_add(1, Ordering::Relaxed);

        // 1. 两条匿名 socketpair：请求通道 + 回调通道
        let (parent_req, child_req) = StdUnixStream::pair()?;
        let (parent_cb, child_cb) = StdUnixStream::pair()?;
        clear_cloexec(child_req.as_raw_fd())?;
        clear_cloexec(child_cb.as_raw_fd())?;

        let exe_path = get_exe_path();

        // 2. 构建子进程命令（同 UID 直启，否则 pkexec 提权）
        let mut cmd = if target_uid == unistd::getuid().as_raw() {
            let mut c = Command::new(&exe_path);
            if is_appimage() {
                c.current_dir("/tmp");
            }
            c
        } else {
            let mut c = Command::new("pkexec");
            c.arg("--user").arg(target_uid.to_string()).arg(&exe_path);
            if is_appimage() {
                c.current_dir("/tmp");
            }
            c
        };

        cmd.args([
            "fs-worker",
            &format!("--worker-id={worker_id}"),
            &format!("--fd={}", child_req.as_raw_fd()),
            &format!("--cb-fd={}", child_cb.as_raw_fd()),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        // 3. 启动
        let mut child = cmd.spawn()?;
        let pid = child.id();
        info!("spawned fs-worker pid={pid} worker_id={worker_id} target_uid={target_uid}");

        // 4. 关闭子端副本
        drop(child_req);
        drop(child_cb);

        // 5. 回收子进程
        tokio::task::spawn_blocking(move || match child.wait() {
            Ok(status) => info!("fs-worker pid={pid} exited with {status}"),
            Err(e) => warn!("fs-worker pid={pid} wait error: {e}"),
        });

        // 6. parent_req → app→worker client
        parent_req.set_nonblocking(true)?;
        let req_stream = UnixStream::from_std(parent_req)?;
        let req_transport = tarpc::serde_transport::new(
            crate::ipc::frame_stream(req_stream),
            tarpc::tokio_serde::formats::Bincode::default(),
        );
        let client =
            FsWorkerServiceClient::new(tarpc::client::Config::default(), req_transport).spawn();

        // 7. parent_cb → 服务 AppCallbackService（worker→app）
        parent_cb.set_nonblocking(true)?;
        let cb_stream = UnixStream::from_std(parent_cb)?;
        serve_callback(cb_stream, registry);

        Ok((pid, client))
    }
}

impl Default for FsWorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 反向回调 server（app 侧实现 AppCallbackService）
// ---------------------------------------------------------------------------

/// app 侧的 [`AppCallbackService`] 实现，把回调路由进 [`CallbackRegistry`]。
#[derive(Clone)]
struct CallbackServer {
    registry: Arc<CallbackRegistry>,
}

impl AppCallbackService for CallbackServer {
    async fn watch_delta(self, _ctx: tarpc::context::Context, watch_id: u64, delta: WatchDelta) {
        self.registry.push_watch_delta(watch_id, delta);
    }

    async fn progress(self, _ctx: tarpc::context::Context, op_id: u64, ev: ProgressEvent) {
        self.registry.push_progress(op_id, ev);
    }

    async fn ask_conflict(
        self,
        _ctx: tarpc::context::Context,
        op_id: u64,
        conflict_id: u64,
        item: ConflictItem,
    ) -> ConflictResolution {
        self.registry.ask_conflict(op_id, conflict_id, item).await
    }
}

/// 在给定流上启动 AppCallbackService server。
fn serve_callback(stream: UnixStream, registry: Arc<CallbackRegistry>) {
    use futures::prelude::*;
    use tarpc::server::Channel;

    let transport = tarpc::serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    let server = CallbackServer { registry };

    async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }

    tokio::spawn(
        tarpc::server::BaseChannel::with_defaults(transport)
            .execute(server.serve())
            .for_each(spawn),
    );
}

// ---------------------------------------------------------------------------
// Reaper — 宽限期回收
// ---------------------------------------------------------------------------

/// 启动 reaper 后台任务，返回其输入端。
///
/// 收到 uid（某 token drop）→ 若该 uid 已无存活 token，标记冷却并在宽限期后回收。
fn start_reaper(
    slots: Arc<Mutex<HashMap<u32, PoolSlot>>>,
    grace: Duration,
) -> mpsc::UnboundedSender<u32> {
    let (tx, mut rx) = mpsc::unbounded_channel::<u32>();
    tokio::spawn(async move {
        while let Some(uid) = rx.recv().await {
            let should_cool = {
                let mut g = slots.lock().unwrap();
                match g.get_mut(&uid) {
                    Some(slot) if slot.sentinel.strong_count() == 0 => {
                        slot.cooling_deadline = Some(Instant::now() + grace);
                        true
                    }
                    _ => false,
                }
            };
            if !should_cool {
                continue;
            }

            let slots2 = slots.clone();
            tokio::spawn(async move {
                tokio::time::sleep(grace).await;
                let to_kill = {
                    let mut g = slots2.lock().unwrap();
                    match g.get(&uid) {
                        Some(slot)
                            if slot.sentinel.strong_count() == 0
                                && slot
                                    .cooling_deadline
                                    .is_some_and(|d| Instant::now() >= d) =>
                        {
                            let pid = slot.pid;
                            g.remove(&uid);
                            Some(pid)
                        }
                        _ => None,
                    }
                };
                if let Some(pid) = to_kill {
                    info!("worker for uid {uid} idle past grace, reaping pid={pid}");
                    kill_worker(uid, pid);
                }
            });
        }
    });
    tx
}

/// SIGTERM 然后（2s 后）SIGKILL 终止 Worker 进程。
fn kill_worker(uid: u32, pid: u32) {
    use nix::sys::signal;
    use nix::unistd::Pid;
    let p = Pid::from_raw(pid as i32);
    debug!("killing worker uid={uid} pid={pid}");
    let _ = signal::kill(p, signal::Signal::SIGTERM);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = signal::kill(p, signal::Signal::SIGKILL);
    });
}

// ---------------------------------------------------------------------------
// FD / 路径工具
// ---------------------------------------------------------------------------

/// 清除文件描述符的 CLOEXEC 标志，使子进程能继承。
fn clear_cloexec(fd: RawFd) -> io::Result<()> {
    use std::os::fd::BorrowedFd;
    // SAFETY: fd 此刻有效
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut flags = FdFlag::from_bits_retain(
        fcntl(borrowed, FcntlArg::F_GETFD)
            .map_err(|e| io::Error::other(e.to_string()))?,
    );
    flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(borrowed, FcntlArg::F_SETFD(flags))
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(())
}

/// 获取当前可执行文件路径（处理 AppImage）。
fn get_exe_path() -> std::path::PathBuf {
    if let Ok(appimage) = std::env::var("APPIMAGE") {
        std::path::PathBuf::from(appimage)
    } else {
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("hnfm"))
    }
}

/// 是否在 AppImage 环境中运行。
fn is_appimage() -> bool {
    std::env::var("APPIMAGE").is_ok()
}
