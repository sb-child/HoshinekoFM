//! FS Worker RPC 服务实现。
//!
//! [`FsWorkerServer`] 实现 [`FsWorkerService`] trait，
//! 通过 [`WatchPool`] 管理目录/文件监视，通过回调通道推送增量/进度/冲突。
//!
//! 所有方法立刻返回（仅表示派发是否被接受），真正的结果／增量经反向的 `AppCallbackService` 推送。

use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use tarpc::context;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::ipc::protocol::{
    AppCallbackServiceClient, EntryKind, FsWorkerService, ProgressEvent, WatchDelta,
};

use super::{
    files::ProceedStrategy,
    ops::{decide_dst, finish_op, run_batch, BatchKind},
    watch::WatchPool,
};

/// 崩溃诊断用：最后一个 watch_id / 最后一个操作名
pub(crate) static LAST_WATCH_ID: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAST_OPERATION: AtomicU64 = AtomicU64::new(0);
// 操作编码：1=watch_dir 2=unwatch 3=watch_stat 4=refresh 5=run_create 6=run_rename 7=run_move 8=run_copy 9=cancel_op 10=stat_vfs
macro_rules! set_last_op {
    ($code:expr, $wid:expr) => {
        LAST_OPERATION.store($code, std::sync::atomic::Ordering::Relaxed);
        LAST_WATCH_ID.store($wid, std::sync::atomic::Ordering::Relaxed);
    };
}


/// FS Worker 服务实现。
#[derive(Clone)]
pub struct FsWorkerServer {
    pub fs_worker_id: u64,
    cb: AppCallbackServiceClient,
    pool: Arc<WatchPool>,
    /// watch_id → canonical PathBuf（用于 unwatch 时从 pool 注销）
    watch_canonical: Arc<Mutex<HashMap<u64, PathBuf>>>,
    /// op_id → 取消标志
    ops: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    /// 冲突 ID 分配
    conflict_seq: Arc<AtomicU64>,
}

