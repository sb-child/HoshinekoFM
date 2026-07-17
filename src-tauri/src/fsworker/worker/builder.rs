use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::select;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn, Instrument};

use super::config::WatchConfig;
use crate::channel::{self, RxAsync, Tx};
use crate::fsworker::protocol::{AppCallbackServiceClient, WatchDelta};

use super::files::build_file;
use super::scheduler::SchedulerEvent;

// --
// 输入
// --

/// Registry 或 Scheduler 发给 DeltaBuilder 的控制命令。
pub enum BuilderCmd {
    /// 重新生成全量快照并推送 Reset。
    Reset {
        path: PathBuf,
    },
    /// 取消某个路径的进行中任务。
    CancelPath {
        path: PathBuf,
    },
    Shutdown,
}

// --
// DeltaBuilder
// --

/// 文件增量构建器。
///
/// 接收 Scheduler 的聚合事件和 Registry 的控制命令，
/// 用信号量限流并发 stat，产出 WatchDelta 推送给 DeltaRouter。
pub struct DeltaBuilder {
    /// 从 Scheduler 接收聚合事件。
    event_rx: RxAsync<SchedulerEvent>,
    /// 从 Registry 接收控制命令。
    cmd_rx: RxAsync<BuilderCmd>,
    /// 向 DeltaRouter 推送 (path, WatchDelta)。
    delta_tx: Tx<(PathBuf, WatchDelta)>,
    /// 取消令牌。
    cancel: CancellationToken,
    /// 运行时配置。
    config: WatchConfig,
    /// 并发限流信号量。
    semaphore: Arc<Semaphore>,
    /// 已取消的路径集合（进行中任务的输出会被丢弃）。
    cancelled: HashMap<PathBuf, ()>,
}

impl DeltaBuilder {
    pub fn spawn(
        cancel: CancellationToken,
        config: &WatchConfig,
        event_rx: RxAsync<SchedulerEvent>,
    ) -> (Tx<BuilderCmd>, RxAsync<(PathBuf, WatchDelta)>, Self) {
        let (cmd_tx, cmd_rx) = channel::unbounded();
        let (delta_tx, delta_rx) = channel::unbounded();

        (
            cmd_tx,
            delta_rx,
            Self {
                event_rx,
                cmd_rx,
                delta_tx,
                cancel,
                config: config.clone(),
                semaphore: Arc::new(Semaphore::new(config.max_parallel_stat)),
                cancelled: HashMap::new(),
            },
        )
    }

    #[instrument(skip(self), name = "builder")]
    pub async fn run(&mut self) {
        info!("DeltaBuilder starting");

        loop {
            select! {
                biased;

                _ = self.cancel.cancelled() => {
                    info!("DeltaBuilder shutting down");
                    return;
                }

                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Ok(BuilderCmd::Reset { path }) => {
                            self.handle_reset(path).await;
                        }
                        Ok(BuilderCmd::CancelPath { path }) => {
                            self.cancelled.insert(path, ());
                        }
                        Ok(BuilderCmd::Shutdown) => {
                            info!("DeltaBuilder received Shutdown");
                            self.cancel.cancel();
                        }
                        Err(_) => return,
                    }
                }

                event = self.event_rx.recv() => {
                    match event {
                        Ok(evt) => self.handle_event(evt).await,
                        Err(_) => return,
                    }
                }
            }
        }
    }

    async fn handle_event(&mut self, event: SchedulerEvent) {
        match event {
            SchedulerEvent::FilesChanged { path, affected } => {
                self.cancelled.remove(&path);
                build_upserts_impl(
                    self.semaphore.clone(),
                    self.delta_tx.clone(),
                    path,
                    affected,
                )
                .await;
            }
            SchedulerEvent::RecoveredAccess { path } => {
                self.cancelled.remove(&path);
                self.handle_reset(path).await;
            }
            SchedulerEvent::LostAccess { path, reason } => {
                let _ = self.delta_tx.send((
                    path.clone(),
                    WatchDelta::Inaccessible {
                        path: path.clone(),
                        ancestor: path.clone(),
                        level: 0,
                        reason,
                    },
                ));
            }
            SchedulerEvent::FatalError { path, reason } => {
                let _ = self
                    .delta_tx
                    .send((path.clone(), WatchDelta::FatalError { path, reason }));
            }
        }
    }

    async fn handle_reset(&mut self, path: PathBuf) {
        self.cancelled.remove(&path);
        let sem = self.semaphore.clone();
        let batch_size = self.config.reset_batch_size;
        let delta_tx = self.delta_tx.clone();

        let h = tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;

            let _ = tokio::task::spawn_blocking(move || {
                let _span = tracing::info_span!("builder::handle_reset_read_dir").entered();
                let read = match std::fs::read_dir(&path) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = delta_tx.send((
                            path.clone(),
                            WatchDelta::Inaccessible {
                                path: path.clone(),
                                ancestor: path.clone(),
                                level: 0,
                                reason: e.to_string(),
                            },
                        ));
                        return;
                    }
                };

                // 先清空前端的旧文件列表（通过同一个 channel 顺序保证 Reset 在 UpsertBatch 前到达）
                let _ = delta_tx.send((path.clone(), WatchDelta::Reset(Vec::new())));

                let mut batch: Vec<crate::fsworker::protocol::File> =
                    Vec::with_capacity(batch_size);
                for entry in read.flatten() {
                    if let Some(f) = build_file(&entry.path()) {
                        batch.push(f);
                        if batch.len() >= batch_size {
                            let chunk =
                                std::mem::replace(&mut batch, Vec::with_capacity(batch_size));
                            let _ = delta_tx.send((path.clone(), WatchDelta::UpsertBatch(chunk)));
                        }
                    }
                }
                if !batch.is_empty() {
                    let _ = delta_tx.send((path.clone(), WatchDelta::UpsertBatch(batch)));
                }
            })
            .await;
        }.instrument(tracing::info_span!("builder::handle_reset_worker")));
        tokio::spawn(async move {
            if let Err(e) = h.await {
                tracing::error!("DeltaBuilder handle_reset task panicked: {e}");
            }
        }.instrument(tracing::info_span!("builder::handle_reset_monitor")));
    }
}

async fn build_upserts_impl(
    sem: Arc<Semaphore>,
    delta_tx: Tx<(PathBuf, WatchDelta)>,
    path: PathBuf,
    affected: Vec<PathBuf>,
) {
    for file_path in affected {
        let dt = delta_tx.clone();
        let fp = file_path;
        let p = path.clone();
        let sem = sem.clone();

        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let delta = tokio::task::spawn_blocking(move || {
                let _span = tracing::info_span!("builder::build_file").entered();
                if fp.exists() {
                    build_file(&fp).map(WatchDelta::Upsert)
                } else {
                    Some(WatchDelta::Remove(fp))
                }
            })
            .await
            .unwrap_or_else(|e| {
                tracing::error!("build_upserts spawn_blocking panicked: {e}");
                None
            });

            if let Some(d) = delta {
                let _ = dt.send((p, d));
            }
        }.instrument(tracing::info_span!("builder::build_upsert")));
    }
}
