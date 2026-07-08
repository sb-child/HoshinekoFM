//! FS Worker 子进程入口与服务实现。
//!
//! 由主进程通过 `hnfm fs-worker --worker-id <n> --fd <n> --cb-fd <n>` 启动，
//! 两条匿名 socketpair：
//!
//! - `--fd`：请求通道，Worker 作 tarpc server，实现 [`FsWorkerService`]。
//! - `--cb-fd`：回调通道，Worker 作 tarpc client，调用主进程的 `AppCallbackService`
//!   推送 watcher 增量 / 批处理进度 / 冲突询问。
//!
//! ## 安全
//!
//! - 不创建 GUI 窗口（Headless）。
//! - 仅通过继承 FD 通信，无对外暴露端口。
//! - 所有 UID 相关操作以本进程 EUID 执行（提权由父进程 pkexec 完成）。
//!
//! ## 设计
//!
//! - 无 `list_dir` / `stat`：只有 `watch_*`。首帧全量 `Reset`，之后增量 `Upsert`/`Remove`。
//! - mime 随首帧给出；缩略图（图片渲染）后续通过 `Upsert` 渐进补发（TODO）。
//! - 所有变更操作立刻返回，进度经回调推送，逐条检查取消标志。
//! - 跨 UID 由父进程 FsService 分派，Worker 只处理同 UID 项。

