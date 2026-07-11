//! 实例发现与复用。
//!
//! 不再有 primary。通过扫描实例目录发现已有实例并复用。

use std::{io, path::PathBuf};

use tarpc::context;
use tokio::net::UnixStream;
use tracing::{error, info};

use crate::instance_bus;
use crate::ipc::protocol::InstanceServiceClient;

/// 尝试请求已有实例打开新窗口。
///
/// 返回 `true` 表示请求已发送（调用方应 `exit(0)`），
/// 返回 `false` 表示无可复用实例（调用方应启动新实例）。
///
/// 如果 `instance_id` 指定，则优先连该实例。
pub async fn run_app_reuse(instance_id: Option<u64>, paths: &[String]) -> bool {
    let sockets = instance_bus::discover_sockets();

    if sockets.is_empty() {
        info!("no running instances found, will start new one");
        return false;
    }

    // 优先连指定的实例
    let candidates: Vec<(u64, PathBuf)> = if let Some(target) = instance_id {
        sockets
            .into_iter()
            .filter(|(id, _)| *id == target)
            .collect()
    } else {
        sockets
    };

    if candidates.is_empty() {
        info!("specified instance not found, will start new one");
        return false;
    }

    for (instance_id, path) in &candidates {
        if let Err(e) = send_open_window(*instance_id, path, paths).await {
            error!("open_window to instance {instance_id} failed: {e}");
        } else {
            info!("sent open_window to instance {instance_id}, exiting");
            return true;
        }
    }

    info!("no reachable instance found, will start new one");
    false
}

async fn send_open_window(
    _instance_id: u64,
    path: &PathBuf,
    tabs_paths: &[String],
) -> io::Result<()> {
    let stream = UnixStream::connect(path).await?;

    let transport = tarpc::serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    let client = InstanceServiceClient::new(tarpc::client::Config::default(), transport).spawn();

    client
        .open_window(context::current(), tabs_paths.to_vec())
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;

    Ok(())
}
