use std::collections::HashMap;
use std::path::PathBuf;

use notify::{RecursiveMode, Watcher};
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use crate::channel::{self, RxAsync, Tx};

use super::config::WatchConfig;

// --
// 控制命令
// --

/// Registry 发给 InotifyManager 的控制命令。
pub enum InotifyCmd {
    Watch { path: PathBuf, is_dir: bool },
    Unwatch { path: PathBuf },
    Rewatch { path: PathBuf, is_dir: bool },
    Shutdown,
}

/// InotifyManager 产出的原始事件。
#[derive(Debug, Clone)]
pub struct RawEvent {
    /// 受影响的 watch target 路径。
    pub path: PathBuf,
    /// inotify 事件中携带的具体受影响的路径列表。
    pub affected_paths: Vec<PathBuf>,
}

// --
// InotifyManager
// --

/// watch 表条目，区分原始 watch 和 dual-watch（父目录）。
struct WatchEntry {
    is_dir: bool,
    /// 若为 dual-watch 父目录，路由时不用 starts_with(t.parent()) 匹配，
    /// 而是仅匹配 p==t（父目录自身变化）或 p.parent()==t（直接子项变化）。
    is_dual: bool,
}

/// 进程级单例 inotify 管理器。
///
/// 整个 fsworker 进程仅有一个 inotify 实例，所有目录/文件 watch
/// 通过添加 watch descriptor 共享。虚拟文件系统（/proc /sys）走
/// 独立 AsyncFd poll。
pub struct InotifyManager {
    /// 接收外部控制命令（Watch / Unwatch / Rewatch / Shutdown）。
    cmd_rx: RxAsync<InotifyCmd>,
    /// 向 WatchScheduler 推送原始事件。
    event_tx: Tx<RawEvent>,
    /// 取消令牌。
    cancel: CancellationToken,
    /// 运行时配置。
    config: WatchConfig,
}

impl InotifyManager {
    /// 创建 InotifyManager 并返回启动所需的通道对。
    ///
    /// 调用方获得：
    /// - `(cmd_tx, event_rx)` -- 用于向管理器发送命令、从管理器接收事件。
    /// - `Self` -- 持有消费端。
    /// - `CancellationToken` -- 传递给调用方用于取消。
    pub fn spawn(
        cancel: CancellationToken,
        config: &WatchConfig,
    ) -> (Tx<InotifyCmd>, RxAsync<RawEvent>, Self) {
        let (cmd_tx, cmd_rx) = channel::unbounded();
        let (event_tx, event_rx) = channel::unbounded();
        (
            cmd_tx,
            event_rx,
            Self {
                cmd_rx,
                event_tx,
                cancel,
                config: config.clone(),
            },
        )
    }

    /// 运行主循环。调用方应 `tokio::spawn(self.run())`。
    #[instrument(skip(self), name = "inotify")]
    pub async fn run(self) {
        info!("InotifyManager starting");

        let (raw_tx, raw_rx) = channel::unbounded::<Vec<PathBuf>>();
        let raw_rx: RxAsync<Vec<PathBuf>> = raw_rx;

        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = raw_tx.send(event.paths);
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    warn!("InotifyManager: failed to create watcher: {e}");
                    return;
                }
            };

        // track watch descriptors: path -> WatchEntry
        let mut watches: HashMap<PathBuf, WatchEntry> = HashMap::new();

        loop {
            select! {
                biased;

                _ = self.cancel.cancelled() => {
                    info!("InotifyManager shutting down");
                    return;
                }

                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Ok(InotifyCmd::Watch { path, is_dir }) => {
                            self.handle_watch(&mut watcher, &mut watches, path.clone(), is_dir);
                        }
                        Ok(InotifyCmd::Unwatch { path }) => {
                            self.handle_unwatch(&mut watcher, &mut watches, &path);
                        }
                        Ok(InotifyCmd::Rewatch { path, is_dir }) => {
                            self.handle_unwatch(&mut watcher, &mut watches, &path);
                            self.handle_watch(&mut watcher, &mut watches, path, is_dir);
                        }
                        Ok(InotifyCmd::Shutdown) => {
                            info!("InotifyManager received Shutdown");
                            self.cancel.cancel();
                        }
                        Err(_) => return,
                    }
                }

                events = raw_rx.recv() => {
                    match events {
                        Ok(paths) => {
                            for p in &paths {
                                if let Some(target) = watches.iter().find_map(|(t, e)| {
                                    let matches = if e.is_dual {
                                        p.parent().map_or(false, |par| par == t.as_path()) || p.as_path() == t.as_path()
                                    } else if e.is_dir {
                                        p.parent().map_or(false, |par| par == t.as_path())
                                    } else {
                                        p.starts_with(t.parent().unwrap_or(t)) || p.as_path() == t.as_path()
                                    };
                                    matches.then_some(t)
                                }) {
                                    let _ = self.event_tx.send(RawEvent {
                                        path: target.clone(),
                                        affected_paths: paths.clone(),
                                    });
                                    break;
                                }
                            }
                        }
                        Err(_) => return,
                    }
                }
            }
        }
    }

    fn handle_watch(
        &self,
        watcher: &mut notify::RecommendedWatcher,
        watches: &mut HashMap<PathBuf, WatchEntry>,
        path: PathBuf,
        is_dir: bool,
    ) {
        if let Some(existing) = watches.get_mut(&path) {
            if existing.is_dual {
                existing.is_dual = false;
                debug!(
                    "InotifyManager: upgraded dual-watch {:?} to primary (is_dir={is_dir})",
                    path
                );
            }
            if is_dir {
                existing.is_dir = true;
            }
            return;
        }
        let parent = path.parent().map(|p| p.to_path_buf());
        match watcher.watch(&path, RecursiveMode::NonRecursive) {
            Ok(()) => {
                debug!("InotifyManager: watching {:?} (is_dir={is_dir})", path);
                watches.insert(
                    path,
                    WatchEntry {
                        is_dir,
                        is_dual: false,
                    },
                );
                // 双 watch：同时监听 parent 以检测类型变更
                if let Some(parent_path) = parent {
                    if !parent_path.as_os_str().is_empty()
                        && parent_path != PathBuf::from("/")
                        && !watches.contains_key(&parent_path)
                    {
                        if let Err(e) = watcher.watch(&parent_path, RecursiveMode::NonRecursive) {
                            warn!(
                                "InotifyManager: dual watch parent {:?} failed: {e}",
                                parent_path
                            );
                        } else {
                            watches.insert(
                                parent_path,
                                WatchEntry {
                                    is_dir: true,
                                    is_dual: true,
                                },
                            );
                        }
                    }
                }
            }
            Err(e) => {
                warn!("InotifyManager: watch {:?} failed: {e}", path);
            }
        }
    }

    fn handle_unwatch(
        &self,
        watcher: &mut notify::RecommendedWatcher,
        watches: &mut HashMap<PathBuf, WatchEntry>,
        path: &PathBuf,
    ) {
        if watches.remove(path).is_some() {
            let _ = watcher.unwatch(path);
            debug!("InotifyManager: unwatched {:?}", path);
        }
    }
}
