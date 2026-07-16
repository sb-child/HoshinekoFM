//! WorkerRelay — per-UID async loop (spawn / heartbeat / relay / restart).

use std::{
    io,
    os::unix::{io::AsRawFd, net::UnixStream as StdUnixStream},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Instant,
};

use nix::unistd;
use tokio::{net::UnixStream, select};
use tracing::{debug, info, warn};

use crate::channel;
use crate::fsworker::protocol::FsWorkerServiceClient;

use super::callback::{CallbackRegistry, serve_callback};
use super::platform::{ORPHAN_EXIT_CODE, clear_cloexec, get_exe_path, is_appimage};
use super::{
    CONNECT_TIMEOUT, DisconnectReason, FALLBACK_CWD, FS_WORKER_ID, HEARTBEAT_INTERVAL,
    HEARTBEAT_TIMEOUT, RESTART_DELAY, SIGKILL_DELAY, WorkerRequest, WorkerRequestContent,
    WorkerResponse, WorkerStatus,
};

/// Worker 中继器。在独立 tokio task 中运行，负责：
///
/// 1. spawn fs-worker 子进程
/// 2. 心跳检测
/// 3. 消息中继（channel ↔ tarpc）
/// 4. 向上层汇报连接状态
/// 5. 崩溃重启（无次数限制，直到 uidtoken 全部 drop）
pub(crate) struct WorkerRelay;

