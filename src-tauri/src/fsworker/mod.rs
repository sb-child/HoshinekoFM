//! 文件系统 Worker 管理。
//!
//! ## 架构
//!
//! - `FsWorkerPool`: 管理所有 Worker 实例 (按 UID 分)
//! - `FsWorkerHandle`: 单个 Worker 连接 + 引用计数 + 忙碌状态
//! - `run_fs_worker`: Worker 子进程入口 (见 `worker.rs`)
//!
//! ## 生命周期
//!
//! ```
//! FsWorkerPool::acquire(uid)
//!   ├─ 已有 Worker → ref_count += 1
//!   └─ 无 Worker   → spawn → ref_count = 1
//!
//! FsWorkerPool::release(uid)
//!   ├─ ref_count -= 1
//!   └─ ref_count == 0 → 30s 闲置超时后关闭
//! ```

use std::{
    collections::HashMap,
    io,
    os::unix::{
        io::{AsRawFd, RawFd},
        net::UnixStream as StdUnixStream,
    },
    process::{Command, Stdio},
    time::Duration,
};

use nix::{
    fcntl::{fcntl, FcntlArg, FdFlag},
    unistd,
};
use tokio::net::UnixStream;
use tracing::info;

use crate::ipc::protocol::FsWorkerServiceClient;

pub mod worker;

// ---------------------------------------------------------------------------
// WorkerOpts
// ---------------------------------------------------------------------------

/// Worker 启动选项（传递给 `hnfm fs-worker` 的命令行参数）。
#[derive(Debug, Clone)]
pub struct FsWorkerOpts {
    /// Worker ID
    pub worker_id: u64,
    /// 继承的文件描述符（匿名 socketpair 的子端）
    pub fd: Option<i32>,
}

// ---------------------------------------------------------------------------
// FsWorkerHandle
// ---------------------------------------------------------------------------

/// 一个活跃的 Worker 连接。
pub struct FsWorkerHandle {
    /// tarpc 客户端
    pub client: FsWorkerServiceClient,
    /// 引用计数
    pub ref_count: u64,
    /// 是否正在执行批量操作 (copy/move/delete)
    pub busy: bool,
    /// 目标 UID (0 = root)
    pub target_uid: u32,
    /// 子进程 PID（用于终止超时 Worker）
    pub pid: u32,
}

impl FsWorkerHandle {
    /// 标记为忙碌。
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// 增加引用计数。
    pub fn inc_ref(&mut self) {
        self.ref_count += 1;
    }

    /// 减少引用计数。
    /// 返回是否降为 0。
    pub fn dec_ref(&mut self) -> bool {
        self.ref_count = self.ref_count.saturating_sub(1);
        self.ref_count == 0
    }
}

// ---------------------------------------------------------------------------
// FsWorkerPool
// ---------------------------------------------------------------------------

/// Worker 实例池，按目标 UID 索引。
pub struct FsWorkerPool {
    /// uid → FsWorkerHandle
    workers: HashMap<u32, FsWorkerHandle>,
    /// 闲置超时时间
    idle_timeout: Duration,
}

impl FsWorkerPool {
    /// 创建新的 FsWorkerPool。
    pub fn new() -> Self {
        Self {
            workers: HashMap::new(),
            idle_timeout: Duration::from_secs(30),
        }
    }

    /// 获取或创建一个指定 UID 的 Worker。
    ///
    /// 返回 Worker 的 `(uid, &mut FsWorkerHandle)`。
    pub async fn acquire(&mut self, target_uid: u32) -> io::Result<(u32, &mut FsWorkerHandle)> {
        // 使用 entry API 避免 borrow checker 问题
        use std::collections::hash_map::Entry;

        match self.workers.entry(target_uid) {
            Entry::Occupied(mut entry) => {
                let handle = entry.get_mut();
                if !handle.is_zombie() {
                    handle.inc_ref();
                } else {
                    // 僵尸 Worker，需要重建
                    let (pid, client) = Self::spawn_worker_inner(target_uid).await?;
                    *handle = FsWorkerHandle {
                        ref_count: 1,
                        busy: false,
                        target_uid,
                        pid,
                        client,
                    };
                }
            }
            Entry::Vacant(entry) => {
                let (pid, client) = Self::spawn_worker_inner(target_uid).await?;

                entry.insert(FsWorkerHandle {
                    ref_count: 1,
                    busy: false,
                    target_uid,
                    pid,
                    client,
                });
            }
        }

        // 此时 self.workers 已解锁，可以安全获取引用
        Ok((
            target_uid,
            self.workers
                .get_mut(&target_uid)
                .expect("worker just inserted"),
        ))
    }

