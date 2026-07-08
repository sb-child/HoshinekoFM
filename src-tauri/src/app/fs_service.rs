//! 文件系统服务层（纯调度）。
//!
//! FsService 不感知 tab/window，不持有业务数据。它只做四件事：
//!
//! 1. 分发 [`UidToken`]（RAII 凭证，鼓励上层长期持有）。
//! 2. 出借 [`Watcher`]（订阅目录/文件变化，首帧全量 + 后续增量）。
//! 3. 出借 [`Progress`]（所有变更操作立刻返回，进度/冲突经回调汇报）。
//! 4. 内部管理 Worker 进程池 + reaper + 反向回调路由（见 [`crate::fsworker`]）。
//!
//! 所有变更操作（create/rename/move/copy）都**假设可能很慢**（软盘/NFS/坏块），
//! 因此一律立刻返回 `Progress`，绝不同步阻塞。
//!
//! 按 tab/window 归组任务、忙检查、取消的语义由上层 UIService 负责。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tarpc::context;
use tokio::sync::mpsc;

use crate::fsworker::{FsWorkerPool, UidToken, WorkerConn};
use crate::ipc::protocol::{ConflictResolution, EntryKind, ProgressEvent, WatchDelta};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Op — copy/move 的单项（逐项携带 src/dst token，可表达跨 UID）
// ---------------------------------------------------------------------------

/// 一个 copy/move 项：从 `src` 到 `dst`，各自携带对应 UID 的凭证。
pub struct Op {
    /// 源：(凭证, 路径)
    pub src: (UidToken, PathBuf),
    /// 目标：(凭证, 路径)
    pub dst: (UidToken, PathBuf),
}

// ---------------------------------------------------------------------------
// Watcher — 目录/文件订阅句柄
// ---------------------------------------------------------------------------

/// 目录或文件的变化订阅。
///
/// - `events` 首次 recv 得到 `WatchDelta::Reset`（全量），之后为增量。
/// - `refresh()` 立刻触发一次全量刷新。
/// - drop 时自动 `unwatch`，并释放对应 Worker 引用。
pub struct Watcher {
    /// 增量事件流
    pub events: mpsc::UnboundedReceiver<WatchDelta>,
    watch_id: u64,
    conn: Arc<WorkerConn>,
    /// 保活对应 Worker
    _token: UidToken,
}

impl Watcher {
    /// 立刻触发全量刷新（对应前端刷新键）。
    pub async fn refresh(&self) {
        let _ = self
            .conn
            .client
            .refresh(context::current(), self.watch_id)
            .await;
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.conn.registry.unregister_watch(self.watch_id);
        let client = self.conn.client.clone();
        let id = self.watch_id;
        tokio::spawn(async move {
            let _ = client.unwatch(context::current(), id).await;
        });
    }
}

// ---------------------------------------------------------------------------
// Progress — 批处理进度句柄
// ---------------------------------------------------------------------------

/// 一个变更操作的进度句柄。
///
/// - `events` 收到 `Started` / `Item` / `Tick` / `Conflict` / `Done`。
/// - 遇到 `Conflict { conflict_id, .. }` → 调用 `resolve(conflict_id, ...)` 决策。
/// - `cancel()` 取消整个操作。
/// - 持有相关 [`UidToken`]，操作期间保活 Worker（"Progress 在 uid token 锁覆盖范围内"）。
pub struct Progress {
    /// 进度事件流
    pub events: mpsc::UnboundedReceiver<ProgressEvent>,
    op_id: u64,
    conn: Arc<WorkerConn>,
    /// 保活相关 Worker
    _tokens: Vec<UidToken>,
}

impl Progress {
    /// 解决一个冲突。
    pub fn resolve(&self, conflict_id: u64, res: ConflictResolution) {
        self.conn
            .registry
            .resolve_conflict(self.op_id, conflict_id, res);
    }

    /// 取消整个操作。
    pub async fn cancel(&self) {
        let _ = self
            .conn
            .client
            .cancel_op(context::current(), self.op_id)
            .await;
    }

    /// 操作 ID（供上层归组/日志）。
    pub fn op_id(&self) -> u64 {
        self.op_id
    }

    /// 取出一个可跨任务持有的取消句柄。
    pub fn canceller(&self) -> Canceller {
        Canceller {
            conn: self.conn.clone(),
            op_id: self.op_id,
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.conn.registry.unregister_op(self.op_id);
    }
}

/// 与 `Progress` 解耦的取消句柄，可克隆、可跨任务持有（例如放进 UIService 的注册表）。
#[derive(Clone)]
pub struct Canceller {
    conn: Arc<WorkerConn>,
    op_id: u64,
}

impl Canceller {
    /// 操作 ID。
    pub fn op_id(&self) -> u64 {
        self.op_id
    }

    /// 取消该操作。
    pub async fn cancel(&self) {
        let _ = self
            .conn
            .client
            .cancel_op(context::current(), self.op_id)
            .await;
    }
}

// ---------------------------------------------------------------------------
// FsService
// ---------------------------------------------------------------------------

/// 文件系统调度服务。
pub struct FsService {
    pool: FsWorkerPool,
    id_seq: AtomicU64,
}

impl FsService {
    /// 创建（必须在 tokio 运行时内，会启动 reaper）。
    pub fn new() -> Self {
        Self {
            pool: FsWorkerPool::new(),
            id_seq: AtomicU64::new(1),
        }
    }

