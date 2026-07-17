//! 实例目录 notify 监听（Linux: inotify, macOS: FSEvents）。
//!
//! 后台 std 线程阻塞读取 OS 事件，通过 tokio channel 发送给主运行时。
//! 启动时先扫描一次目录，补 TOCTOU 窗口（watch 注册前发生的变更）。

use std::path::PathBuf;

use crossfire::BlockingTxTrait;
use notify::{EventKind, RecursiveMode, Watcher};

use crate::channel;

/// 实例目录事件。
pub enum WatchEvent {
    /// 初始扫描结果（watch 注册后的补漏扫描）。
    Init(Vec<(u64, PathBuf)>),
    /// 新 socket 文件创建。
    New(u64, PathBuf),
    /// socket 文件被删除（实例退出）。
    Gone(u64),
}

/// 启动后台 notify 线程，返回事件接收端。
///
/// 线程内部：
/// 1. 扫描目录 -> 发送 `WatchEvent::Init`
/// 2. 注册 notify watch -> 事件循环发送 `New` / `Gone`
pub fn watch_dir(dir: PathBuf) -> channel::RxAsync<WatchEvent> {
    let (tx, rx) = channel::unbounded::<WatchEvent>();
    let dir_c = dir.clone();

    std::thread::spawn(move || {
        // 1. 初始扫描（补 TOCTOU 窗口）
        if let Ok(entries) = std::fs::read_dir(&dir_c) {
            let mut sockets = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(id) = parse_sock_pid(&path) {
                    sockets.push((id, path));
                }
            }
            if !sockets.is_empty() {
                // receiver 已 drop 时静默退出
                if tx.send(WatchEvent::Init(sockets)).is_err() {
                    return;
                }
            }
        }

        // 2. 注册 notify
        let (raw_tx, raw_rx) = channel::unbounded_blocking();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                let _ = raw_tx.send(res);
            }) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!(
                        "failed to create notify watcher (inotify limit exhausted?): {e}"
                    );
                    return;
                }
            };

        if watcher.watch(&dir_c, RecursiveMode::NonRecursive).is_err() {
            return;
        }

        // 3. 事件循环：blocking channel -> async channel
        loop {
            let event = match raw_rx.recv() {
                Ok(Ok(e)) => e,
                _ => break,
            };
            let Some(path) = event.paths.first() else {
                continue;
            };
            let Some(id) = parse_sock_pid(path) else {
                continue;
            };

            match event.kind {
                EventKind::Create(_) => {
                    if tx.send(WatchEvent::New(id, path.clone())).is_err() {
                        break;
                    }
                }
                EventKind::Remove(_) => {
                    if tx.send(WatchEvent::Gone(id)).is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }
    });

    rx
}

/// 从 socket 文件名提取实例 PID：`instance_<pid>.sock`
fn parse_sock_pid(path: &PathBuf) -> Option<u64> {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|s| s.strip_prefix("instance_"))
        .and_then(|s| s.strip_suffix(".sock"))
        .and_then(|s| s.parse::<u64>().ok())
}
