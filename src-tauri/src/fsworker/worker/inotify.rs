use std::collections::HashMap;
use std::path::PathBuf;

use notify::{RecursiveMode, Watcher};
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use crate::channel::{self, RxAsync, Tx};

use super::config::WatchConfig;

pub enum InotifyCmd {
    Watch { path: PathBuf, scope: WatchScope },
    Unwatch { path: PathBuf },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchScope {
    /// 文件列表用：需要此路径的**所有直接子项**的事件。
    Children,
    /// 面包屑用：仅需要此路径**自身**的属性变化事件。
    SelfOnly,
}

#[derive(Debug, Clone)]
pub struct RawEvent {
    pub path: PathBuf,
    pub affected_paths: Vec<PathBuf>,
    /// 触发此事件的监视范围。SelfOnly 表示目录自身属性变化，Children 表示子项变化。
    pub scope: WatchScope,
}

struct WatchEntry {
    /// 最宽权限：SelfOnly 可升级到 Children，不降级。
    scope: WatchScope,
}

pub struct InotifyManager {
    cmd_rx: RxAsync<InotifyCmd>,
    event_tx: Tx<RawEvent>,
    cancel: CancellationToken,
    config: WatchConfig,
}

impl InotifyManager {
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
                        Ok(InotifyCmd::Watch { path, scope }) => {
                            self.handle_watch(&mut watcher, &mut watches, path, scope);
                        }
                        Ok(InotifyCmd::Unwatch { path }) => {
                            self.handle_unwatch(&mut watcher, &mut watches, &path, None);
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
                            // 按 (target, scope) 分组，每个 target 只收到它匹配的路径子集
                            let mut by_target: HashMap<PathBuf, (Vec<PathBuf>, WatchScope)> = HashMap::new();

                            for p in &paths {
                                for (target, entry) in &watches {
                                    let matches = match entry.scope {
                                        WatchScope::Children => {
                                            p.parent().map_or(false, |par| par == target.as_path())
                                        }
                                        WatchScope::SelfOnly => {
                                            p.as_path() == target.as_path()
                                        }
                                    };
                                    if matches {
                                        let e = by_target
                                            .entry(target.clone())
                                            .or_insert_with(|| (Vec::new(), entry.scope));
                                        e.0.push(p.clone());
                                    }
                                }
                            }

                            for (target, (affected, scope)) in by_target {
                                let _ = self.event_tx.send(RawEvent {
                                    path: target,
                                    affected_paths: affected,
                                    scope,
                                });
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
        scope: WatchScope,
    ) {
        if let Some(existing) = watches.get_mut(&path) {
            if scope == WatchScope::Children {
                existing.scope = WatchScope::Children;
            }
            return;
        }

        match watcher.watch(&path, RecursiveMode::NonRecursive) {
            Ok(()) => {
                debug!("InotifyManager: watching {:?} (scope={:?})", path, scope);
                watches.insert(path, WatchEntry { scope });
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
        _parent: Option<&PathBuf>,
    ) {
        if watches.remove(path).is_some() {
            let _ = watcher.unwatch(path);
            debug!("InotifyManager: unwatched {:?}", path);
        }
    }
}
