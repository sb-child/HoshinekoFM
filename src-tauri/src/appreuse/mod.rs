//! 多实例管理：实例注册中心与 Primary 争夺。
//!
//! ## 注册目录
//!
//! ```text
//! ~/.cache/hnfm/instances/
//! ├── primary -> instance_1024.sock   # 原子 symlink，指向默认主实例
//! ├── instance_1024.sock              # 实例 A 的监听 socket
//! └── instance_2048.sock              # 实例 B 的监听 socket
//! ```
//!
//! ## 启动流程
//!
//! 1. 调用 `try_acquire_primary()`：原子 `symlink("instance_<pid>.sock", "primary")`
//! 2. 如果成功 → 自身成为 primary，启动 Tauri + 监听实例间连接
//! 3. 如果失败：
//!    - `--new-instance` → 仍启动，但不设 primary
//!    - 否则 → 连接 primary，发送 `open_tabs`，自身退出

use std::{fs, io, path::PathBuf};

use nix::unistd;
use tarpc::context;
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, info, warn};

use crate::ipc::protocol::{InstanceServiceClient, TabState};

/// 实例注册目录基路径。
const INSTANCES_DIR: &str = ".cache/hnfm/instances";

/// 确保实例注册目录存在并返回路径。
fn ensure_instances_dir() -> PathBuf {
    let home = dirs_home();
    let dir = home.join(INSTANCES_DIR);
    fs::create_dir_all(&dir).expect("failed to create instances dir");
    dir
}

/// 获取用户 home 目录。
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

// ---------------------------------------------------------------------------
// Primary 争夺
// ---------------------------------------------------------------------------

/// 尝试抢占 primary。
///
/// 使用原子 `symlink("instance_<pid>.sock", "primary")` 实现互斥。
///
/// 返回 `(be_primary, instance_id)`，`instance_id` 是当前进程 PID。
pub fn try_acquire_primary() -> (bool, u64) {
    let instance_id = unistd::getpid().as_raw() as u64;
    let instances_dir = ensure_instances_dir();
    let socket_name = format!("instance_{}.sock", instance_id);
    let primary_link = instances_dir.join("primary");
    let socket_path = instances_dir.join(&socket_name);

    // 检查 primary symlink 是否存在（用 symlink_metadata 而非 exists，后者会 follow symlink）
    if let Ok(meta) = fs::symlink_metadata(&primary_link) {
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(&primary_link) {
                // target 不存在 → 悬空死链
                if !target.exists() {
                    let _ = fs::remove_file(&primary_link);
                    info!("removed dangling primary symlink at {primary_link:?}");
                } else if let Err(e) = std::os::unix::net::UnixStream::connect(&target) {
                    // target 存在但无法连接 → 目标进程已死
                    let _ = fs::remove_file(&primary_link);
                    let _ = fs::remove_file(&target);
                    info!(
                        "removed stale primary (socket unreachable: {e}), cleaned up {target:?}"
                    );
                } else {
                    // 连接成功 → primary 仍然存活
                    info!("primary instance is alive at {target:?}");
                    return (false, instance_id);
                }
            }
        }
    }

    // 原子 symlink 抢 primary
    debug!("attempting to acquire primary (symlink {socket_name:?} -> primary)");
    match std::os::unix::fs::symlink(&socket_path, &primary_link) {
        Ok(()) => {
            info!("acquired primary (instance {instance_id})");
            (true, instance_id)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            info!("primary already taken, running as secondary instance {instance_id}");
            (false, instance_id)
        }
        Err(e) => {
            warn!("failed to acquire primary: {e}, running as secondary");
            (false, instance_id)
        }
    }
}

/// 释放 primary 并清理资源。
///
/// 删除 primary symlink 和实例 socket 文件。
fn cleanup_instance(instance_id: u64) {
    let instances_dir = ensure_instances_dir();
    let primary_link = instances_dir.join("primary");
    let socket_path = instances_dir.join(format!("instance_{}.sock", instance_id));

    // 只有当 primary 指向我们自己时才删除
    if let Ok(target) = fs::read_link(&primary_link) {
        if target == socket_path {
            let _ = fs::remove_file(&primary_link);
            info!("removed primary symlink for instance {instance_id}");
        }
    }

    // 删除自己的 socket 文件
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
        debug!("removed socket file for instance {instance_id}");
    }
}

