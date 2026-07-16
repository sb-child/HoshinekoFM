use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use super::config::WatchConfig;
use crate::channel::{self, RxAsync, Tx};
use crate::fsworker::protocol::{AppCallbackServiceClient, WatchDelta};

// ---------------------------------------------------------------------------
// 控制命令
// ---------------------------------------------------------------------------

pub enum RouterCmd {
    Register {
        watch_id: u64,
        path: PathBuf,
        cb: AppCallbackServiceClient,
    },
    Unregister {
        watch_id: u64,
        path: PathBuf,
    },
    Shutdown,
}

// ---------------------------------------------------------------------------
// DeltaRouter
// ---------------------------------------------------------------------------

/// 增量路由器。
///
/// 从 DeltaBuilder 接收 (path, WatchDelta)，按 path 查找 subscriber 映射，
/// 分发 tarpc RPC 推送。处理背压降级：连续超时后标记 dirty，恢复时推送 Reset。
pub struct DeltaRouter {
    /// 从 DeltaBuilder 接收 delta。
    delta_rx: RxAsync<(PathBuf, WatchDelta)>,
    /// 从 Registry 接收控制命令。
    cmd_rx: RxAsync<RouterCmd>,
    /// 取消令牌。
    cancel: CancellationToken,
    /// 运行时配置。
    config: WatchConfig,
    /// path → (watch_id, callback)
    path_subs: HashMap<PathBuf, Vec<(u64, AppCallbackServiceClient)>>,
    /// watch_id → path（快速 unregister 查找）
    watch_map: HashMap<u64, PathBuf>,
    /// 背压降级标记：被标记的 watch_id 跳过增量推送，恢复时发 Reset。
    overflowed: HashSet<u64>,
    /// per-watch_id 连续 push 超时计数。
    push_timeouts: HashMap<u64, usize>,
}

impl DeltaRouter {
    pub fn spawn(
        cancel: CancellationToken,
        config: &WatchConfig,
        delta_rx: RxAsync<(PathBuf, WatchDelta)>,
    ) -> (Tx<RouterCmd>, Self) {
        let (cmd_tx, cmd_rx) = channel::unbounded();
        (
            cmd_tx,
            Self {
                delta_rx,
                cmd_rx,
                cancel,
                config: config.clone(),
                path_subs: HashMap::new(),
                watch_map: HashMap::new(),
                overflowed: HashSet::new(),
                push_timeouts: HashMap::new(),
            },
        )
    }

    #[instrument(skip(self), name = "router")]
    pub async fn run(&mut self) {
        info!("DeltaRouter starting");

        loop {
            select! {
                biased;

                _ = self.cancel.cancelled() => {
                    info!("DeltaRouter shutting down");
                    return;
                }

                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Ok(RouterCmd::Register { watch_id, path, cb }) => {
                            self.path_subs.entry(path.clone())
                                .or_default()
                                .push((watch_id, cb));
                            self.watch_map.insert(watch_id, path);
                        }
                        Ok(RouterCmd::Unregister { watch_id, path }) => {
                            self.watch_map.remove(&watch_id);
                            if let Some(subs) = self.path_subs.get_mut(&path) {
                                subs.retain(|(wid, _)| *wid != watch_id);
                                if subs.is_empty() {
                                    self.path_subs.remove(&path);
                                }
                            }
                            self.overflowed.remove(&watch_id);
                            self.push_timeouts.remove(&watch_id);
                        }
                        Ok(RouterCmd::Shutdown) => {
                            info!("DeltaRouter received Shutdown");
                            self.cancel.cancel();
                        }
                        Err(_) => return,
                    }
                }

                pair = self.delta_rx.recv() => {
                    match pair {
                        Ok((path, delta)) => self.route(&path, delta).await,
                        Err(_) => return,
                    }
                }
            }
        }
    }

    #[instrument(skip(self, delta), name = "route")]
    async fn route(&mut self, path: &PathBuf, delta: WatchDelta) {
        let Some(subs) = self.path_subs.get(path) else {
            return;
        };

        let subs: Vec<_> = subs.clone();

        for (watch_id, cb) in &subs {
            if self.overflowed.contains(watch_id) {
                // 背压降级中：跳过增量，等恢复时统一发 Reset
                continue;
            }

            let result = tokio::time::timeout(
                self.config.backpressure_push_timeout,
                cb.watch_delta(tarpc::context::current(), *watch_id, delta.clone()),
            )
            .await;

            match result {
                Ok(Ok(())) => {
                    self.push_timeouts.remove(watch_id);
                }
                Ok(Err(e)) => {
                    warn!("DeltaRouter: RPC push failed for watch_id={watch_id}: {e}");
                    self.on_push_failure(*watch_id);
                }
                Err(_) => {
                    // 超时
                    self.on_push_failure(*watch_id);
                }
            }
        }
    }

    fn on_push_failure(&mut self, watch_id: u64) {
        let count = self.push_timeouts.entry(watch_id).or_insert(0);
        *count += 1;
        if *count >= self.config.backpressure_dirty_threshold {
            warn!("DeltaRouter: watch_id={watch_id} overflowed after {count} timeouts");
            self.overflowed.insert(watch_id);
        }
    }
}
