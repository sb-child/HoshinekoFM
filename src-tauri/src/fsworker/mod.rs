//! 文件系统 Worker 进程池与生命周期管理。
//!
//! ## 架构
//!
//! ```text
//! FsService  ──(channel)──→  FsWorkerPool(WorkerRelay per UID)  ──(tarpc)──→  FsWorker process
//!   ↑                                 │                                          │
//!   └──(CallbackRegistry route)───────┴──(CallbackServer tarpc)─────────────────┘
//! ```
//!
//! - [`FsWorkerPool`]：按 UID 管理 Worker 进程；分发 [`UidToken`]。
//! - [`UidToken`]：RAII 凭证。持有 `request_tx` 和 `registry`，保证操作期间 Worker 存活。
//! - [`LeaseSentinel`]：token 的引用计数哨兵；最后一个 token drop 时通知 reaper。
//! - [`CallbackRegistry`]：把 Worker 反向回调（watcher 增量 / 进度 / 冲突）路由回对应句柄。
//! - [`WorkerRelay`]：per-UID async loop，负责 spawn/心跳/消息中继/崩溃重启/状态汇报。
//!
//! ## 生命周期
//!
//! ```text
//! request_token(uid)
//!   ├─ 已有存活 token → 复用（共享 relay 的 request_tx + registry）
//!   └─ 无 → 创建 WorkerRelay → 开始事件循环
//!
//! 某 uid 所有 token drop
//!   → LeaseSentinel::drop → reaper
//!   → WorkerRelay 检测到无 token → 立即销毁（无宽限期）
//! ```
//!
//! ## 崩溃恢复
//!
//! Worker 崩溃时 WorkerRelay:
//! 1. 通过 CallbackRegistry 对所有活跃 watcher/op 发送 ConnectionLost
//! 2. kill 子进程 → 延迟 1s → spawn 新子进程
//! 3. 无次数限制，直到 uidtoken 全部 drop
//!
//! ## 心跳检测
//!
//! - 每 0.5s 检查子进程存活 + ping tarpc
//! - 超时 1s，连续失败 2 次 → 判定崩溃

use std::{
    collections::HashMap,
    io,
    os::unix::{
        io::{AsRawFd, RawFd},
        net::UnixStream as StdUnixStream,
    },
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
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
    AppCallbackService, ConflictItem, ConflictResolution, EntryKind, FsWorkerServiceClient,
    ProgressEvent, WatchDelta,
};

pub mod worker;

pub use worker::run_fs_worker;

/// 全局 fs_worker_id 计数器（仅用于日志区分）。
static FS_WORKER_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// FsWorkerOpts
// ---------------------------------------------------------------------------

/// FS Worker 启动选项（传递给 `hnfm __fs-worker` 的命令行参数）。
#[derive(Debug, Clone)]
pub struct FsWorkerOpts {
    /// FS Worker ID
    pub fs_worker_id: u64,
    /// 请求通道 fd（app→worker，Worker 作 server）
    pub fd: Option<i32>,
    /// 回调通道 fd（worker→app，Worker 作 client）
    pub cb_fd: Option<i32>,
    /// 主进程 PID（用于 Worker 侧孤儿检测，绕过 pkexec 中介）。
    pub parent_pid: Option<u32>,
}

// ---------------------------------------------------------------------------
// WorkerStatus / DisconnectReason — 连接状态汇报
// ---------------------------------------------------------------------------

/// Worker 连接状态（WorkerRelay → 上层 FsService）。
#[derive(Debug, Clone)]
pub enum WorkerStatus {
    /// 已连接并正常运行。
    Connected { uid: u32, pid: u32 },
    /// 已断开。
    Disconnected {
        uid: u32,
        reason: DisconnectReason,
        reconnecting: bool,
    },
    /// 尝试（重新）连接中。
    Reconnecting { uid: u32, attempt: u32 },
    /// 进程已 spawn，等待首次 tarpc 握手完成（例如 pkexec 等待密码期间）。
    Connecting { uid: u32 },
}

