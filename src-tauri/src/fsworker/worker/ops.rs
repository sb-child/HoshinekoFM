//! 变更操作：create/rename/move/copy + 批处理 + 冲突决策。
//!
//! 所有操作立刻返回，进度经回调通道推送。

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use tracing::warn;

use tarpc::context;
use tokio::sync::Mutex;

use crate::fsworker::protocol::{
    AppCallbackServiceClient, ConflictItem, ConflictResolution, ItemStatus, ProgressEvent,
};

use super::files::{ProceedStrategy, copy_path, move_path, move_path_noreplace, unique_path};

// --
// 批处理执行
// --

#[derive(Clone, Copy)]
pub enum BatchKind {
    Move,
    Copy,
}

pub struct BatchConfig {
    pub cb: Arc<AppCallbackServiceClient>,
    pub ops: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    pub op_id: u64,
    pub cancel: Arc<AtomicBool>,
    pub conflict_seq: Arc<AtomicU64>,
    pub items: Vec<(PathBuf, PathBuf)>,
    pub kind: BatchKind,
}

/// 执行批处理操作（move 或 copy）。每个条目均需检查冲突。
pub async fn run_batch(config: BatchConfig) {
    let total = config.items.len() as u64;
    let _ = config.cb
        .progress(context::current(), config.op_id, ProgressEvent::Started { total })
        .await;

    let mut succeeded = 0u64;
    let mut failed = 0u64;
    let mut cancelled = false;
    let mut done = 0u64;

    for (src, dst) in config.items {
        if config.cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        done += 1;

        let status = match decide_dst(&config.cb, config.op_id, &config.conflict_seq, &src, &dst).await {
            Decision::Cancel => {
                cancelled = true;
                break;
            }
            Decision::Skip => ItemStatus::Skipped,
            Decision::Proceed {
                dst: final_dst,
                strategy,
            } => {
                let renamed = final_dst != dst;
                let s = src.clone();
                let d = final_dst.clone();
                let kind = config.kind;
                let res = tokio::task::spawn_blocking(move || {
                    let _span = tracing::info_span!("ops::run_batch_move_copy").entered();
                    match kind {
                    BatchKind::Move => {
                        if strategy == ProceedStrategy::Overwrite {
                            move_path(&s, &d)
                        } else {
                            move_path_noreplace(&s, &d)
                        }
                    }
                    BatchKind::Copy => copy_path(&s, &d, strategy != ProceedStrategy::Overwrite),
                    }
                })
                .await
                .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
                match res {
                    Ok(()) => {
                        succeeded += 1;
                        if renamed {
                            ItemStatus::Renamed(final_dst)
                        } else {
                            ItemStatus::Ok
                        }
                    }
                    Err(e)
                        if strategy != ProceedStrategy::Overwrite
                            && matches!(kind, BatchKind::Move)
                            && e.raw_os_error() == Some(nix::errno::Errno::EEXIST as i32) =>
                    {
                        // TOCTOU: 重问
                        let d2 = final_dst.clone();
                        match decide_dst(&config.cb, config.op_id, &config.conflict_seq, &src, &d2).await {
                            Decision::Proceed {
                                dst: fd,
                                strategy: st2,
                            } => {
                                let s2 = src.clone();
                                let fd_move = fd.clone();
                                let res2 = tokio::task::spawn_blocking(move || {
                                    let _span = tracing::info_span!("ops::run_batch_toctou_retry").entered();
                                    if st2 == ProceedStrategy::Overwrite {
                                        move_path(&s2, &fd_move)
                                    } else {
                                        move_path_noreplace(&s2, &fd_move)
                                    }
                                })
                                .await
                                .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
                                match res2 {
                                    Ok(()) => {
                                        succeeded += 1;
                                        if fd != d2 {
                                            ItemStatus::Renamed(fd)
                                        } else {
                                            ItemStatus::Ok
                                        }
                                    }
                                    Err(e2) => {
                                        failed += 1;
                                        ItemStatus::Failed(e2.to_string())
                                    }
                                }
                            }
                            Decision::Skip => ItemStatus::Skipped,
                            Decision::Cancel => {
                                cancelled = true;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        ItemStatus::Failed(e.to_string())
                    }
                }
            }
        };

        let _ = config.cb
            .progress(
                context::current(),
                config.op_id,
                ProgressEvent::Item {
                    src: src.clone(),
                    dst: dst.clone(),
                    status,
                },
            )
            .await;
        let _ = config.cb
            .progress(
                context::current(),
                config.op_id,
                ProgressEvent::Tick {
                    done,
                    total,
                    current: src,
                },
            )
            .await;
    }

    finish_op(&config.cb, &config.ops, config.op_id, succeeded, failed, cancelled).await;
}

// --
// 冲突决策
// --

pub enum Decision {
    Proceed {
        dst: PathBuf,
        strategy: ProceedStrategy,
    },
    Skip,
    Cancel,
}

/// 若 `dst` 已存在则询问上层，返回最终决策。
pub async fn decide_dst(
    cb: &AppCallbackServiceClient,
    op_id: u64,
    conflict_seq: &Arc<AtomicU64>,
    src: &Path,
    dst: &Path,
) -> Decision {
    let dst_buf = dst.to_path_buf();
    let exists = tokio::task::spawn_blocking(move || {
        let _span = tracing::info_span!("ops::decide_dst_exists").entered();
        dst_buf.exists()
    })
        .await
        .unwrap_or(false);
    if !exists {
        return Decision::Proceed {
            dst: dst.to_path_buf(),
            strategy: ProceedStrategy::NoConflict,
        };
    }
    let conflict_id = conflict_seq.fetch_add(1, Ordering::Relaxed);
    let item = ConflictItem {
        src: src.to_path_buf(),
        dst: dst.to_path_buf(),
    };
    let res = cb
        .ask_conflict(long_ctx(), op_id, conflict_id, item)
        .await
        .unwrap_or(ConflictResolution::CancelAll);
    match res {
        ConflictResolution::Skip => Decision::Skip,
        ConflictResolution::Overwrite => Decision::Proceed {
            dst: dst.to_path_buf(),
            strategy: ProceedStrategy::Overwrite,
        },
        ConflictResolution::AutoRename => Decision::Proceed {
            dst: unique_path(dst),
            strategy: ProceedStrategy::Rename,
        },
        ConflictResolution::Rename(name) => {
            let parent = dst
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/"));
            Decision::Proceed {
                dst: parent.join(name),
                strategy: ProceedStrategy::Rename,
            }
        }
        ConflictResolution::CancelAll => Decision::Cancel,
    }
}

/// tarpc 长超时上下文（用于阻塞式 ask_conflict，等待用户决策）。
fn long_ctx() -> context::Context {
    let mut c = context::current();
    c.deadline = std::time::Instant::now() + Duration::from_secs(3600);
    c
}

/// 结束操作：推 Done、注销取消标志。
pub async fn finish_op(
    cb: &AppCallbackServiceClient,
    ops: &Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    op_id: u64,
    succeeded: u64,
    failed: u64,
    cancelled: bool,
) {
    if let Err(e) = cb
        .progress(
            context::current(),
            op_id,
            ProgressEvent::Done {
                succeeded,
                failed,
                cancelled,
            },
        )
        .await
    {
        warn!("failed to send Progress::Done for op_id={op_id}: {e}");
    }
    ops.lock().await.remove(&op_id);
}