impl FsWorkerServer {
    pub fn new(fs_worker_id: u64, cb: AppCallbackServiceClient) -> Self {
        let uid = nix::unistd::getuid().as_raw();
        info!("fs worker {} starting, uid={}", fs_worker_id, uid);
        Self {
            fs_worker_id,
            cb,
            pool: Arc::new(WatchPool::new()),
            watch_canonical: Arc::new(Mutex::new(HashMap::new())),
            ops: Arc::new(Mutex::new(HashMap::new())),
            conflict_seq: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl FsWorkerService for FsWorkerServer {
    async fn ping(self, _ctx: context::Context) -> bool {
        true
    }

    // -----------------------------------------------------------------------
    // Watch
    // -----------------------------------------------------------------------

    async fn watch_dir(
        self,
        _ctx: context::Context,
        watch_id: u64,
        path: PathBuf,
    ) -> Result<(), String> {
        set_last_op!(1, watch_id);
        debug!("[w{}] watch_dir {watch_id} {path:?}", self.fs_worker_id);

        let shared = self.pool.register(watch_id, path.clone(), true, self.cb.clone()).await;
        self.watch_canonical.lock().await.insert(watch_id, shared.target.clone());

        Ok(())
    }

    async fn watch_stat(
        self,
        _ctx: context::Context,
        watch_id: u64,
        path: PathBuf,
    ) -> Result<(), String> {
        set_last_op!(3, watch_id);
        debug!("[w{}] watch_stat {watch_id} {path:?}", self.fs_worker_id);

        let shared = self.pool.register(watch_id, path.clone(), false, self.cb.clone()).await;
        self.watch_canonical.lock().await.insert(watch_id, shared.target.clone());

        Ok(())
    }

    async fn refresh(self, _ctx: context::Context, watch_id: u64) {
        set_last_op!(4, watch_id);
        debug!("[w{}] refresh {watch_id}", self.fs_worker_id);
        let canonical = self.watch_canonical.lock().await.get(&watch_id).cloned();
        if canonical.is_none() {
            return;
        }
        let cb = self.cb.clone();
        tokio::spawn(async move {
            let _ = cb.watch_delta(context::current(), watch_id, WatchDelta::Reset(Vec::new())).await;
        });
    }

    async fn unwatch(self, _ctx: context::Context, watch_id: u64) {
        set_last_op!(2, watch_id);
        debug!("[w{}] unwatch {watch_id}", self.fs_worker_id);
        let canonical = self.watch_canonical.lock().await.remove(&watch_id);
        if let Some(c) = canonical {
            self.pool.unregister(watch_id, &c).await;
        }
    }

    // -----------------------------------------------------------------------
    // 变更操作
    // -----------------------------------------------------------------------

    async fn run_create(
        self,
        _ctx: context::Context,
        op_id: u64,
        path: PathBuf,
        kind: EntryKind,
    ) -> Result<(), String> {
        set_last_op!(5, op_id);
        let _cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        tokio::spawn(async move {
            let _ = cb
                .progress(context::current(), op_id, ProgressEvent::Started { total: 1 })
                .await;
            let mut succeeded = 0u64;
            let mut failed = 0u64;
            let mut cancelled = false;

            match decide_dst(&cb, op_id, &conflict_seq, &path, &path).await {
                Decision::Cancel => cancelled = true,
                Decision::Skip => {
                    let _ = cb
                        .progress(
                            context::current(),
                            op_id,
                            ProgressEvent::Item {
                                src: path.clone(),
                                dst: path.clone(),
                                status: crate::ipc::protocol::ItemStatus::Skipped,
                            },
                        )
                        .await;
                }
                Decision::Proceed { dst, .. } => {
                    let res =
                        tokio::task::spawn_blocking(move || {
                            super::files::create_entry(&dst, kind)
                        })
                        .await
                        .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
                    let status = match res {
                        Ok(()) => {
                            succeeded += 1;
                            crate::ipc::protocol::ItemStatus::Ok
                        }
                        Err(e) => {
                            failed += 1;
                            crate::ipc::protocol::ItemStatus::Failed(e.to_string())
                        }
                    };
                    let _ = cb
                        .progress(
                            context::current(),
                            op_id,
                            ProgressEvent::Item {
                                src: path.clone(),
                                dst: path.clone(),
                                status,
                            },
                        )
                        .await;
                }
            }
            finish_op(&cb, &ops, op_id, succeeded, failed, cancelled).await;
        });
        Ok(())
    }

    async fn run_rename(
        self,
        _ctx: context::Context,
        op_id: u64,
        path: PathBuf,
        new_name: String,
    ) -> Result<(), String> {
        set_last_op!(6, op_id);
        let _cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        tokio::spawn(async move {
            let _ = cb
                .progress(context::current(), op_id, ProgressEvent::Started { total: 1 })
                .await;
            let parent = path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"));
            let dst = parent.join(&new_name);
            let mut succeeded = 0u64;
            let mut failed = 0u64;
            let mut cancelled = false;

            match decide_dst(&cb, op_id, &conflict_seq, &path, &dst).await {
                Decision::Cancel => cancelled = true,
                Decision::Skip => {
                    let _ = cb
                        .progress(
                            context::current(),
                            op_id,
                            ProgressEvent::Item {
                                src: path.clone(),
                                dst: dst.clone(),
                                status: crate::ipc::protocol::ItemStatus::Skipped,
                            },
                        )
                        .await;
                }
                Decision::Proceed {
                    dst: final_dst,
                    strategy,
                } => {
                    let renamed = final_dst != dst;
                    let s = path.clone();
                    let d = final_dst.clone();
                    let res = super::files::rename_with_strategy(&s, &d, strategy);
                    let status = match res {
                        Ok(()) => {
                            succeeded += 1;
                            if renamed {
                                crate::ipc::protocol::ItemStatus::Renamed(final_dst.clone())
                            } else {
                                crate::ipc::protocol::ItemStatus::Ok
                            }
                        }
                        Err(e)
                            if strategy != ProceedStrategy::Overwrite
                                && e.raw_os_error()
                                    == Some(nix::errno::Errno::EEXIST as i32) =>
                        {
                            let s2 = s.clone();
                            let d2 = final_dst.clone();
                            match decide_dst(&cb, op_id, &conflict_seq, &s2, &d2).await {
                                Decision::Proceed {
                                    dst: fd,
                                    strategy: st2,
                                } => {
                                    match super::files::rename_with_strategy(&s2, &fd, st2) {
                                        Ok(()) => {
                                            succeeded += 1;
                                            if fd != d2 {
                                                crate::ipc::protocol::ItemStatus::Renamed(fd)
                                            } else {
                                                crate::ipc::protocol::ItemStatus::Ok
                                            }
                                        }
                                        Err(e2) => {
                                            failed += 1;
                                            crate::ipc::protocol::ItemStatus::Failed(
                                                e2.to_string(),
                                            )
                                        }
                                    }
                                }
                                Decision::Skip => crate::ipc::protocol::ItemStatus::Skipped,
                                Decision::Cancel => {
                                    cancelled = true;
                                    crate::ipc::protocol::ItemStatus::Skipped
                                }
                            }
                        }
                        Err(e) => {
                            failed += 1;
                            crate::ipc::protocol::ItemStatus::Failed(e.to_string())
                        }
                    };
                    let _ = cb
                        .progress(
                            context::current(),
                            op_id,
                            ProgressEvent::Item {
                                src: path.clone(),
                                dst: final_dst,
                                status,
                            },
                        )
                        .await;
                }
            }
            finish_op(&cb, &ops, op_id, succeeded, failed, cancelled).await;
        });
        Ok(())
    }

    async fn run_move(
        self,
        _ctx: context::Context,
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), String> {
        set_last_op!(7, op_id);
        let cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        tokio::spawn(async move {
            run_batch(&cb, &ops, op_id, cancel, &conflict_seq, items, BatchKind::Move).await;
        });
        Ok(())
    }

    async fn run_copy(
        self,
        _ctx: context::Context,
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), String> {
        set_last_op!(8, op_id);
        let cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        tokio::spawn(async move {
            run_batch(&cb, &ops, op_id, cancel, &conflict_seq, items, BatchKind::Copy).await;
        });
        Ok(())
    }

    async fn cancel_op(self, _ctx: context::Context, op_id: u64) {
        set_last_op!(9, op_id);
        if let Some(flag) = self.ops.lock().await.get(&op_id) {
            flag.store(true, Ordering::Relaxed);
            debug!("[w{}] cancel op {op_id}", self.fs_worker_id);
        }
    }

    async fn stat_vfs(self, _ctx: context::Context, path: PathBuf) -> Result<(u64, u64), String> {
        set_last_op!(10, 0);
        let vfs = nix::sys::statvfs::statvfs(&path)
            .map_err(|e| format!("statvfs {}: {e}", path.display()))?;
        let block_size = vfs.block_size() as u64;
        Ok((vfs.blocks() * block_size, vfs.blocks_free() * block_size))
    }
}

impl FsWorkerServer {
    async fn register_op(&self, op_id: u64) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.ops.lock().await.insert(op_id, flag.clone());
        flag
    }
}

use super::ops::Decision;