/// 断开连接原因。
#[derive(Debug, Clone)]
pub enum DisconnectReason {
    ProcessExited { status: i32 },
    ProcessCrashed { signal: i32 },
    HeartbeatTimeout { last_heartbeat: Instant },
    ConnectionLost { error: String },
    OrphanExit { exit_code: i32 },
    Other { message: String },
}

// ---------------------------------------------------------------------------
// WorkerRequest / WorkerResponse — FsService ↔ Relay 协议
// ---------------------------------------------------------------------------

/// FsService → WorkerRelay 的请求。
pub struct WorkerRequest {
    /// 请求内容
    pub content: WorkerRequestContent,
    /// 响应通道
    pub response_tx: oneshot::Sender<WorkerResponse>,
}

/// 请求内容。
#[derive(Clone)]
pub enum WorkerRequestContent {
    WatchDir { watch_id: u64, dir: String },
    WatchStat { watch_id: u64, file: String },
    Refresh { watch_id: u64 },
    Unwatch { watch_id: u64 },
    RunCreate { op_id: u64, path: String, kind: EntryKind },
    RunRename { op_id: u64, path: String, new_name: String },
    RunMove { op_id: u64, items: Vec<(String, String)> },
    RunCopy { op_id: u64, items: Vec<(String, String)> },
    CancelOp { op_id: u64 },
}

/// WorkerRelay → FsService 的响应。
pub enum WorkerResponse {
    Ok,
    Err(String),
    /// Worker 尚未连接，调用方稍后重试。
    Connecting,
}

// ---------------------------------------------------------------------------
// CallbackRegistry — 反向回调路由
// ---------------------------------------------------------------------------

/// 把 Worker 反向回调路由到对应 `Watcher` / `Progress` 的通道。
///
/// `watch_id` / `op_id` 全局分配（由 FsService）。
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

    /// 推送 ConnectionLost 到所有注册的 watcher 和 op。
    pub fn notify_connection_lost(&self, reason: DisconnectReason, reconnecting: bool) {
        let reason_str = format!("{:?}", reason);
        {
            let watches = self.watches.lock().unwrap();
            for (watch_id, tx) in watches.iter() {
                let _ = tx.send(WatchDelta::ConnectionLost {
                    watch_id: *watch_id,
                    reason: reason_str.clone(),
                    reconnecting,
                });
            }
        }
        {
            let ops = self.ops.lock().unwrap();
            for (op_id, tx) in ops.iter() {
                let _ = tx.send(ProgressEvent::ConnectionLost {
                    op_id: *op_id,
                    reason: reason_str.clone(),
                    reconnecting,
                });
            }
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
        rx.await.unwrap_or(ConflictResolution::CancelAll)
    }
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
        let _ = self.reaper_tx.send(self.uid);
    }
}

/// UID 访问凭证（RAII）。
///
/// 持有 `request_tx` 和 `registry`，保证操作期间 Worker 存活。
/// 所有该 uid 的 token drop 后，对应 Worker 立即销毁（无宽限期）。
#[derive(Clone)]
pub struct UidToken {
    uid: u32,
    request_tx: mpsc::UnboundedSender<WorkerRequest>,
    /// 共享的 CallbackRegistry，用于注册 watcher/op 回调
    pub registry: Arc<CallbackRegistry>,
    _sentinel: Arc<LeaseSentinel>,
}

