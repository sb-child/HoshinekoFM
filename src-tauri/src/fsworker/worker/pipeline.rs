//! Watch 流水线装配。

use std::path::PathBuf;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::config::WatchConfig;
use crate::channel::Tx;
use crate::fsworker::protocol::{AppCallbackServiceClient, WatchDelta};

use super::builder::{BuilderCmd, DeltaBuilder};
use super::inotify::{InotifyCmd, InotifyManager};
use super::registry::WatchRegistry;
use super::router::{DeltaRouter, RouterCmd};
use super::scheduler::{SchedulerCmd, WatchScheduler};

/// 流水线中所有长生命周期 actor 的 handle 集合。
pub struct PipelineHandles {
    inotify: JoinHandle<()>,
    scheduler: JoinHandle<()>,
    builder: JoinHandle<()>,
    router: JoinHandle<()>,
}

impl PipelineHandles {
    /// 按依赖顺序等待所有 actor 退出。
    pub async fn join_all(self) {
        let _ = tokio::join!(self.inotify, self.scheduler, self.builder, self.router,);
    }
}

/// 组装并启动 watch 流水线。
///
/// 返回 `(WatchRegistry, PipelineHandles)`。
/// - `WatchRegistry` 传递给 `FsWorkerServer`，用于 subscribe/unsubscribe/refresh。
/// - `PipelineHandles` 持有所有 spawn 的 handle，用于 shutdown 时等待。
pub fn assemble(
    cancel: CancellationToken,
    config: &WatchConfig,
    fs_worker_id: u64,
) -> (WatchRegistry, PipelineHandles) {
    let span = tracing::info_span!("pipeline", id = %fs_worker_id);
    let _guard = span.enter();

    // 1. InotifyManager
    let (inotify_tx, event_rx, inotify) = InotifyManager::spawn(cancel.clone(), config);
    let inotify_handle = tokio::spawn(async move { inotify.run().await });

    // 2. WatchScheduler
    let (scheduler_tx, scheduler_event_rx, scheduler) =
        WatchScheduler::spawn(cancel.clone(), config, event_rx);
    let scheduler_handle = tokio::spawn(async move {
        let mut s = scheduler;
        s.run().await;
    });

    // 3. DeltaBuilder
    let (builder_tx, delta_rx, builder) =
        DeltaBuilder::spawn(cancel.clone(), config, scheduler_event_rx);
    let builder_handle = tokio::spawn(async move {
        let mut b = builder;
        b.run().await;
    });

    // 4. DeltaRouter
    let (router_tx, mut router) = DeltaRouter::spawn(cancel.clone(), config, delta_rx);
    let router_handle = tokio::spawn(async move { router.run().await });

    // 5. WatchRegistry
    let registry = WatchRegistry::new(inotify_tx, scheduler_tx, builder_tx, router_tx);

    drop(_guard);
    info!("Pipeline assembled for fs-worker {fs_worker_id}");

    (
        registry,
        PipelineHandles {
            inotify: inotify_handle,
            scheduler: scheduler_handle,
            builder: builder_handle,
            router: router_handle,
        },
    )
}
