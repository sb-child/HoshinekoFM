//! [`FsWorkerServer`] 实现 [`FsWorkerService`] trait，
//! 通过 [`WatchRegistry`] 管理目录/文件监视，通过回调通道推送增量/进度/冲突。
//!
//! 所有方法立刻返回（仅表示派发是否被接受），真正的结果／增量经反向的 `AppCallbackService` 推送。

use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use tarpc::context;
use tokio::sync::Mutex;
use tracing::{Instrument, debug, info};

use crate::error::AppError;
use crate::fsworker::protocol::{
    AppCallbackServiceClient, EntryKind, FsWorkerService, ProgressEvent, WatchDelta,
};

use super::{
    files::ProceedStrategy,
    ops::{BatchConfig, BatchKind, decide_dst, finish_op, run_batch},
    registry::WatchRegistry,
};

/// 崩溃诊断用：最后一个 watch_id / 最后一个操作名
pub(crate) static LAST_WATCH_ID: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAST_OPERATION: AtomicU64 = AtomicU64::new(0);

/// 操作编码常量，用于 set_last_op! 宏。
const WATCH_DIR: u64 = 1;
const UNWATCH: u64 = 2;
const WATCH_STAT: u64 = 3;
const REFRESH: u64 = 4;
const RUN_CREATE: u64 = 5;
const RUN_RENAME: u64 = 6;
const RUN_MOVE: u64 = 7;
const RUN_COPY: u64 = 8;
const CANCEL_OP: u64 = 9;
const STAT_VFS: u64 = 10;
const WATCH_BREADCRUMB: u64 = 11;

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
    registry: Arc<WatchRegistry>,
    watch_paths: Arc<Mutex<HashMap<u64, PathBuf>>>,
    /// op_id -> 取消标志
    ops: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    /// 冲突 ID 分配
    conflict_seq: Arc<AtomicU64>,
}