impl UidToken {
    /// 目标 UID。
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// 向 WorkerRelay 发送请求并等待响应。
    /// 
    /// 若 Worker 尚未连接（返回 `Connecting`），内部自动等待重试。
    pub async fn send_request(&self, content: WorkerRequestContent) -> Result<WorkerResponse, String> {
        loop {
            let (response_tx, response_rx) = oneshot::channel();
            let request = WorkerRequest {
                content: content.clone(),
                response_tx,
            };

            self.request_tx
                .send(request)
                .map_err(|_| "request channel closed".to_string())?;

            match response_rx.await {
                Ok(WorkerResponse::Connecting) => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Ok(other) => return Ok(other),
                Err(_) => return Err("response dropped".to_string()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RelaySlot — per-UID 中继器槽位
// ---------------------------------------------------------------------------

struct RelaySlot {
    /// FsService → Relay 请求通道
    request_tx: mpsc::UnboundedSender<WorkerRequest>,
    /// 共享回调路由
    registry: Arc<CallbackRegistry>,
    /// 存活 token 的弱引用；`strong_count() == 0` 表示无 token
    sentinel: Weak<LeaseSentinel>,
    /// 子进程 PID（由 WorkerRelay 更新）
    pid: Arc<AtomicU32>,
    /// Relay loop 的 abort handle（用于 kill 时同步）
    _abort_handle: tokio::task::AbortHandle,
}

// ---------------------------------------------------------------------------
// FsWorkerPool
// ---------------------------------------------------------------------------

/// Worker 进程池，按目标 UID 索引。
pub struct FsWorkerPool {
    slots: Arc<Mutex<HashMap<u32, RelaySlot>>>,
    status_tx: mpsc::UnboundedSender<WorkerStatus>,
    status_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<WorkerStatus>>>,
    reaper_tx: mpsc::UnboundedSender<u32>,
}

impl FsWorkerPool {
    /// 创建进程池（必须在 tokio 运行时内调用）。
    pub fn new() -> Self {
        let slots: Arc<Mutex<HashMap<u32, RelaySlot>>> = Arc::new(Mutex::new(HashMap::new()));
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        let reaper_tx = start_reaper(slots.clone(), status_tx.clone());

        Self {
            slots,
            status_tx,
            status_rx: Arc::new(tokio::sync::Mutex::new(status_rx)),
            reaper_tx,
        }
    }

    /// 请求某 uid 的访问凭证。
    ///
    /// 找到/创建对应 WorkerRelay，返回 [`UidToken`]。创建失败时返回错误。
    pub async fn request_token(&self, uid: u32) -> io::Result<UidToken> {
        // 快路径：已有 slot
        {
            let g = self.slots.lock().unwrap();
            if let Some(slot) = g.get(&uid) {
                if let Some(sentinel) = slot.sentinel.upgrade() {
                    return Ok(UidToken {
                        uid,
                        request_tx: slot.request_tx.clone(),
                        registry: slot.registry.clone(),
                        _sentinel: sentinel,
                    });
                }
            }
        }

        // 慢路径：创建新 WorkerRelay（不持锁 await）
        let registry = Arc::new(CallbackRegistry::new());
        let (request_tx, request_rx) = mpsc::unbounded_channel::<WorkerRequest>();
        let pid = Arc::new(AtomicU32::new(0));

        let sentinel = Arc::new(LeaseSentinel {
            uid,
            reaper_tx: self.reaper_tx.clone(),
        });

        let abort_handle = WorkerRelay::spawn(
            uid,
            pid.clone(),
            registry.clone(),
            request_rx,
            self.status_tx.clone(),
            sentinel.clone(),
        );

        let slot = RelaySlot {
            request_tx: request_tx.clone(),
            registry: registry.clone(),
            sentinel: Arc::downgrade(&sentinel),
            pid,
            _abort_handle: abort_handle,
        };

        // 并发双 spawn 检查
        {
            let mut g = self.slots.lock().unwrap();
            if let Some(existing) = g.get(&uid) {
                if let Some(existing_sentinel) = existing.sentinel.upgrade() {
                    warn!("concurrent request_token for uid {uid}, reusing existing slot");
                    return Ok(UidToken {
                        uid,
                        request_tx: existing.request_tx.clone(),
                        registry: existing.registry.clone(),
                        _sentinel: existing_sentinel,
                    });
                }
            }
            g.insert(uid, slot);
        }

        Ok(UidToken {
            uid,
            request_tx,
            registry,
            _sentinel: sentinel,
        })
    }

    /// 获取状态接收端（供上层监听 Worker 连接状态）。
    pub fn status_receiver(&self) -> Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<WorkerStatus>>> {
        self.status_rx.clone()
    }
}

impl Default for FsWorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// WorkerRelay — per-UID async loop
// ---------------------------------------------------------------------------

/// Worker 中继器。在独立 tokio task 中运行，负责：
///
/// 1. spawn fs-worker 子进程
/// 2. 心跳检测（每 0.5s，超时 1s，连续失败 2 次）
/// 3. 消息中继（channel ↔ tarpc）
/// 4. 向上层汇报连接状态
/// 5. 崩溃重启（无次数限制，直到 uidtoken 全部 drop）
struct WorkerRelay;

impl WorkerRelay {
    fn spawn(
        uid: u32,
        pid: Arc<AtomicU32>,
        registry: Arc<CallbackRegistry>,
        request_rx: mpsc::UnboundedReceiver<WorkerRequest>,
        status_tx: mpsc::UnboundedSender<WorkerStatus>,
        sentinel: Arc<LeaseSentinel>,
    ) -> tokio::task::AbortHandle {
        tokio::spawn(async move {
            Self::run_loop(uid, pid, registry, request_rx, status_tx, sentinel).await;
        })
        .abort_handle()
    }

    async fn run_loop(
        uid: u32,
        pid: Arc<AtomicU32>,
        registry: Arc<CallbackRegistry>,
        mut request_rx: mpsc::UnboundedReceiver<WorkerRequest>,
        status_tx: mpsc::UnboundedSender<WorkerStatus>,
        sentinel: Arc<LeaseSentinel>,
    ) {
        let mut attempt: u32 = 0;

        loop {
            if Arc::strong_count(&sentinel) <= 1 {
                info!("no more uidtoken for uid {uid}, shutting down relay");
                break;
            }

            attempt += 1;
            if attempt > 1 {
                let _ = status_tx.send(WorkerStatus::Reconnecting { uid, attempt });
            }

            let fs_worker_id = FS_WORKER_ID.fetch_add(1, Ordering::Relaxed);

            let (child_pid, client) = match Self::spawn_fs_worker(uid, fs_worker_id, registry.clone()).await {
                Ok(v) => v,
                Err(e) => {
                    warn!("failed to spawn fs-worker for uid {uid}: {e}");
                    let reason = DisconnectReason::Other {
                        message: format!("failed to spawn fs-worker: {e}"),
                    };
                    let _ = status_tx.send(WorkerStatus::Disconnected {
                        uid,
                        reason: reason.clone(),
                        reconnecting: true,
                    });
                    registry.notify_connection_lost(reason, true);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            pid.store(child_pid, Ordering::Relaxed);

            // 进入 Connecting 阶段：响应请求立即以 Connecting 应答，心跳只在连接确认后启动
            let _ = status_tx.send(WorkerStatus::Connecting { uid });

            let reason = Self::run_relay(uid, child_pid, &client, &mut request_rx, &status_tx).await;
            pid.store(0, Ordering::Relaxed);

            let _ = status_tx.send(WorkerStatus::Disconnected {
                uid,
                reason: reason.clone(),
                reconnecting: true,
            });
            registry.notify_connection_lost(reason, true);

            kill_fs_worker(uid, child_pid);

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn run_relay(
        uid: u32,
        child_pid: u32,
        client: &FsWorkerServiceClient,
        request_rx: &mut mpsc::UnboundedReceiver<WorkerRequest>,
        status_tx: &mpsc::UnboundedSender<WorkerStatus>,
    ) -> DisconnectReason {
        let mut connected = false;
        let mut tick_interval = tokio::time::interval(Duration::from_millis(500));
        let mut last_heartbeat = Instant::now();
        let mut heartbeat_failures: u32 = 0;

        loop {
            tokio::select! {
                maybe_req = request_rx.recv() => {
                    match maybe_req {
                        Some(request) => {
                            if connected {
                                let client = client.clone();
                                tokio::spawn(async move {
                                    let response = Self::forward_request(&client, request.content).await;
                                    let _ = request.response_tx.send(response);
                                });
                            } else {
                                let _ = request.response_tx.send(WorkerResponse::Connecting);
                            }
                        }
                        None => {
                            return DisconnectReason::Other {
                                message: "request channel closed".into(),
                            };
                        }
                    }
                }

                _ = tick_interval.tick() => {
                    if !Self::is_process_alive(child_pid) {
                        return DisconnectReason::Other {
                            message: "process died, cause unknown".into(),
                        };
                    }

                    if connected {
                        match tokio::time::timeout(
                            Duration::from_secs(1),
                            client.ping(tarpc::context::current()),
                        ).await {
                            Ok(Ok(true)) => {
                                last_heartbeat = Instant::now();
                                heartbeat_failures = 0;
                            }
                            Ok(Ok(false)) => {
                                heartbeat_failures += 1;
                                if heartbeat_failures >= 2 {
                                    return DisconnectReason::HeartbeatTimeout { last_heartbeat };
                                }
                            }
                            Ok(Err(e)) => {
                                heartbeat_failures += 1;
                                if heartbeat_failures >= 2 {
                                    return DisconnectReason::ConnectionLost {
                                        error: format!("heartbeat RPC failed: {e}"),
                                    };
                                }
                            }
                            Err(_) => {
                                heartbeat_failures += 1;
                                if heartbeat_failures >= 2 {
                                    return DisconnectReason::HeartbeatTimeout { last_heartbeat };
                                }
                            }
                        }
                    } else {
                        // 连接中：仅尝试首次 ping，失败不杀进程（pkexec 可能在等待密码）
                        match tokio::time::timeout(
                            Duration::from_secs(2),
                            client.ping(tarpc::context::current()),
                        ).await {
                            Ok(Ok(true)) => {
                                connected = true;
                                last_heartbeat = Instant::now();
                                heartbeat_failures = 0;
                                let _ = status_tx.send(WorkerStatus::Connected { uid, pid: child_pid });
                            }
                            _ => {
                                // 连接尚未建立，继续等待
                            }
                        }
                    }
                }

                status = Self::wait_process(child_pid) => {
                    return status;
                }
            }
        }
    }

    fn is_process_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    async fn wait_process(pid: u32) -> DisconnectReason {
        tokio::task::spawn_blocking(move || {
            let mut status: i32 = 0;
            unsafe { libc::waitpid(pid as i32, &mut status, 0) };
            status
        })
        .await
        .map(|status| {
            if libc::WIFEXITED(status) {
                let code = libc::WEXITSTATUS(status);
                if code == ORPHAN_EXIT_CODE {
                    DisconnectReason::OrphanExit { exit_code: code }
                } else {
                    DisconnectReason::ProcessExited { status: code }
                }
            } else if libc::WIFSIGNALED(status) {
                DisconnectReason::ProcessCrashed {
                    signal: libc::WTERMSIG(status),
                }
            } else {
                DisconnectReason::Other {
                    message: format!("unknown wait status: {status}"),
                }
            }
        })
        .unwrap_or(DisconnectReason::Other {
            message: "waitpid blocking task panicked".into(),
        })
    }

    async fn forward_request(
        client: &FsWorkerServiceClient,
        content: WorkerRequestContent,
    ) -> WorkerResponse {
        let ctx = tarpc::context::current();
        let result = match content {
            WorkerRequestContent::WatchDir { watch_id, dir } => {
                client.watch_dir(ctx, watch_id, dir).await
            }
            WorkerRequestContent::WatchStat { watch_id, file } => {
                client.watch_stat(ctx, watch_id, file).await
            }
            WorkerRequestContent::Refresh { watch_id } => {
                let _ = client.refresh(ctx, watch_id).await;
                Ok(Ok(()))
            }
            WorkerRequestContent::Unwatch { watch_id } => {
                let _ = client.unwatch(ctx, watch_id).await;
                Ok(Ok(()))
            }
            WorkerRequestContent::RunCreate {
                op_id,
                path,
                kind,
            } => client.run_create(ctx, op_id, path, kind).await,
            WorkerRequestContent::RunRename {
                op_id,
                path,
                new_name,
            } => client.run_rename(ctx, op_id, path, new_name).await,
            WorkerRequestContent::RunMove { op_id, items } => {
                client.run_move(ctx, op_id, items).await
            }
            WorkerRequestContent::RunCopy { op_id, items } => {
                client.run_copy(ctx, op_id, items).await
            }
            WorkerRequestContent::CancelOp { op_id } => {
                let _ = client.cancel_op(ctx, op_id).await;
                Ok(Ok(()))
            }
        };

        match result {
            Ok(Ok(())) => WorkerResponse::Ok,
            Ok(Err(e)) => WorkerResponse::Err(e),
            Err(e) => WorkerResponse::Err(format!("RPC error: {e}")),
        }
    }

    /// 启动一个新的 FS Worker 子进程（双 socketpair）。
    async fn spawn_fs_worker(
        target_uid: u32,
        fs_worker_id: u64,
        registry: Arc<CallbackRegistry>,
    ) -> io::Result<(u32, FsWorkerServiceClient)> {
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
            "__fs-worker",
            &format!("--fs-worker-id={fs_worker_id}"),
            &format!("--fd={}", child_req.as_raw_fd()),
            &format!("--cb-fd={}", child_cb.as_raw_fd()),
            &format!("--parent-pid={}", std::process::id()),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        // 3. 启动
        let mut child = cmd.spawn()?;
        let pid = child.id();
        info!(
            "spawned fs-worker pid={pid} fs_worker_id={fs_worker_id} target_uid={target_uid}"
        );

        // 4. 关闭子端副本
        drop(child_req);
        drop(child_cb);

        // 5. 回收子进程（WorkerRelay 通过 wait_process 也监听，这个仅做日志）
        tokio::task::spawn_blocking(move || match child.wait() {
            Ok(status) => info!("fs-worker pid={pid} exited with {status}"),
            Err(e) => warn!("fs-worker pid={pid} wait error: {e}"),
        });

        // 6. parent_req → app→worker tarpc client
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
// Reaper — token 监控
// ---------------------------------------------------------------------------

/// 启动 reaper 后台任务。
///
/// 收到 uid（某 token drop）→ 若该 uid 已无存活 token，立即清理 slot。
fn start_reaper(
    slots: Arc<Mutex<HashMap<u32, RelaySlot>>>,
    status_tx: mpsc::UnboundedSender<WorkerStatus>,
) -> mpsc::UnboundedSender<u32> {
    let (tx, mut rx) = mpsc::unbounded_channel::<u32>();
    tokio::spawn(async move {
        while let Some(uid) = rx.recv().await {
            let to_remove = {
                let g = slots.lock().unwrap();
                match g.get(&uid) {
                    Some(slot) if slot.sentinel.strong_count() == 0 => {
                        let child_pid = slot.pid.load(Ordering::Relaxed);
                        Some(child_pid)
                    }
                    _ => None,
                }
            };

            if let Some(child_pid) = to_remove {
                // 先 kill 子进程（WorkerRelay loop 会自行退出）
                if child_pid > 0 {
                    kill_fs_worker(uid, child_pid);
                }
                slots.lock().unwrap().remove(&uid);
                let _ = status_tx.send(WorkerStatus::Disconnected {
                    uid,
                    reason: DisconnectReason::Other {
                        message: "all tokens dropped".into(),
                    },
                    reconnecting: false,
                });
                info!("reaped worker slot for uid {uid}");
            }
        }
    });
    tx
}

/// SIGTERM 然后（2s 后）SIGKILL 终止 Worker 进程。
fn kill_fs_worker(uid: u32, pid: u32) {
    use nix::sys::signal;
    use nix::unistd::Pid;
    if pid == 0 {
        return;
    }
    let p = Pid::from_raw(pid as i32);
    debug!("killing fs-worker uid={uid} pid={pid}");
    let _ = signal::kill(p, signal::Signal::SIGTERM);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = signal::kill(p, signal::Signal::SIGKILL);
    });
}

// ---------------------------------------------------------------------------
// Worker 孤儿退出码
// ---------------------------------------------------------------------------

/// 孤儿退出码：Worker 检测到主进程消失时以此码退出。
pub const ORPHAN_EXIT_CODE: i32 = 100;

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