    /// 释放指定 UID 的 Worker 引用。
    ///
    /// ref_count 降为 0 时启动 30s 闲置计时器。
    pub async fn release(&mut self, target_uid: u32) {
        let should_schedule_drop = {
            let handle = self.workers.get_mut(&target_uid);
            handle.map_or(false, |h| h.dec_ref())
        };

        if should_schedule_drop {
            info!(
                "worker for uid {target_uid} ref_count=0, scheduling drop in {:?}",
                self.idle_timeout
            );
            // 复制 uid 用于 async drop
            let uid = target_uid;
            let timeout = self.idle_timeout;
            // 启动闲置超时任务
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                info!("worker for uid {uid} idle timeout, would drop now");
            });
        }
    }

    /// 获取指定 UID 的 Worker（不改变引用计数）。
    pub fn get(&self, target_uid: u32) -> Option<&FsWorkerHandle> {
        self.workers.get(&target_uid)
    }

    /// 获取指定 UID 的 Worker（可变引用，不改变引用计数）。
    pub fn get_mut(&mut self, target_uid: u32) -> Option<&mut FsWorkerHandle> {
        self.workers.get_mut(&target_uid)
    }

    /// 检查指定 Worker 是否繁忙。
    pub fn is_busy(&self, target_uid: u32) -> bool {
        self.workers
            .get(&target_uid)
            .map_or(false, |h| h.busy)
    }

    /// 启动一个新的 FS Worker 子进程（静态方法，不借 &mut self）。
    async fn spawn_worker_inner(target_uid: u32) -> io::Result<(u32, FsWorkerServiceClient)> {
        let worker_id = unistd::getpid().as_raw() as u64; // 用 PID 派生 worker_id

        // 1. 创建匿名 socketpair
        let (parent_sock, child_sock) = StdUnixStream::pair()?;

        // 2. 清除 child_sock 的 CLOEXEC 标志，使子进程能继承它
        let child_raw_fd = child_sock.as_raw_fd();
        clear_cloexec(child_raw_fd)?;

        // 3. 获取可执行文件路径
        let exe_path = get_exe_path();

        // 4. 构建子进程命令
        let mut cmd = if target_uid == unistd::getuid().as_raw() {
            // 同 UID，直接启动
            let mut c = Command::new(&exe_path);
            if is_appimage() {
                c.current_dir("/tmp");
            }
            c
        } else {
            // 提权到目标 UID
            let mut c = Command::new("pkexec");
            c.arg("--user")
                .arg(target_uid.to_string())
                .arg(&exe_path);
            if is_appimage() {
                c.current_dir("/tmp");
            }
            c
        };

        cmd.args([
            "fs-worker",
            &format!("--worker-id={worker_id}"),
            &format!("--fd={child_raw_fd}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        // 5. 启动子进程
        let mut child = cmd.spawn()?;
        let pid = child.id();
        info!("spawned fs-worker pid={pid} worker_id={worker_id} target_uid={target_uid}");

        // 6. 关闭 child_sock（子进程已有 fd 副本）
        drop(child_sock);

        // 7. 后台回收子进程（避免僵尸进程）
        tokio::task::spawn_blocking(move || {
            let exit = child.wait();
            match exit {
                Ok(status) => info!("fs-worker pid={pid} exited with {status}"),
                Err(e) => info!("fs-worker pid={pid} wait error: {e}"),
            }
        });

        // 8. 设置 parent_sock 为非阻塞并转为 tokio stream
        parent_sock.set_nonblocking(true)?;
        let stream = UnixStream::from_std(parent_sock)?;

        // 9. 构建 tarpc transport 和 client
        let codec_builder =
            tarpc::tokio_util::codec::length_delimited::Builder::new();
        let transport = tarpc::serde_transport::new(
            codec_builder.new_framed(stream),
            tarpc::tokio_serde::formats::Bincode::default(),
        );

        let client =
            FsWorkerServiceClient::new(tarpc::client::Config::default(), transport).spawn();

        Ok((pid, client))
    }
}

impl Default for FsWorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// FsWorkerHandle 辅助
// ---------------------------------------------------------------------------

impl FsWorkerHandle {
    /// 是否应该被回收（子进程已退出或连接断开）。
    pub fn is_zombie(&self) -> bool {
        // TODO: 通过 ping 或检查 client 状态检测连接是否存活
        false
    }
}

// ---------------------------------------------------------------------------
// FD 工具
// ---------------------------------------------------------------------------

/// 清除文件描述符的 CLOEXEC 标志。
fn clear_cloexec(fd: RawFd) -> io::Result<()> {
    use std::os::fd::BorrowedFd;

    // SAFETY: fd is a valid file descriptor at this point
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };

    let mut flags = FdFlag::from_bits_retain(
        fcntl(borrowed, FcntlArg::F_GETFD)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?,
    );
    flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(borrowed, FcntlArg::F_SETFD(flags))
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 路径工具
// ---------------------------------------------------------------------------

/// 获取当前可执行文件路径。
///
/// 处理 AppImage 场景：如果在 AppImage 中运行，返回 `.AppImage` 文件本身。
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

// ---------------------------------------------------------------------------
// Worker 入口 (re-export)
// ---------------------------------------------------------------------------

pub use worker::run_fs_worker;