impl WorkerRelay {
    pub(crate) fn spawn(
        uid: u32,
        pid: Arc<AtomicU32>,
        registry: Arc<CallbackRegistry>,
        request_rx: channel::RxAsync<WorkerRequest>,
        status_tx: channel::Tx<WorkerStatus>,
        sentinel: Arc<super::LeaseSentinel>,
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
        mut request_rx: channel::RxAsync<WorkerRequest>,
        status_tx: channel::Tx<WorkerStatus>,
        sentinel: Arc<super::LeaseSentinel>,
    ) {
        info!("WorkerRelay started for uid={uid}");
        let mut attempt: u32 = 0;

        loop {
            if Arc::strong_count(&sentinel) <= 1 {
                info!("no more uidtoken for uid {uid}, shutting down relay");
                break;
            }

            attempt += 1;
            if attempt > 1 {
                info!("WorkerRelay uid={uid}: reconnecting attempt={attempt}");
                let _ = status_tx.send(WorkerStatus::Reconnecting { uid, attempt });
            }

            let fs_worker_id = FS_WORKER_ID.fetch_add(1, Ordering::Relaxed);
            info!(
                "WorkerRelay uid={uid}: spawning fs_worker_id={fs_worker_id} (attempt={attempt})"
            );

            let (child_pid, client) =
                match Self::spawn_fs_worker(uid, fs_worker_id, registry.clone()).await {
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
                        tokio::time::sleep(RESTART_DELAY).await;
                        continue;
                    }
                };

            pid.store(child_pid, Ordering::Relaxed);

            let _ = status_tx.send(WorkerStatus::Connecting { uid });

            let reason =
                Self::run_relay(uid, child_pid, &client, &mut request_rx, &status_tx).await;
            pid.store(0, Ordering::Relaxed);

            let _ = status_tx.send(WorkerStatus::Disconnected {
                uid,
                reason: reason.clone(),
                reconnecting: true,
            });
            registry.notify_connection_lost(reason, true);

            kill_fs_worker(uid, child_pid);

            tokio::time::sleep(RESTART_DELAY).await;
        }
        info!("WorkerRelay uid={uid}: loop exited, relay shutting down");
    }

    async fn run_relay(
        uid: u32,
        child_pid: u32,
        client: &FsWorkerServiceClient,
        request_rx: &mut channel::RxAsync<WorkerRequest>,
        status_tx: &channel::Tx<WorkerStatus>,
    ) -> DisconnectReason {
        info!("WorkerRelay run_relay started for uid={uid} child_pid={child_pid}");
        let mut connected = false;
        let mut tick_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        let mut last_heartbeat = Instant::now();
        let mut heartbeat_failures: u32 = 0;

        let wait_fut = Self::wait_process(child_pid);
        tokio::pin!(wait_fut);

        loop {
            select! {
                maybe_req = request_rx.recv() => {
                    match Self::handle_relay_request(uid, client, maybe_req.ok(), connected) {
                        Some(reason) => return reason,
                        None => {}
                    }
                }

                _ = tick_interval.tick() => {
                    match Self::handle_relay_heartbeat(
                        uid, child_pid, client, status_tx,
                        &mut connected, &mut last_heartbeat, &mut heartbeat_failures,
                    ).await {
                        Some(reason) => return reason,
                        None => {}
                    }
                }

                status = &mut wait_fut => {
                    info!("WorkerRelay uid={uid}: wait_process returned {status:?}");
                    return status;
                }
            }
        }
    }

    fn handle_relay_request(
        uid: u32,
        client: &FsWorkerServiceClient,
        request: Option<WorkerRequest>,
        connected: bool,
    ) -> Option<DisconnectReason> {
        match request {
            Some(request) => {
                if connected {
                    let client = client.clone();
                    let h = tokio::spawn(async move {
                        let response = Self::forward_request(&client, request.content).await;
                        let _ = request.response_tx.send(response);
                    });
                    // 后台监控 panic，至少记录日志
                    tokio::spawn(async move {
                        if let Err(e) = h.await {
                            tracing::error!(uid, "forward_request task panicked: {e}");
                        }
                    });
                } else {
                    debug!("run_relay uid={uid}: request while connecting, replying Connecting");
                    let _ = request.response_tx.send(WorkerResponse::Connecting);
                }
                None
            }
            None => {
                warn!("WorkerRelay uid={uid}: request channel closed");
                Some(DisconnectReason::Other {
                    message: "request channel closed".into(),
                })
            }
        }
    }

    async fn handle_relay_heartbeat(
        uid: u32,
        child_pid: u32,
        client: &FsWorkerServiceClient,
        status_tx: &channel::Tx<WorkerStatus>,
        connected: &mut bool,
        last_heartbeat: &mut Instant,
        heartbeat_failures: &mut u32,
    ) -> Option<DisconnectReason> {
        if !Self::is_process_alive(child_pid) {
            warn!("WorkerRelay uid={uid}: child_pid={child_pid} process not alive (kill 0 failed)");
            return Some(DisconnectReason::Other {
                message: "process died, cause unknown".into(),
            });
        }

        if *connected {
            match tokio::time::timeout(HEARTBEAT_TIMEOUT, client.ping(tarpc::context::current()))
                .await
            {
                Ok(Ok(true)) => {
                    *last_heartbeat = Instant::now();
                    *heartbeat_failures = 0;
                }
                Ok(Ok(false)) => {
                    *heartbeat_failures += 1;
                    warn!(
                        "WorkerRelay uid={uid}: heartbeat returned false ({heartbeat_failures}/2)"
                    );
                    if *heartbeat_failures >= 2 {
                        return Some(DisconnectReason::HeartbeatTimeout {
                            last_heartbeat: *last_heartbeat,
                        });
                    }
                }
                Ok(Err(e)) => {
                    *heartbeat_failures += 1;
                    warn!(
                        "WorkerRelay uid={uid}: heartbeat RPC failed ({heartbeat_failures}/2): {e}"
                    );
                    if *heartbeat_failures >= 2 {
                        return Some(DisconnectReason::ConnectionLost {
                            error: format!("heartbeat RPC failed: {e}"),
                        });
                    }
                }
                Err(_) => {
                    *heartbeat_failures += 1;
                    warn!("WorkerRelay uid={uid}: heartbeat timeout ({heartbeat_failures}/2)");
                    if *heartbeat_failures >= 2 {
                        return Some(DisconnectReason::HeartbeatTimeout {
                            last_heartbeat: *last_heartbeat,
                        });
                    }
                }
            }
        } else {
            match tokio::time::timeout(CONNECT_TIMEOUT, client.ping(tarpc::context::current()))
                .await
            {
                Ok(Ok(true)) => {
                    *connected = true;
                    *last_heartbeat = Instant::now();
                    *heartbeat_failures = 0;
                    info!("WorkerRelay uid={uid}: connected to child_pid={child_pid}");
                    let _ = status_tx.send(WorkerStatus::Connected {
                        uid,
                        pid: child_pid,
                    });
                }
                _ => {
                    debug!("WorkerRelay uid={uid}: waiting for first ping (connecting)...");
                }
            }
        }
        None
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
        if let WorkerRequestContent::StatVfs { ref path } = content {
            let ctx = tarpc::context::current();
            return match client.stat_vfs(ctx, path.clone()).await {
                Ok(Ok((total, free))) => WorkerResponse::StatVfsResult {
                    total_bytes: total,
                    free_bytes: free,
                },
                Ok(Err(e)) => WorkerResponse::Err(e),
                Err(e) => WorkerResponse::Err(format!("RPC error: {e}")),
            };
        }

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
            WorkerRequestContent::RunCreate { op_id, path, kind } => {
                client.run_create(ctx, op_id, path, kind).await
            }
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
            WorkerRequestContent::StatVfs { .. } => {
                unreachable!("handled before match")
            }
            WorkerRequestContent::WatchBreadcrumb { watch_id, path } => {
                client.watch_breadcrumb(ctx, watch_id, path).await
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
        let (parent_req, child_req) = StdUnixStream::pair()?;
        let (parent_cb, child_cb) = StdUnixStream::pair()?;
        clear_cloexec(child_req.as_raw_fd())?;
        clear_cloexec(child_cb.as_raw_fd())?;

        let exe_path = get_exe_path();

        let mut cmd = if target_uid == unistd::getuid().as_raw() {
            let mut c = Command::new(&exe_path);
            if is_appimage() {
                c.current_dir(FALLBACK_CWD);
            }
            c
        } else {
            let mut c = Command::new("pkexec");
            c.arg("--user").arg(target_uid.to_string()).arg(&exe_path);
            if is_appimage() {
                c.current_dir(FALLBACK_CWD);
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
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped());

        let (mut child, pid) = tokio::task::spawn_blocking(move || {
            let child = cmd.spawn()?;
            let pid = child.id();
            Ok::<(std::process::Child, u32), io::Error>((child, pid))
        })
        .await
        .unwrap_or_else(|e| Err(io::Error::other(e.to_string())))?;
        info!("spawned fs-worker pid={pid} fs_worker_id={fs_worker_id} target_uid={target_uid}");

        drop(child_req);
        drop(child_cb);

        let stderr_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let stderr_snapshot = stderr_buf.clone();
        if let Some(mut stderr_reader) = child.stderr.take() {
            let h = tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let mut buf = String::new();
                let _ = stderr_reader.read_to_string(&mut buf);
                *stderr_snapshot.lock().unwrap() = buf;
            });
            tokio::spawn(async move {
                if let Err(e) = h.await {
                    tracing::error!("stderr_reader task panicked: {e}");
                }
            });
        }

        let cb = tokio::task::spawn_blocking(move || {
            let exit_status = child.wait();
            let stderr_text = stderr_buf.lock().unwrap().clone();

            match exit_status {
                Ok(status) => {
                    if stderr_text.is_empty() {
                        info!(
                            "fs-worker pid={pid} fs_worker_id={fs_worker_id} exited with {status}"
                        );
                    } else {
                        warn!(
                            "fs-worker pid={pid} fs_worker_id={fs_worker_id} exited with {status}, stderr: {stderr_text}"
                        );
                    }
                }
                Err(e) => {
                    let extra = if stderr_text.is_empty() {
                        String::new()
                    } else {
                        format!(" stderr={stderr_text}")
                    };
                    warn!("fs-worker pid={pid} wait error: {e}{extra}");
                }
            }
        });
        tokio::spawn(async move {
            if let Err(e) = cb.await {
                tracing::error!(pid, fs_worker_id, "child_waiter task panicked: {e}");
            }
        });

        parent_req.set_nonblocking(true)?;
        let req_stream = UnixStream::from_std(parent_req)?;
        let req_transport = tarpc::serde_transport::new(
            crate::mesh::transport::frame_stream(req_stream),
            tarpc::tokio_serde::formats::Bincode::default(),
        );
        let client =
            FsWorkerServiceClient::new(tarpc::client::Config::default(), req_transport).spawn();

        parent_cb.set_nonblocking(true)?;
        let cb_stream = UnixStream::from_std(parent_cb)?;
        serve_callback(cb_stream, registry);

        Ok((pid, client))
    }
}

/// SIGTERM 然后（SIGKILL_DELAY 后）SIGKILL 终止 Worker 进程。
pub(crate) fn kill_fs_worker(uid: u32, pid: u32) {
    use nix::sys::signal;
    use nix::unistd::Pid;
    if pid == 0 {
        return;
    }
    let p = Pid::from_raw(pid as i32);
    debug!("killing fs-worker uid={uid} pid={pid}");
    let _ = signal::kill(p, signal::Signal::SIGTERM);
    let h = tokio::spawn(async move {
        tokio::time::sleep(SIGKILL_DELAY).await;
        let _ = signal::kill(p, signal::Signal::SIGKILL);
    });
    // 后台监控 panic（极低概率，但 AGENTS.md 要求所有 spawn 处理错误）
    tokio::spawn(async move {
        if let Err(e) = h.await {
            tracing::error!(uid, pid, "kill_fs_worker SIGKILL task panicked: {e}");
        }
    });
}
