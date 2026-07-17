//! Mesh 实例发现层。
//!
//! 管理 `~/.cache/hnfm/instances/` 目录下的 socket 和 lock 文件。
//! 提供绑定、存活检查、清理等工具函数。`watch_instances` 后台任务
//! 仍在 `instance_bus` 中（与 `InstanceBus` 紧密耦合）。

use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

use tracing::{info, warn};

// --
// 路径工具
// --

/// `~/.cache/hnfm/instances/` 目录。
pub(crate) fn instances_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    PathBuf::from(home)
        .join(".cache")
        .join("hnfm")
        .join("instances")
}

/// 实例的 lock 文件路径。
pub(crate) fn lock_path(instance_id: u64) -> PathBuf {
    instances_dir().join(format!("instance_{instance_id}.lock"))
}

/// 本实例的 socket 文件路径。
pub(crate) fn socket_path(instance_id: u64) -> PathBuf {
    instances_dir().join(format!("instance_{instance_id}.sock"))
}

// --
// 存活检查
// --

/// 检查 PID 是否存活。
fn pid_alive(pid: u64) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// 检查实例是否存活。
///
/// 优先通过 lock 文件的 flock 判断，无 lock 文件时回退到 `pid_alive()`
/// 以兼容旧版本实例（未持有 lock 文件）。
pub(crate) fn is_instance_alive(instance_id: u64) -> bool {
    let lock = lock_path(instance_id);
    if let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create(false)
        .open(&lock)
    {
        let fd = file.as_raw_fd();
        let held = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) != 0 };
        if !held {
            unsafe { libc::flock(fd, libc::LOCK_UN) };
        }
        drop(file);
        return held;
    }
    pid_alive(instance_id)
}

/// 公开的存活检查，供 `main.rs` 早期冲突检测使用。
pub fn instance_exists(instance_id: u64) -> bool {
    is_instance_alive(instance_id)
}

// --
// 扫描
// --

/// 扫描实例目录，返回 `(instance_id, socket_path)` 列表。
///
/// 通过 lock 文件 flock 判断存活，同时清理残骸。
pub(crate) fn discover_sockets() -> Vec<(u64, PathBuf)> {
    let dir = instances_dir();
    let mut result = Vec::new();

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if let Some(pid_str) = name
            .strip_prefix("instance_")
            .and_then(|s| s.strip_suffix(".sock"))
        {
            if let Ok(id) = pid_str.parse::<u64>() {
                if is_instance_alive(id) {
                    result.push((id, path));
                } else {
                    warn!("discover_sockets: stale socket for instance {id}, removing {path:?}");
                    if let Err(e) = std::fs::remove_file(&path) {
                        warn!("failed to remove stale socket {path:?}: {e}");
                    }
                    if let Err(e) = std::fs::remove_file(&lock_path(id)) {
                        warn!("failed to remove stale lock for instance {id}: {e}");
                    }
                }
            }
        }
    }

    result
}

// --
// 绑定与清理
// --

/// 绑定本实例的 Unix Domain Socket（含竞态保护）。
pub(crate) fn bind_socket(instance_id: u64) -> io::Result<tokio::net::UnixListener> {
    let dir = instances_dir();
    std::fs::create_dir_all(&dir)?;

    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(lock_path(instance_id))?;
    let fd = lock_file.as_raw_fd();
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("instance {instance_id} is already running"),
        ));
    }
    // SAFETY: lock_file holds an exclusive flock on the instance lock file.
    // We intentionally forget it so the lock persists for the process lifetime.
    // The OS releases the flock automatically when the process exits, so there
    // is no resource leak. If bind_socket is called twice for the same instance,
    // the second call correctly returns AddrInUse -- the first lock is still held.
    std::mem::forget(lock_file);

    let path = socket_path(instance_id);
    if let Err(e) = std::fs::remove_file(&path) {
        warn!("failed to remove stale socket before bind {path:?}: {e}");
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    info!("instance {instance_id} listening on {path:?}");
    Ok(listener)
}

/// 退出时删除本实例的 socket 和 lock 文件。
pub(crate) fn cleanup_socket(instance_id: u64) {
    let path = socket_path(instance_id);
    if let Err(e) = std::fs::remove_file(&path) {
        warn!("failed to cleanup socket {path:?}: {e}");
    } else {
        info!("cleaned up socket {path:?}");
    }
    let lock = lock_path(instance_id);
    if let Err(e) = std::fs::remove_file(&lock) {
        warn!("failed to cleanup lock {lock:?}: {e}");
    }
}
