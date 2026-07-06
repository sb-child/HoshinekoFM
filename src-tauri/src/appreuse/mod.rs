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
//!    - 否则 → 连接 primary，发送打开新窗口请求，自身退出

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
// 实例间 RPC (tarpc)
// ---------------------------------------------------------------------------
//
// 实例间通信使用 tarpc 框架，传输层为：
//   - 编解码: `LengthDelimitedCodec` (帧边界) + `Bincode` (序列化)
//   - 传输: `tokio::net::UnixStream` (Unix Domain Socket)
//   - 服务定义: `crate::ipc::protocol::InstanceService`
//
// 每次 RPC 调用都是短连接：连接 → 发送请求 → 断开。

/// 为给定的 UnixStream 创建 tarpc 客户端。
///
/// 封装了 `LengthDelimitedCodec` + `Bincode` 传输层初始化样板代码，
/// 避免在多个请求函数中重复。
///
/// # 返回
/// - `InstanceServiceClient` —— 可直接调用 `.open_tabs()` / `.transfer_tab()` / `.ping()`
fn make_instance_client(stream: UnixStream) -> InstanceServiceClient {
    let transport = tarpc::serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    InstanceServiceClient::new(tarpc::client::Config::default(), transport).spawn()
}

/// 请求 primary 实例打开新窗口。
///
/// 通过 tarpc RPC 调用 primary 实例的 `open_tabs()`，
/// 该调用会在 primary 进程中创建一个新窗口并为每个 path 创建 tab。
///
/// # 参数
/// - `stream`: 已连接 primary 实例的 UnixStream
/// - `paths`: 新窗口中要打开的路径列表
pub async fn request_open_window(stream: UnixStream, paths: &[String]) -> io::Result<()> {
    let client = make_instance_client(stream);

    if let Err(e) = client
        .open_tabs(context::current(), paths.to_vec())
        .await
    {
        error!("failed to request open_window: {e}");
    } else {
        info!("sent open_window request: {paths:?}");
    }
    Ok(())
}

/// 请求 primary 实例接收一个跨实例迁移的 tab。
///
/// 通过 tarpc RPC 调用 primary 实例的 `transfer_tab()`，
/// 将序列化后的 `TabState` 发送给目标实例。
///
/// # 参数
/// - `stream`: 已连接目标实例的 UnixStream
/// - `tab`: 要迁移的标签页完整状态
pub async fn request_transfer_tab(stream: UnixStream, tab: TabState) -> io::Result<()> {
    let client = make_instance_client(stream);
    let tab_id = tab.id;

    if let Err(e) = client
        .transfer_tab(context::current(), tab)
        .await
    {
        error!("failed to request transfer_tab: {e}");
    } else {
        info!("sent transfer_tab request: id={tab_id}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// run_app_reuse
// ---------------------------------------------------------------------------

/// 连接到指定 instance_id，发送打开新窗口请求，然后退出进程。
pub async fn run_app_reuse(instance_id: Option<u64>, paths: &[String]) -> ! {
    let connect_result = if let Some(id) = instance_id {
        connect_to_instance(id).await
    } else {
        connect_to_primary().await
    };

    match connect_result {
        Ok(stream) => {
            request_open_window(stream, paths).await.ok();
            info!("sent open new window request to primary, exiting");
            std::process::exit(0);
        }
        Err(e) => {
            error!("failed to connect to instance: {e}");
            std::process::exit(1);
        }
    }
}
