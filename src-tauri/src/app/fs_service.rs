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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::channel;

use crate::fsworker::{FsWorkerPool, UidToken, WorkerRequestContent};
use crate::ipc::protocol::{ConflictResolution, EntryKind, ProgressEvent, WatchDelta};

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
    pub events: channel::RxAsync<WatchDelta>,
    pub(crate) watch_id: u64,
    /// 保活对应 Worker + 发送请求
    pub(crate) _token: UidToken,
}

impl Watcher {
    /// Watcher 的全局唯一 ID。
    pub fn watch_id(&self) -> u64 {
        self.watch_id
    }

    /// 立刻触发全量刷新（对应前端刷新键）。
    pub async fn refresh(&self) {
        let _ = self
            ._token
            .send_request(WorkerRequestContent::Refresh {
                watch_id: self.watch_id,
            })
            .await;
    }

    /// 拆解 Watcher 为部件。消耗 self 避免 Drop 触发 unwatch，
    /// 调用者负责用返回的部件自行管理生命周期。
    pub fn into_parts(self) -> (channel::RxAsync<WatchDelta>, u64, UidToken) {
        use std::mem::ManuallyDrop;
        let me = ManuallyDrop::new(self);
        unsafe {
            let events = std::ptr::read(&me.events);
            let watch_id = me.watch_id;
            let token = std::ptr::read(&me._token);
            (events, watch_id, token)
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self._token.registry.unregister_watch(self.watch_id);
        let token = self._token.clone();
        let id = self.watch_id;
        tokio::spawn(async move {
            let _ = token
                .send_request(WorkerRequestContent::Unwatch { watch_id: id })
                .await;
        });
    }
}

// ---------------------------------------------------------------------------
// Progress — 批处理进度句柄
// ---------------------------------------------------------------------------

/// 一个变更操作的进度句柄。
///
/// - `events` 收到 `Started` / `Item` / `Tick` / `Conflict` / `Done` / `ConnectionLost`。
/// - 遇到 `Conflict { conflict_id, .. }` → 调用 `resolve(conflict_id, ...)` 决策。
/// - `cancel()` 取消整个操作。
/// - 持有相关 [`UidToken`]，操作期间保活 Worker。
pub struct Progress {
    /// 进度事件流
    pub events: channel::RxAsync<ProgressEvent>,
    op_id: u64,
    /// 保活相关 Worker + 发送请求
    _token: UidToken,
}

impl Progress {
    /// 解决一个冲突。
    pub fn resolve(&self, conflict_id: u64, res: ConflictResolution) {
        self._token
            .registry
            .resolve_conflict(self.op_id, conflict_id, res);
    }

    /// 取消整个操作。
    pub async fn cancel(&self) {
        let _ = self
            ._token
            .send_request(WorkerRequestContent::CancelOp { op_id: self.op_id })
            .await;
    }

    /// 操作 ID（供上层归组/日志）。
    pub fn op_id(&self) -> u64 {
        self.op_id
    }

    /// 取出一个可跨任务持有的取消句柄。
    pub fn canceller(&self) -> Canceller {
        Canceller {
            token: self._token.clone(),
            op_id: self.op_id,
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self._token.registry.unregister_op(self.op_id);
    }
}

/// 与 `Progress` 解耦的取消句柄，可克隆、可跨任务持有。
#[derive(Clone)]
pub struct Canceller {
    token: UidToken,
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
            .token
            .send_request(WorkerRequestContent::CancelOp { op_id: self.op_id })
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
    pub(crate) async fn try_request_uid_token(&self, uid: u32) -> Result<UidToken, String> {
        self.pool
            .request_token(uid)
            .await
            .map_err(|e| format!("request fs-worker for uid {uid}: {e}"))
    }

    // -----------------------------------------------------------------------
    // Watch
    // -----------------------------------------------------------------------

    /// 监视目录。立刻返回 Watcher；首帧全量，之后增量。
    pub(crate) async fn watch_dir(&self, token: &UidToken, dir: &Path) -> Result<Watcher, String> {
        let watch_id = self.next_id();
        let rx = token.registry.register_watch(watch_id);
        match token
            .send_request(WorkerRequestContent::WatchDir {
                watch_id,
                dir: dir.to_path_buf(),
            })
            .await
        {
            Ok(crate::fsworker::WorkerResponse::Ok) => Ok(Watcher {
                events: rx,
                watch_id,
                _token: token.clone(),
            }),
            Ok(crate::fsworker::WorkerResponse::Err(e)) => {
                token.registry.unregister_watch(watch_id);
                Err(e)
            }
            Ok(_) => unreachable!("Connecting handled internally by send_request"),
            Err(e) => {
                token.registry.unregister_watch(watch_id);
                Err(e)
            }
        }
    }