use std::{
    collections::HashMap,
    io,
    os::unix::{fs::MetadataExt, io::FromRawFd, net::UnixStream as StdUnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use tarpc::context;
use tokio::{net::UnixStream, sync::Mutex};
use tracing::{debug, info, warn};

use crate::ipc::protocol::{
    AppCallbackServiceClient, ConflictItem, ConflictResolution, EntryKind, File, FsWorkerService,
    ItemStatus, ProgressEvent, WatchDelta,
};

use super::FsWorkerOpts;

// ---------------------------------------------------------------------------
// Worker 服务实现
// ---------------------------------------------------------------------------

/// 一个活跃 watcher 的资源句柄。drop 即停止监视（notify watcher 析构 + 任务 abort）。
struct WatchHandle {
    /// 被监视路径
    path: PathBuf,
    /// true=目录监视，false=单文件属性监视
    is_dir: bool,
    /// notify 监视器（保持存活）
    _watcher: Option<notify::RecommendedWatcher>,
    /// 后台任务（事件泵）的中止句柄
    tasks: Vec<tokio::task::AbortHandle>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// FS Worker 服务实现。
#[derive(Clone)]
pub struct FsWorkerServer {
    /// Worker ID（日志用）
    worker_id: u64,
    /// 回调通道客户端（worker → app）
    cb: AppCallbackServiceClient,
    /// watch_id → 监视句柄
    watches: Arc<Mutex<HashMap<u64, WatchHandle>>>,
    /// op_id → 取消标志
    ops: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    /// 冲突 ID 分配
    conflict_seq: Arc<AtomicU64>,
}

impl FsWorkerServer {
    pub fn new(opts: &FsWorkerOpts, cb: AppCallbackServiceClient) -> Self {
        let uid = nix::unistd::getuid().as_raw();
        info!("fs worker {} starting, uid={}", opts.worker_id, uid);
        Self {
            worker_id: opts.worker_id,
            cb,
            watches: Arc::new(Mutex::new(HashMap::new())),
            ops: Arc::new(Mutex::new(HashMap::new())),
            conflict_seq: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl FsWorkerService for FsWorkerServer {
    // -----------------------------------------------------------------------
    // Watch
    // -----------------------------------------------------------------------

    async fn watch_dir(
        self,
        _ctx: context::Context,
        watch_id: u64,
        path: String,
    ) -> Result<(), String> {
        debug!("[w{}] watch_dir {watch_id} {path}", self.worker_id);
        let dir = resolve_path(&PathBuf::from(&path));

        // 初次全量扫描（后台）
        let cb = self.cb.clone();
        let scan_dir = dir.clone();
        let scan_task = tokio::spawn(async move {
            push_dir_reset(&cb, watch_id, &scan_dir).await;
        });

        // notify 监视 + 事件泵
        let (watcher, pump) = match spawn_dir_watch(self.cb.clone(), watch_id, dir.clone()) {
            Ok(pair) => pair,
            Err(e) => {
                warn!("[w{}] notify watch failed for {dir:?}: {e}", self.worker_id);
                (None, None)
            }
        };

        let mut tasks = vec![scan_task.abort_handle()];
        if let Some(p) = pump {
            tasks.push(p);
        }
        self.watches.lock().await.insert(
            watch_id,
            WatchHandle {
                path: dir,
                is_dir: true,
                _watcher: watcher,
                tasks,
            },
        );
        Ok(())
    }

    async fn watch_stat(
        self,
        _ctx: context::Context,
        watch_id: u64,
        path: String,
    ) -> Result<(), String> {
        debug!("[w{}] watch_stat {watch_id} {path}", self.worker_id);
        let target = PathBuf::from(&path);

        let cb = self.cb.clone();
        let scan_target = target.clone();
        let scan_task = tokio::spawn(async move {
            push_stat_reset(&cb, watch_id, &scan_target).await;
        });

        // 监视文件本身（其内容/属性变化）
        let (watcher, pump) = match spawn_stat_watch(self.cb.clone(), watch_id, target.clone()) {
            Ok(pair) => pair,
            Err(e) => {
                warn!("[w{}] notify stat watch failed for {target:?}: {e}", self.worker_id);
                (None, None)
            }
        };

        let mut tasks = vec![scan_task.abort_handle()];
        if let Some(p) = pump {
            tasks.push(p);
        }
        self.watches.lock().await.insert(
            watch_id,
            WatchHandle {
                path: target,
                is_dir: false,
                _watcher: watcher,
                tasks,
            },
        );
        Ok(())
    }

    async fn refresh(self, _ctx: context::Context, watch_id: u64) {
        let (path, is_dir) = {
            let watches = self.watches.lock().await;
            match watches.get(&watch_id) {
                Some(h) => (h.path.clone(), h.is_dir),
                None => return,
            }
        };
        let cb = self.cb.clone();
        tokio::spawn(async move {
            if is_dir {
                push_dir_reset(&cb, watch_id, &path).await;
            } else {
                push_stat_reset(&cb, watch_id, &path).await;
            }
        });
    }

    async fn unwatch(self, _ctx: context::Context, watch_id: u64) {
        debug!("[w{}] unwatch {watch_id}", self.worker_id);
        // drop WatchHandle → 停止 notify + abort 任务
        self.watches.lock().await.remove(&watch_id);
    }

    // -----------------------------------------------------------------------
    // 变更操作（立刻返回，进度经回调）
    // -----------------------------------------------------------------------

    async fn run_create(
        self,
        _ctx: context::Context,
        op_id: u64,
        path: String,
        kind: EntryKind,
    ) -> Result<(), String> {
        let cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        tokio::spawn(async move {
            let _ = cb.progress(context::current(), op_id, ProgressEvent::Started { total: 1 }).await;
            let target = PathBuf::from(&path);
            let mut succeeded = 0u64;
            let mut failed = 0u64;
            let mut cancelled = false;

            match decide_dst(&cb, op_id, &conflict_seq, &target, &target).await {
                Decision::Cancel => cancelled = true,
                Decision::Skip => {
                    let _ = cb.progress(context::current(), op_id, ProgressEvent::Item {
                        src: target.clone(), dst: target.clone(), status: ItemStatus::Skipped,
                    }).await;
                }
                Decision::Proceed(dst) => {
                    let res = tokio::task::spawn_blocking(move || create_entry(&dst, kind))
                        .await
                        .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
                    let status = match res {
                        Ok(()) => { succeeded += 1; ItemStatus::Ok }
                        Err(e) => { failed += 1; ItemStatus::Failed(e.to_string()) }
                    };
                    let _ = cb.progress(context::current(), op_id, ProgressEvent::Item {
                        src: target.clone(), dst: target.clone(), status,
                    }).await;
                }
            }
            let _ = cancel; // 已通过 decide/loop 检查
            finish_op(&cb, &ops, op_id, succeeded, failed, cancelled).await;
        });
        Ok(())
    }

    async fn run_rename(
        self,
        _ctx: context::Context,
        op_id: u64,
        path: String,
        new_name: String,
    ) -> Result<(), String> {
        let _cancel = self.register_op(op_id).await;
        let cb = self.cb.clone();
        let conflict_seq = self.conflict_seq.clone();
        let ops = self.ops.clone();
        tokio::spawn(async move {
            let _ = cb.progress(context::current(), op_id, ProgressEvent::Started { total: 1 }).await;
            let src = PathBuf::from(&path);
            let parent = src.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
            let dst = parent.join(&new_name);
            let mut succeeded = 0u64;
            let mut failed = 0u64;
            let mut cancelled = false;

            match decide_dst(&cb, op_id, &conflict_seq, &src, &dst).await {
                Decision::Cancel => cancelled = true,
                Decision::Skip => {
                    let _ = cb.progress(context::current(), op_id, ProgressEvent::Item {
                        src: src.clone(), dst: dst.clone(), status: ItemStatus::Skipped,
                    }).await;
                }
                Decision::Proceed(final_dst) => {
                    let renamed = final_dst != dst;
                    let s = src.clone();
                    let d = final_dst.clone();
                    let res = tokio::task::spawn_blocking(move || std::fs::rename(&s, &d))
                        .await
                        .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
                    let status = match res {
                        Ok(()) => {
                            succeeded += 1;
                            if renamed { ItemStatus::Renamed(final_dst.clone()) } else { ItemStatus::Ok }
                        }
                        Err(e) => { failed += 1; ItemStatus::Failed(e.to_string()) }
                    };
                    let _ = cb.progress(context::current(), op_id, ProgressEvent::Item {
                        src: src.clone(), dst: final_dst, status,
                    }).await;
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
        items: Vec<(String, String)>,
    ) -> Result<(), String> {
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
        items: Vec<(String, String)>,
    ) -> Result<(), String> {
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
        if let Some(flag) = self.ops.lock().await.get(&op_id) {
            flag.store(true, Ordering::Relaxed);
            debug!("[w{}] cancel op {op_id}", self.worker_id);
        }
    }
}

impl FsWorkerServer {
    /// 注册一个操作的取消标志。
    async fn register_op(&self, op_id: u64) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.ops.lock().await.insert(op_id, flag.clone());
        flag
    }
}

// ---------------------------------------------------------------------------
// 批处理执行
// ---------------------------------------------------------------------------

enum BatchKind {
    Move,
    Copy,
}

#[allow(clippy::explicit_counter_loop)]
async fn run_batch(
    cb: &AppCallbackServiceClient,
    ops: &Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    op_id: u64,
    cancel: Arc<AtomicBool>,
    conflict_seq: &Arc<AtomicU64>,
    items: Vec<(String, String)>,
    kind: BatchKind,
) {
    let total = items.len() as u64;
    let _ = cb.progress(context::current(), op_id, ProgressEvent::Started { total }).await;

    let mut succeeded = 0u64;
    let mut failed = 0u64;
    let mut cancelled = false;
    // `done` 是已尝试条目计数（进度心跳用），因循环有提前 break，非简单迭代计数。
    let mut done = 0u64;

    for (src_s, dst_s) in items {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        done += 1;
        let src = PathBuf::from(&src_s);
        let dst = PathBuf::from(&dst_s);

        let status = match decide_dst(cb, op_id, conflict_seq, &src, &dst).await {
            Decision::Cancel => {
                cancelled = true;
                break;
            }
            Decision::Skip => ItemStatus::Skipped,
            Decision::Proceed(final_dst) => {
                let renamed = final_dst != dst;
                let s = src.clone();
                let d = final_dst.clone();
                let res = match kind {
                    BatchKind::Move => {
                        tokio::task::spawn_blocking(move || move_path(&s, &d)).await
                    }
                    BatchKind::Copy => {
                        tokio::task::spawn_blocking(move || copy_path(&s, &d)).await
                    }
                }
                .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
                match res {
                    Ok(()) => {
                        succeeded += 1;
                        if renamed {
                            ItemStatus::Renamed(final_dst.clone())
                        } else {
                            ItemStatus::Ok
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        ItemStatus::Failed(e.to_string())
                    }
                }
            }
        };

        let _ = cb
            .progress(
                context::current(),
                op_id,
                ProgressEvent::Item {
                    src: src.clone(),
                    dst: dst.clone(),
                    status,
                },
            )
            .await;
        let _ = cb
            .progress(
                context::current(),
                op_id,
                ProgressEvent::Tick {
                    done,
                    total,
                    current: src,
                },
            )
            .await;
    }

    finish_op(cb, ops, op_id, succeeded, failed, cancelled).await;
}

/// 冲突决策结果。
enum Decision {
    /// 继续，使用给定目标路径
    Proceed(PathBuf),
    /// 跳过该项
    Skip,
    /// 取消整个操作
    Cancel,
}

/// 若 `dst` 已存在则询问上层，返回最终决策。
async fn decide_dst(
    cb: &AppCallbackServiceClient,
    op_id: u64,
    conflict_seq: &Arc<AtomicU64>,
    src: &Path,
    dst: &Path,
) -> Decision {
    if !dst.exists() {
        return Decision::Proceed(dst.to_path_buf());
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
        ConflictResolution::Overwrite => Decision::Proceed(dst.to_path_buf()),
        ConflictResolution::AutoRename => Decision::Proceed(unique_path(dst)),
        ConflictResolution::Rename(name) => {
            let parent = dst.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
            Decision::Proceed(parent.join(name))
        }
        ConflictResolution::CancelAll => Decision::Cancel,
    }
}

/// 结束操作：推 Done、注销取消标志。
async fn finish_op(
    cb: &AppCallbackServiceClient,
    ops: &Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    op_id: u64,
    succeeded: u64,
    failed: u64,
    cancelled: bool,
) {
    let _ = cb
        .progress(
            context::current(),
            op_id,
            ProgressEvent::Done {
                succeeded,
                failed,
                cancelled,
            },
        )
        .await;
    ops.lock().await.remove(&op_id);
}

// ---------------------------------------------------------------------------
// Watcher 后台任务
// ---------------------------------------------------------------------------

/// 全量扫描目录并推 `Reset`。
async fn push_dir_reset(cb: &AppCallbackServiceClient, watch_id: u64, dir: &Path) {
    let dir = dir.to_path_buf();
    let files = tokio::task::spawn_blocking(move || list_dir_files(&dir))
        .await
        .unwrap_or_default();
    let _ = cb
        .watch_delta(context::current(), watch_id, WatchDelta::Reset(files))
        .await;
}

/// 扫描单个路径并推 `Reset`（含 0 或 1 个 File）。
async fn push_stat_reset(cb: &AppCallbackServiceClient, watch_id: u64, path: &Path) {
    let p = path.to_path_buf();
    let file = tokio::task::spawn_blocking(move || build_file(&p)).await.ok().flatten();
    let files = file.map(|f| vec![f]).unwrap_or_default();
    let _ = cb
        .watch_delta(context::current(), watch_id, WatchDelta::Reset(files))
        .await;
}

/// 为目录建立 notify 监视 + 事件泵任务。
fn spawn_dir_watch(
    cb: AppCallbackServiceClient,
    watch_id: u64,
    dir: PathBuf,
) -> notify::Result<(Option<notify::RecommendedWatcher>, Option<tokio::task::AbortHandle>)> {
    use notify::{RecursiveMode, Watcher};

    let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let _ = evt_tx.send(event.paths);
        }
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    let pump = tokio::spawn(async move {
        while let Some(paths) = evt_rx.recv().await {
            for p in paths {
                let delta = if p.exists() {
                    match build_file(&p) {
                        Some(f) => WatchDelta::Upsert(f),
                        None => continue,
                    }
                } else {
                    WatchDelta::Remove(p)
                };
                let _ = cb.watch_delta(context::current(), watch_id, delta).await;
            }
        }
    });

    Ok((Some(watcher), Some(pump.abort_handle())))
}

/// 为单个文件建立 notify 监视 + 事件泵任务。
fn spawn_stat_watch(
    cb: AppCallbackServiceClient,
    watch_id: u64,
    target: PathBuf,
) -> notify::Result<(Option<notify::RecommendedWatcher>, Option<tokio::task::AbortHandle>)> {
    use notify::{RecursiveMode, Watcher};

    let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = evt_tx.send(());
        }
    })?;
    // 监视文件本身；若为路径不存在会失败，交由调用者忽略
    watcher.watch(&target, RecursiveMode::NonRecursive)?;

    let pump = tokio::spawn(async move {
        while evt_rx.recv().await.is_some() {
            let delta = if target.exists() {
                match build_file(&target) {
                    Some(f) => WatchDelta::Upsert(f),
                    None => continue,
                }
            } else {
                WatchDelta::Remove(target.clone())
            };
            let _ = cb.watch_delta(context::current(), watch_id, delta).await;
        }
    });

    Ok((Some(watcher), Some(pump.abort_handle())))
}

// ---------------------------------------------------------------------------
// Worker 入口
// ---------------------------------------------------------------------------

/// 运行 FS Worker 子进程。
pub async fn run_fs_worker(opts: FsWorkerOpts) -> ! {
    use futures::prelude::*;
    use tarpc::{
        serde_transport,
        server::{BaseChannel, Channel},
    };

    // 1. 回调通道（worker → app）：恢复 fd → tarpc client
    let cb_fd = opts.cb_fd.expect("--cb-fd is required for fs-worker mode");
    info!("fs worker {} restoring cb-fd={}", opts.worker_id, cb_fd);
    let cb_std = unsafe { StdUnixStream::from_raw_fd(cb_fd) };
    cb_std.set_nonblocking(true).expect("cb set_nonblocking");
    let cb_stream = UnixStream::from_std(cb_std).expect("cb to tokio stream");
    let cb_transport = serde_transport::new(
        crate::ipc::frame_stream(cb_stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    let cb = AppCallbackServiceClient::new(tarpc::client::Config::default(), cb_transport).spawn();

    // 2. 请求通道（app → worker）：恢复 fd → tarpc server
    let server = FsWorkerServer::new(&opts, cb);
    let fd = opts.fd.expect("--fd is required for fs-worker mode");
    info!("fs worker {} restoring fd={}", opts.worker_id, fd);
    let std_stream = unsafe { StdUnixStream::from_raw_fd(fd) };
    std_stream.set_nonblocking(true).expect("set_nonblocking");
    let stream = UnixStream::from_std(std_stream).expect("to tokio stream");
    let transport = serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );

    async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }

    info!("fs worker {} entering serve loop", opts.worker_id);
    BaseChannel::with_defaults(transport)
        .execute(server.serve())
        .for_each(spawn)
        .await;

    info!("fs worker {} exiting", opts.worker_id);
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// 文件操作
// ---------------------------------------------------------------------------

/// tarpc 长超时上下文（用于阻塞式 ask_conflict，等待用户决策）。
fn long_ctx() -> context::Context {
    let mut c = context::current();
    c.deadline = std::time::Instant::now() + Duration::from_secs(3600);
    c
}

/// 解析路径中的符号链接（失败则原样返回）。
fn resolve_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 列出目录内容为 `File` 列表（含 mime；缩略图 TODO）。
fn list_dir_files(dir: &Path) -> Vec<File> {
    let mut files = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            warn!("list_dir_files({dir:?}) failed: {e}");
            return files;
        }
    };
    for entry in read.flatten() {
        if let Some(f) = build_file(&entry.path()) {
            files.push(f);
        }
    }
    files.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    files
}

/// 构建单个 `File`（用 symlink 元数据判断软链接，用 stat 取属性）。
fn build_file(path: &Path) -> Option<File> {
    let symlink_meta = std::fs::symlink_metadata(path).ok()?;
    let is_symlink = symlink_meta.file_type().is_symlink();
    // 跟随符号链接取真实属性；失败则退回 symlink 元数据
    let meta = std::fs::metadata(path).unwrap_or(symlink_meta);

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_directory = meta.is_dir();
    let mime = if is_directory {
        Some("inode/directory".to_string())
    } else if is_symlink && std::fs::metadata(path).is_err() {
        Some("inode/symlink".to_string())
    } else {
        Some(guess_mime(path))
    };

    Some(File {
        name,
        path: path.to_path_buf(),
        size: meta.len(),
        modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        is_directory,
        is_symlink,
        permissions: meta.mode(),
        owner_uid: meta.uid(),
        owner_gid: meta.gid(),
        mime,
        thumbnail: None, // TODO: 图片渐进式渲染，算好后经 Upsert 补发
    })
}

/// 创建文件或目录。
fn create_entry(path: &Path, kind: EntryKind) -> io::Result<()> {
    match kind {
        EntryKind::Dir => std::fs::create_dir_all(path),
        EntryKind::File => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map(|_| ())
        }
    }
}

/// 移动路径：先 rename，跨设备（EXDEV）时退化为 copy + 删除。
fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(nix::errno::Errno::EXDEV as i32) => {
            copy_path(src, dst)?;
            if src.is_dir() {
                std::fs::remove_dir_all(src)
            } else {
                std::fs::remove_file(src)
            }
        }
        Err(e) => Err(e),
    }
}

/// 递归复制文件/目录。
fn copy_path(src: &Path, dst: &Path) -> io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_path(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// 为已存在的目标生成唯一路径：`foo.txt` → `foo (1).txt` → `foo (2).txt` …
fn unique_path(dst: &Path) -> PathBuf {
    let parent = dst.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
    let stem = dst.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = dst.extension().map(|s| s.to_string_lossy().into_owned());

    for i in 1..10_000 {
        let name = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    // 极端兜底
    parent.join(format!("{stem}.{}", nix::unistd::getpid().as_raw()))
}

/// 基于扩展名的 MIME 猜测。
fn guess_mime(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "txt" | "md" | "rs" | "toml" | "json" | "yaml" | "yml" | "xml" | "html" | "css" | "js"
        | "ts" | "jsx" | "tsx" | "py" | "sh" | "bash" | "env" | "cfg" | "ini" | "log" | "csv"
        | "lock" => "text/plain".to_string(),
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "webp" => "image/webp".to_string(),
        "mp3" => "audio/mpeg".to_string(),
        "wav" => "audio/wav".to_string(),
        "flac" => "audio/flac".to_string(),
        "ogg" => "audio/ogg".to_string(),
        "mp4" => "video/mp4".to_string(),
        "mkv" => "video/x-matroska".to_string(),
        "mov" => "video/quicktime".to_string(),
        "avi" => "video/x-msvideo".to_string(),
        "webm" => "video/webm".to_string(),
        "zip" => "application/zip".to_string(),
        "tar" => "application/x-tar".to_string(),
        "gz" => "application/gzip".to_string(),
        "xz" => "application/x-xz".to_string(),
        "pdf" => "application/pdf".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}
