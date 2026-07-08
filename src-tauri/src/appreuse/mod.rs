//! 实例发现与复用。
//!
//! 不再有 primary。通过扫描实例目录发现已有实例并复用。

use std::{io, path::PathBuf};

use tarpc::context;
use tokio::net::UnixStream;
use tracing::{error, info};

use crate::ipc::protocol::InstanceServiceClient;

/// 尝试将 `open_tabs` 发送到已有实例。
///
/// 返回 `true` 表示 tabs 已发送（调用方应 `exit(0)`），
/// 返回 `false` 表示无可复用实例（调用方应启动新实例）。
///
/// 如果 `instance_id` 指定，则优先连该实例。
pub async fn run_app_reuse(instance_id: Option<u64>, paths: &[String]) -> bool {
    let sockets = discover_sockets();

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
        if let Err(e) = send_open_tabs(*instance_id, path, paths).await {
            error!("open_tabs to instance {instance_id} failed: {e}");
        } else {
            info!("sent open_tabs to instance {instance_id}, exiting");
            return true;
        }
    }

    info!("no reachable instance found, will start new one");
    false
}

async fn send_open_tabs(
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
        .open_tabs(context::current(), tabs_paths.to_vec())
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;

    Ok(())
}

fn discover_sockets() -> Vec<(u64, PathBuf)> {
    let dir = match instances_dir() {
        Ok(d) => d,
        Err(_) => return vec![],
    };

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
            if let Ok(pid) = pid_str.parse::<u64>() {
                result.push((pid, path));
            }
        }
    }

    result
}

fn instances_dir() -> io::Result<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let dir = PathBuf::from(format!("{home}/.cache/hnfm/instances"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