    /// 监视单个文件/目录的属性（面包屑用）。
    pub(crate) async fn watch_stat(
        &self,
        token: &UidToken,
        file: &Path,
    ) -> Result<Watcher, String> {
        let watch_id = self.next_id();
        let rx = token.registry.register_watch(watch_id);
        match token
            .send_request(WorkerRequestContent::WatchStat {
                watch_id,
                file: file.to_path_buf(),
            })
            .await
        {
            Ok(crate::fsworker::WorkerResponse::Ok) => Ok(Watcher {
                events: rx,
                watch_id,
                _token: token.clone(),
            }),
            Ok(crate::fsworker::WorkerResponse::Err(e)) => {
                token.registry.unregister_watch(watch_id);
                Err(e)
            }
            Ok(_) => unreachable!("Connecting handled internally by send_request"),
            Err(e) => {
                token.registry.unregister_watch(watch_id);
                Err(e)
            }
        }
    }

    /// 监听面包屑路径段信息（home/mount 判断）。
    /// 首帧立即推送，后续 /proc/mounts 变化时重推。
    pub(crate) async fn watch_breadcrumb(
        &self,
        token: &UidToken,
        path: &Path,
    ) -> Result<Watcher, String> {
        let watch_id = self.next_id();
        let rx = token.registry.register_watch(watch_id);
        match token
            .send_request(WorkerRequestContent::WatchBreadcrumb {
                watch_id,
                path: path.to_path_buf(),
            })
            .await
        {
            Ok(crate::fsworker::WorkerResponse::Ok) => Ok(Watcher {
                events: rx,
                watch_id,
                _token: token.clone(),
            }),
            Ok(crate::fsworker::WorkerResponse::Err(e)) => {
                token.registry.unregister_watch(watch_id);
                Err(e)
            }
            Ok(_) => unreachable!("Connecting handled internally by send_request"),
            Err(e) => {
                token.registry.unregister_watch(watch_id);
                Err(e)
            }
        }
    }

    // -----------------------------------------------------------------------
    // 变更操作（一律返回 Progress）
    // -----------------------------------------------------------------------

    /// 创建文件或目录。
    pub(crate) async fn create(
        &self,
        token: &UidToken,
        path: &Path,
        kind: EntryKind,
    ) -> Result<Progress, String> {
        let op_id = self.next_id();
        let rx = token.registry.register_op(op_id);
        let rpc = token
            .send_request(WorkerRequestContent::RunCreate {
                op_id,
                path: path.to_path_buf(),
                kind,
            })
            .await;
        Self::finish_dispatch(rpc, token, rx, op_id)
    }

    /// 重命名。
    pub(crate) async fn rename(
        &self,
        token: &UidToken,
        path: &Path,
        new_name: &str,
    ) -> Result<Progress, String> {
        let op_id = self.next_id();
        let rx = token.registry.register_op(op_id);
        let rpc = token
            .send_request(WorkerRequestContent::RunRename {
                op_id,
                path: path.to_path_buf(),
                new_name: new_name.to_string(),
            })
            .await;
        Self::finish_dispatch(rpc, token, rx, op_id)
    }

    /// 批量移动。
    pub(crate) async fn move_(&self, ops: Vec<Op>) -> Result<Progress, String> {
        self.run_batch(ops, true).await
    }

    /// 批量复制。
    pub(crate) async fn copy(&self, ops: Vec<Op>) -> Result<Progress, String> {
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

        let token = ops[0].src.0.clone();
        let items: Vec<(PathBuf, PathBuf)> = ops
            .iter()
            .map(|o| (o.src.1.clone(), o.dst.1.clone()))
            .collect();
        let op_id = self.next_id();
        let rx = token.registry.register_op(op_id);
        let content = if is_move {
            WorkerRequestContent::RunMove { op_id, items }
        } else {
            WorkerRequestContent::RunCopy { op_id, items }
        };
        let rpc = token.send_request(content).await;
        Self::finish_dispatch(rpc, &token, rx, op_id)
    }

    /// 统一处理派发结果：成功建 Progress，失败注销 op。
    fn finish_dispatch(
        rpc: Result<crate::fsworker::WorkerResponse, String>,
        token: &UidToken,
        rx: channel::RxAsync<ProgressEvent>,
        op_id: u64,
    ) -> Result<Progress, String> {
        match rpc {
            Ok(crate::fsworker::WorkerResponse::Ok) => Ok(Progress {
                events: rx,
                op_id,
                _token: token.clone(),
            }),
            Ok(crate::fsworker::WorkerResponse::Err(e)) => {
                token.registry.unregister_op(op_id);
                Err(e)
            }
            Ok(_) => unreachable!("Connecting handled internally by send_request"),
            Err(e) => {
                token.registry.unregister_op(op_id);
                Err(e)
            }
        }
    }
}

impl Default for FsService {
    fn default() -> Self {
        Self::new()
    }
}
