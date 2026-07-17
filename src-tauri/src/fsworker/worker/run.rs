//! FS Worker 子进程入口。
//!
//! 由主进程通过 `hnfm __fs-worker --fs-worker-id <n> --fd <n> --cb-fd <n>` 启动，
//! 两条匿名 socketpair：
//!
//! - `--fd`：请求通道，Worker 作 tarpc server，实现 [`FsWorkerService`]。
//! - `--cb-fd`：回调通道，Worker 作 tarpc client，调用主进程的 `AppCallbackService`
//!   推送 watcher 增量 / 批处理进度 / 冲突询问。
//!
//! ## 安全
//! - 不创建 GUI 窗口（Headless）。
//! - 仅通过继承 FD 通信，无对外暴露端口。
//! - 所有 UID 相关操作以本进程 EUID 执行（提权由父进程 pkexec 完成）。

use std::future::Future;
use std::{
    os::unix::io::FromRawFd, os::unix::net::UnixStream as StdUnixStream, sync::Arc, time::Duration,
};

use futures::StreamExt;
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info, warn};

use super::{config::WatchConfig, pipeline, service::FsWorkerServer};
use crate::{
    fsworker::protocol::{AppCallbackServiceClient, FsWorkerService},
    fsworker::{FsWorkerOpts, ORPHAN_EXIT_CODE},
};

/// 运行 FS Worker 子进程。
pub async fn run_fs_worker(opts: FsWorkerOpts) -> ! {
    use tarpc::{
        serde_transport,
        server::{BaseChannel, Channel},
    };

    // 0. panic hook: 崩溃时打印 backtrace 后 flush 退出
    let fs_worker_id = opts.fs_worker_id;
    let fs_worker_id_for_panic = fs_worker_id;
    std::panic::set_hook(Box::new(move |info| {
        let last_op = super::service::LAST_OPERATION.load(std::sync::atomic::Ordering::Relaxed);
        let last_wid = super::service::LAST_WATCH_ID.load(std::sync::atomic::Ordering::Relaxed);
        let backtrace = std::backtrace::Backtrace::force_capture();
        eprintln!(
            "[PANIC fs-worker={fs_worker_id_for_panic}] last_op={last_op} last_watch_id={last_wid}\n{info}\n{backtrace}"
        );
        std::process::exit(101);
    }));

    // 1. 孤儿检测
    let parent_pid = opts.parent_pid;
    tokio::spawn(async move {
        let mut failures: u32 = 0;
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            if unsafe { libc::kill(parent_pid, 0) } == 0 {
                failures = 0;
                continue;
            }
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::ESRCH) => {
                    failures += 1;
                    if failures >= 2 {
                        warn!(
                            "fs-worker {fs_worker_id} detected orphan (parent_pid={parent_pid} gone), exiting with code {}",
                            ORPHAN_EXIT_CODE
                        );
                        std::process::exit(ORPHAN_EXIT_CODE);
                    }
                }
                Some(libc::EPERM) => {
                    let ppid = unsafe { libc::getppid() };
                    if ppid <= 1 {
                        failures += 1;
                        if failures >= 2 {
                            warn!("fs-worker {fs_worker_id} orphan via ppid (cross-UID), exiting");
                            std::process::exit(ORPHAN_EXIT_CODE);
                        }
                    } else {
                        failures = 0;
                    }
                }
                _ => {
                    failures = 0;
                }
            }
        }
    }.instrument(tracing::info_span!("run::orphan_check")));

    // 2. 回调通道（worker -> app）
    let cb_fd = opts.cb_fd;
    info!("fs worker {} restoring cb-fd={}", opts.fs_worker_id, cb_fd);
    let cb_std = unsafe { StdUnixStream::from_raw_fd(cb_fd) };
    cb_std.set_nonblocking(true).expect("cb set_nonblocking");
    let cb_stream = UnixStream::from_std(cb_std).expect("cb to tokio stream");
    let cb_transport = serde_transport::new(
        crate::mesh::transport::frame_stream(cb_stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    let cb = AppCallbackServiceClient::new(tarpc::client::Config::default(), cb_transport).spawn();

    // 3. 启动 Watch 流水线
    let cancel = CancellationToken::new();
    let config = WatchConfig::default();
    let (registry, handles) = pipeline::assemble(cancel.clone(), &config, opts.fs_worker_id);

    // 4. 请求通道（app -> worker）：恢复 fd -> tarpc server
    let server = FsWorkerServer::new(opts.fs_worker_id, cb, Arc::new(registry));
    let fd = opts.fd;
    info!("fs worker {} restoring fd={}", opts.fs_worker_id, fd);
    let std_stream = unsafe { StdUnixStream::from_raw_fd(fd) };
    std_stream.set_nonblocking(true).expect("set_nonblocking");
    let stream = UnixStream::from_std(std_stream).expect("to tokio stream");
    let transport = serde_transport::new(
        crate::mesh::transport::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );

    async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut.instrument(tracing::info_span!("run::channel_handler")));
    }

    info!("fs worker {} entering serve loop", opts.fs_worker_id);
    BaseChannel::with_defaults(transport)
        .execute(server.serve())
        .for_each(spawn)
        .await;

    info!(
        "fs worker {} serve loop exited, shutting down",
        opts.fs_worker_id
    );

    // 关闭流水线
    cancel.cancel();
    handles.join_all().await;

    std::process::exit(0);
}