impl FsWorkerServer {
    pub fn new(
        fs_worker_id: u64,
        cb: AppCallbackServiceClient,
        registry: Arc<WatchRegistry>,
    ) -> Self {
        let uid = nix::unistd::getuid().as_raw();
        info!("fs worker {} starting, uid={}", fs_worker_id, uid);
        Self {
            fs_worker_id,
            cb,
            registry,
            watch_paths: Arc::new(Mutex::new(HashMap::new())),
            ops: Arc::new(Mutex::new(HashMap::new())),
            conflict_seq: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl FsWorkerService for FsWorkerServer {
    async fn ping(self, _ctx: context::Context) -> bool {
        true
    }

    // --
    // Watch
    // --

    async fn watch_dir(
        self,
        _ctx: context::Context,
        watch_id: u64,
        path: PathBuf,
    ) -> Result<(), AppError> {
        set_last_op!(WATCH_DIR, watch_id);
        debug!("[w{}] watch_dir {watch_id} {path:?}", self.fs_worker_id);
        self.registry
            .subscribe(watch_id, path.clone(), true, self.cb.clone())
            .await;
        self.watch_paths.lock().await.insert(watch_id, path);
        Ok(())
    }

    async fn watch_stat(
        self,
        _ctx: context::Context,
        watch_id: u64,
        path: PathBuf,
    ) -> Result<(), AppError> {
        set_last_op!(WATCH_STAT, watch_id);
        debug!("[w{}] watch_stat {watch_id} {path:?}", self.fs_worker_id);
        self.registry
            .subscribe(watch_id, path.clone(), false, self.cb.clone())
            .await;
        self.watch_paths.lock().await.insert(watch_id, path);
        Ok(())
    }

    async fn refresh(self, _ctx: context::Context, watch_id: u64) {
        set_last_op!(REFRESH, watch_id);
        debug!("[w{}] refresh {watch_id}", self.fs_worker_id);
        if let Some(path) = self.watch_paths.lock().await.get(&watch_id).cloned() {
            self.registry.request_reset(watch_id, &path).await;
        }
    }

    async fn unwatch(self, _ctx: context::Context, watch_id: u64) {
        set_last_op!(UNWATCH, watch_id);
        debug!("[w{}] unwatch {watch_id}", self.fs_worker_id);
        if let Some(path) = self.watch_paths.lock().await.remove(&watch_id) {
            self.registry.unsubscribe(watch_id, &path).await;
        }
    }

    // --
    // 变更操作
    // --

    async fn run_create(
        self,
        _ctx: context::Context,
        op_id: u64,
        path: PathBuf,
        kind: EntryKind,
    ) -> Result<(), AppError> {
        set_last_op!(RUN_CREATE, op_id);
        let _cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        let handle = tokio::spawn(
            async move {
                let _ = cb
                    .progress(
                        context::current(),
                        op_id,
                        ProgressEvent::Started { total: 1 },
                    )
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
                                    status: crate::fsworker::protocol::ItemStatus::Skipped,
                                },
                            )
                            .await;
                    }
                    Decision::Proceed { dst, .. } => {
                        let res = tokio::task::spawn_blocking(move || {
                            let _span =
                                tracing::info_span!("service::run_create::create_entry").entered();
                            super::files::create_entry(&dst, kind)
                        })
                        .await
                        .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
                        let status = match res {
                            Ok(()) => {
                                succeeded += 1;
                                crate::fsworker::protocol::ItemStatus::Ok
                            }
                            Err(e) => {
                                failed += 1;
                                crate::fsworker::protocol::ItemStatus::Failed(e.to_string())
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
            }
            .instrument(tracing::info_span!("service::run_create::worker")),
        );
        tokio::spawn(
            async move {
                if let Err(e) = handle.await {
                    tracing::error!(op_id, "run_create task panicked: {e}");
                }
            }
            .instrument(tracing::info_span!("service::run_create::monitor")),
        );
        Ok(())
    }

    async fn run_rename(
        self,
        _ctx: context::Context,
        op_id: u64,
        path: PathBuf,
        new_name: String,
    ) -> Result<(), AppError> {
        set_last_op!(RUN_RENAME, op_id);
        let _cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        let handle = tokio::spawn(
            async move {
                let _ = cb
                    .progress(
                        context::current(),
                        op_id,
                        ProgressEvent::Started { total: 1 },
                    )
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
                                    status: crate::fsworker::protocol::ItemStatus::Skipped,
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
                                    crate::fsworker::protocol::ItemStatus::Renamed(
                                        final_dst.clone(),
                                    )
                                } else {
                                    crate::fsworker::protocol::ItemStatus::Ok
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
                                    } => match super::files::rename_with_strategy(&s2, &fd, st2) {
                                        Ok(()) => {
                                            succeeded += 1;
                                            if fd != d2 {
                                                crate::fsworker::protocol::ItemStatus::Renamed(fd)
                                            } else {
                                                crate::fsworker::protocol::ItemStatus::Ok
                                            }
                                        }
                                        Err(e2) => {
                                            failed += 1;
                                            crate::fsworker::protocol::ItemStatus::Failed(
                                                e2.to_string(),
                                            )
                                        }
                                    },
                                    Decision::Skip => {
                                        crate::fsworker::protocol::ItemStatus::Skipped
                                    }
                                    Decision::Cancel => {
                                        cancelled = true;
                                        crate::fsworker::protocol::ItemStatus::Skipped
                                    }
                                }
                            }
                            Err(e) => {
                                failed += 1;
                                crate::fsworker::protocol::ItemStatus::Failed(e.to_string())
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
            }
            .instrument(tracing::info_span!("service::run_rename::worker")),
        );
        tokio::spawn(
            async move {
                if let Err(e) = handle.await {
                    tracing::error!(op_id, "run_rename task panicked: {e}");
                }
            }
            .instrument(tracing::info_span!("service::run_rename::monitor")),
        );
        Ok(())
    }

    async fn run_move(
        self,
        _ctx: context::Context,
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), AppError> {
        set_last_op!(RUN_MOVE, op_id);
        let cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        let handle = tokio::spawn(
            async move {
                run_batch(BatchConfig {
                    cb: Arc::new(cb),
                    ops,
                    op_id,
                    cancel,
                    conflict_seq,
                    items,
                    kind: BatchKind::Move,
                })
                .await;
            }
            .instrument(tracing::info_span!("service::run_move::worker")),
        );
        tokio::spawn(
            async move {
                if let Err(e) = handle.await {
                    tracing::error!(op_id, "run_move task panicked: {e}");
                }
            }
            .instrument(tracing::info_span!("service::run_move::monitor")),
        );
        Ok(())
    }

    async fn run_copy(
        self,
        _ctx: context::Context,
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    ) -> Result<(), AppError> {
        set_last_op!(RUN_COPY, op_id);
        let cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        let handle = tokio::spawn(
            async move {
                run_batch(BatchConfig {
                    cb: Arc::new(cb),
                    ops,
                    op_id,
                    cancel,
                    conflict_seq,
                    items,
                    kind: BatchKind::Copy,
                })
                .await;
            }
            .instrument(tracing::info_span!("service::run_copy::worker")),
        );
        tokio::spawn(
            async move {
                if let Err(e) = handle.await {
                    tracing::error!(op_id, "run_copy task panicked: {e}");
                }
            }
            .instrument(tracing::info_span!("service::run_copy::monitor")),
        );
        Ok(())
    }

    async fn cancel_op(self, _ctx: context::Context, op_id: u64) {
        set_last_op!(CANCEL_OP, op_id);
        if let Some(flag) = self.ops.lock().await.get(&op_id) {
            flag.store(true, Ordering::Relaxed);
            debug!("[w{}] cancel op {op_id}", self.fs_worker_id);
        }
    }

    async fn stat_vfs(self, _ctx: context::Context, path: PathBuf) -> Result<(u64, u64), AppError> {
        set_last_op!(STAT_VFS, 0);
        let p = path.clone();
        let vfs = tokio::task::spawn_blocking(move || {
            let _span = tracing::info_span!("service::stat_vfs").entered();
            nix::sys::statvfs::statvfs(&p)
        })
        .await
        .map_err(|e| AppError::Other(format!("spawn_blocking: {e}")))?
        .map_err(|e| AppError::Other(format!("statvfs {}: {e}", path.display())))?;
        let block_size = vfs.block_size() as u64;
        Ok((vfs.blocks() * block_size, vfs.blocks_free() * block_size))
    }

    async fn watch_breadcrumb(
        self,
        _ctx: context::Context,
        watch_id: u64,
        path: PathBuf,
    ) -> Result<(), AppError> {
        set_last_op!(WATCH_BREADCRUMB, watch_id);
        debug!(
            "[w{}] watch_breadcrumb {watch_id} {path:?}",
            self.fs_worker_id
        );

        // 首帧：立即推送当前面包屑段信息
        let initial_segments = tokio::task::spawn_blocking({
            let path = path.clone();
            move || {
                let _span = tracing::info_span!("service::watch_breadcrumb::init").entered();
                let home_map = HOME_MAP.clone();
                let mount_map = load_mount_map();
                build_breadcrumb_segments(&path, &home_map, &mount_map)
            }
        })
        .await
        .map_err(|e| AppError::Other(format!("spawn_blocking: {e}")))?;

        let _ = self
            .cb
            .watch_delta(
                context::current(),
                watch_id,
                WatchDelta::BreadcrumbSegments(initial_segments),
            )
            .await;

        // 复用 WatchRegistry 监听 /proc/mounts 变化
        // 多个 watch_breadcrumb 共享同一个 /proc/mounts 监视
        let mount_path = PathBuf::from("/proc/mounts");
        self.registry
            .subscribe(watch_id, mount_path.clone(), false, self.cb.clone())
            .await;
        self.watch_paths.lock().await.insert(watch_id, mount_path);

        Ok(())
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

// --
// 面包屑辅助：/etc/passwd + /proc/mounts 读取与路径段判断
// --

use std::sync::LazyLock;

/// 家目录缓存：path -> (username, uid)。/etc/passwd 极少变化，进程生命周期内缓存。
static HOME_MAP: LazyLock<HashMap<String, (String, u32)>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    if let Ok(content) = std::fs::read_to_string("/etc/passwd") {
        for line in content.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 7 {
                let username = fields[0].to_string();
                let uid: u32 = fields[2].parse().unwrap_or(0);
                let home = fields[5].to_string();
                if home != "/" && !home.is_empty() {
                    map.insert(home, (username, uid));
                }
            }
        }
    }
    map
});