    fn next_id(&self) -> u64 {
        self.id_seq.fetch_add(1, Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // Token
    // -----------------------------------------------------------------------

    /// 请求某 uid 的访问凭证。找不到/创建失败（如 pkexec 未通过）时返回错误。
    pub async fn try_request_uid_token(&self, uid: u32) -> Result<UidToken, String> {
        self.pool
            .request_token(uid)
            .await
            .map_err(|e| format!("request worker for uid {uid}: {e}"))
    }

    // -----------------------------------------------------------------------
    // Watch
    // -----------------------------------------------------------------------

    /// 监视目录。立刻返回 Watcher；首帧全量，之后增量。
    pub async fn watch_dir(&self, token: &UidToken, dir: &str) -> Result<Watcher, String> {
        let watch_id = self.next_id();
        let conn = token.conn().clone();
        let rx = conn.registry.register_watch(watch_id);
        match conn
            .client
            .watch_dir(context::current(), watch_id, dir.to_string())
            .await
        {
            Ok(Ok(())) => Ok(Watcher {
                events: rx,
                watch_id,
                conn,
                _token: token.clone(),
            }),
            Ok(Err(e)) => {
                conn.registry.unregister_watch(watch_id);
                Err(e)
            }
            Err(e) => {
                conn.registry.unregister_watch(watch_id);
                Err(format!("watch_dir RPC: {e}"))
            }
        }
    }

    /// 监视单个文件/目录的属性（面包屑用）。
    pub async fn watch_stat(&self, token: &UidToken, file: &str) -> Result<Watcher, String> {
        let watch_id = self.next_id();
        let conn = token.conn().clone();
        let rx = conn.registry.register_watch(watch_id);
        match conn
            .client
            .watch_stat(context::current(), watch_id, file.to_string())
            .await
        {
            Ok(Ok(())) => Ok(Watcher {
                events: rx,
                watch_id,
                conn,
                _token: token.clone(),
            }),
            Ok(Err(e)) => {
                conn.registry.unregister_watch(watch_id);
                Err(e)
            }
            Err(e) => {
                conn.registry.unregister_watch(watch_id);
                Err(format!("watch_stat RPC: {e}"))
            }
        }
    }

    // -----------------------------------------------------------------------
    // 变更操作（一律返回 Progress）
    // -----------------------------------------------------------------------

    /// 创建文件或目录。
    pub async fn create(
        &self,
        token: &UidToken,
        path: &str,
        kind: EntryKind,
    ) -> Result<Progress, String> {
        let op_id = self.next_id();
        let conn = token.conn().clone();
        let rx = conn.registry.register_op(op_id);
        let rpc = conn
            .client
            .run_create(context::current(), op_id, path.to_string(), kind)
            .await;
        Self::finish_dispatch(rpc, conn, rx, op_id, vec![token.clone()])
    }

    /// 重命名。
    pub async fn rename(
        &self,
        token: &UidToken,
        path: &str,
        new_name: &str,
    ) -> Result<Progress, String> {
        let op_id = self.next_id();
        let conn = token.conn().clone();
        let rx = conn.registry.register_op(op_id);
        let rpc = conn
            .client
            .run_rename(context::current(), op_id, path.to_string(), new_name.to_string())
            .await;
        Self::finish_dispatch(rpc, conn, rx, op_id, vec![token.clone()])
    }

    /// 批量移动。
    pub async fn move_(&self, ops: Vec<Op>) -> Result<Progress, String> {
        self.run_batch(ops, true).await
    }

    /// 批量复制。
    pub async fn copy(&self, ops: Vec<Op>) -> Result<Progress, String> {
        self.run_batch(ops, false).await
    }

    async fn run_batch(&self, ops: Vec<Op>, is_move: bool) -> Result<Progress, String> {
        if ops.is_empty() {
            return Err("empty ops".into());
        }
        // v1：要求所有项同一 UID（跨 UID 待实现）
        let uid = ops[0].src.0.uid();
        let all_same = ops
            .iter()
            .all(|o| o.src.0.uid() == uid && o.dst.0.uid() == uid);
        if !all_same {
            todo!("跨 UID copy/move 尚未实现");
        }

        let conn = ops[0].src.0.conn().clone();
        let items: Vec<(String, String)> = ops
            .iter()
            .map(|o| {
                (
                    o.src.1.to_string_lossy().into_owned(),
                    o.dst.1.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let mut tokens = Vec::with_capacity(ops.len() * 2);
        for o in ops {
            tokens.push(o.src.0);
            tokens.push(o.dst.0);
        }

        let op_id = self.next_id();
        let rx = conn.registry.register_op(op_id);
        let rpc = if is_move {
            conn.client.run_move(context::current(), op_id, items).await
        } else {
            conn.client.run_copy(context::current(), op_id, items).await
        };
        Self::finish_dispatch(rpc, conn, rx, op_id, tokens)
    }

    /// 统一处理派发结果：成功建 Progress，失败注销 op。
    fn finish_dispatch(
        rpc: Result<Result<(), String>, tarpc::client::RpcError>,
        conn: Arc<WorkerConn>,
        rx: mpsc::UnboundedReceiver<ProgressEvent>,
        op_id: u64,
        tokens: Vec<UidToken>,
    ) -> Result<Progress, String> {
        match rpc {
            Ok(Ok(())) => Ok(Progress {
                events: rx,
                op_id,
                conn,
                _tokens: tokens,
            }),
            Ok(Err(e)) => {
                conn.registry.unregister_op(op_id);
                Err(e)
            }
            Err(e) => {
                conn.registry.unregister_op(op_id);
                Err(format!("dispatch RPC: {e}"))
            }
        }
    }
}

impl Default for FsService {
    fn default() -> Self {
        Self::new()
    }
}
