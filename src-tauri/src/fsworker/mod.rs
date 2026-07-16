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
//! 2. kill 子进程 → 延迟 → spawn 新子进程
//! 3. 无次数限制，直到 uidtoken 全部 drop
//!
//! ## 心跳检测
//!
//! - 定时检查子进程存活 + ping tarpc
//! - 超时检测，连续失败 2 次 → 判定崩溃

use std::{
    path::PathBuf,
    sync::atomic::AtomicU64,
    time::{Duration, Instant},
};

use crate::channel::oneshot;
use crate::mesh::types::ui::EntryKind;

// 子模块
pub mod callback;
pub mod platform;
pub mod pool;
pub mod protocol;
pub mod relay;
pub mod worker;

pub use worker::run_fs_worker;

// Re-export pub types from submodules
pub use callback::CallbackRegistry;
pub use platform::ORPHAN_EXIT_CODE;
pub use pool::{FsWorkerPool, LeaseSentinel, UidToken};

/// 全局 fs_worker_id 计数器（仅用于日志区分）。
static FS_WORKER_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

pub(crate) const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
pub(crate) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const SIGKILL_DELAY: Duration = Duration::from_secs(2);
pub(crate) const RESTART_DELAY: Duration = Duration::from_secs(1);
pub(crate) const CONNECTING_RETRY_DELAY: Duration = Duration::from_millis(250);
pub(crate) const FALLBACK_CWD: &str = "/tmp";

// ---------------------------------------------------------------------------
// FsWorkerOpts
// ---------------------------------------------------------------------------

/// FS Worker 启动选项（传递给 `hnfm __fs-worker` 的命令行参数）。
#[derive(Debug, Clone)]
pub struct FsWorkerOpts {
    /// FS Worker ID
    pub fs_worker_id: u64,
    /// 请求通道 fd（app→worker，Worker 作 server）
    pub fd: i32,
    /// 回调通道 fd（worker→app，Worker 作 client）
    pub cb_fd: i32,
    /// 主进程 PID（用于 Worker 侧孤儿检测，绕过 pkexec 中介）。
    pub parent_pid: i32,
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
    pub response_tx: oneshot::TxOneshot<WorkerResponse>,
}

/// 请求内容。
#[derive(Clone)]
pub enum WorkerRequestContent {
    WatchDir {
        watch_id: u64,
        dir: PathBuf,
    },
    WatchStat {
        watch_id: u64,
        file: PathBuf,
    },
    Refresh {
        watch_id: u64,
    },
    Unwatch {
        watch_id: u64,
    },
    RunCreate {
        op_id: u64,
        path: PathBuf,
        kind: EntryKind,
    },
    RunRename {
        op_id: u64,
        path: PathBuf,
        new_name: String,
    },
    RunMove {
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    },
    RunCopy {
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    },
    CancelOp {
        op_id: u64,
    },
    StatVfs {
        path: PathBuf,
    },
    WatchBreadcrumb {
        watch_id: u64,
        path: PathBuf,
    },
}

/// WorkerRelay → FsService 的响应。
pub enum WorkerResponse {
    Ok,
    Err(String),
    /// Worker 尚未连接，调用方稍后重试。
    Connecting,
    /// statvfs 结果。
    StatVfsResult {
        total_bytes: u64,
        free_bytes: u64,
    },
}