/// 加载挂载映射：mountpoint -> (source, fstype)。
/// /proc/mounts 随挂载变化，每次调用重新读取。
fn load_mount_map() -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    if let Ok(content) = std::fs::read_to_string("/proc/mounts") {
        for line in content.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 3 {
                let source = unescape_mount(fields[0]);
                let mountpoint = unescape_mount(fields[1]);
                let fstype = fields[2].to_string();
                if !mountpoint.is_empty() {
                    map.insert(mountpoint, (source, fstype));
                }
            }
        }
    }
    map
}

/// 还原 /proc/mounts 中的转义字符（\040->空格 等）。
fn unescape_mount(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 3 < bytes.len()
            && bytes[i] == b'\\'
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let octal =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            result.push(octal as char);
            i += 4;
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// 为面包屑构建每个路径段的 home/mount 信息。
fn build_breadcrumb_segments(
    path: &std::path::Path,
    home_map: &HashMap<String, (String, u32)>,
    mount_map: &HashMap<String, (String, String)>,
) -> Vec<crate::fsworker::protocol::BreadcrumbSegment> {
    let seg_paths = super::files::split_path_segments(path);
    seg_paths
        .iter()
        .map(|seg_path| {
            let path_str = seg_path.to_string_lossy().to_string();
            let name = seg_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());

            let (is_home, home_username) = match home_map.get(&path_str) {
                Some((username, _)) => (true, Some(username.clone())),
                None => (false, None),
            };

            let (is_mount_point, mount_source) = match mount_map.get(&path_str) {
                Some((source, _fstype)) => (true, Some(source.clone())),
                None => (false, None),
            };

            crate::fsworker::protocol::BreadcrumbSegment {
                name,
                path: path_str,
                is_home,
                home_username,
                is_mount_point,
                mount_source,
            }
        })
        .collect()
}