/// 释放 primary（删除 symlink）。
///
/// 仍在语义上保留此函数供外部调用。
pub fn release_primary(instance_id: u64) {
    cleanup_instance(instance_id);
}

// ---------------------------------------------------------------------------
// Primary 监听
// ---------------------------------------------------------------------------

/// 启动 primary 的实例间连接监听器。
pub async fn start_primary_listener(instance_id: u64) -> io::Result<UnixListener> {
    let instances_dir = ensure_instances_dir();
    let socket_path = instances_dir.join(format!("instance_{}.sock", instance_id));

    let _ = fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    info!("instance {instance_id} listening on {socket_path:?}");
    Ok(listener)
}

// ---------------------------------------------------------------------------
// 连接到指定实例
// ---------------------------------------------------------------------------

/// 连接到指定实例 ID。
pub async fn connect_to_instance(instance_id: u64) -> io::Result<UnixStream> {
    let instances_dir = ensure_instances_dir();
    let socket_path = instances_dir.join(format!("instance_{}.sock", instance_id));
    debug!("connecting to instance {instance_id} at {socket_path:?}");
    UnixStream::connect(&socket_path).await
}

/// 连接到当前 primary 实例。
pub async fn connect_to_primary() -> io::Result<UnixStream> {
    let instances_dir = ensure_instances_dir();
    let primary_link = instances_dir.join("primary");
    let target = fs::read_link(&primary_link)?;
    UnixStream::connect(&target).await
}

// ---------------------------------------------------------------------------
// 实例间消息发送 (tarpc)
// ---------------------------------------------------------------------------

/// 向实例发送 `open_tabs` 请求。
pub async fn send_open_tabs(stream: UnixStream, paths: &[String]) -> io::Result<()> {
    use tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec;

    let codec_builder = LengthDelimitedCodec::builder();
    let framed = codec_builder.new_framed(stream);
    let transport = tarpc::serde_transport::new(framed, tarpc::tokio_serde::formats::Bincode::default());

    let client = InstanceServiceClient::new(tarpc::client::Config::default(), transport).spawn();

    if let Err(e) = client
        .open_tabs(context::current(), paths.to_vec())
        .await
    {
        error!("failed to send open_tabs: {e}");
    } else {
        info!("sent open_tabs request: {paths:?}");
    }
    Ok(())
}

/// 向实例发送 `transfer_tab` 请求。
pub async fn send_transfer_tab(stream: UnixStream, tab: TabState) -> io::Result<()> {
    use tarpc::tokio_util::codec::length_delimited::LengthDelimitedCodec;

    let codec_builder = LengthDelimitedCodec::builder();
    let framed = codec_builder.new_framed(stream);
    let transport = tarpc::serde_transport::new(framed, tarpc::tokio_serde::formats::Bincode::default());

    let client = InstanceServiceClient::new(tarpc::client::Config::default(), transport).spawn();

    if let Err(e) = client
        .transfer_tab(context::current(), tab)
        .await
    {
        error!("failed to send transfer_tab: {e}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// run_app_reuse
// ---------------------------------------------------------------------------

/// 连接到指定 instance_id，发送 `open_tabs` 请求，然后退出进程。
pub async fn run_app_reuse(instance_id: Option<u64>, paths: &[String]) -> ! {
    let connect_result = if let Some(id) = instance_id {
        connect_to_instance(id).await
    } else {
        connect_to_primary().await
    };

    match connect_result {
        Ok(stream) => {
            send_open_tabs(stream, paths).await.ok();
            info!("sent open_tabs request, exiting");
            std::process::exit(0);
        }
        Err(e) => {
            error!("failed to connect to instance: {e}");
            std::process::exit(1);
        }
    }
}
