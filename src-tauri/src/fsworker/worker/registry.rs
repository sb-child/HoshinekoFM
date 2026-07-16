use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, instrument};

use crate::channel::Tx;
use crate::fsworker::protocol::AppCallbackServiceClient;

use super::builder::BuilderCmd;
use super::inotify::InotifyCmd;
use super::router::RouterCmd;
use super::scheduler::SchedulerCmd;

// ---------------------------------------------------------------------------
// WatchRegistry
// ---------------------------------------------------------------------------

/// Watch 订阅注册中心。
///
/// 维护 path → subscriber 映射。subscriber 数从 0→1 时启动 watch，
/// 从 1→0 时停止 watch。所有写操作由 FsWorkerServer tarpc 线程独占调用，
/// 因此内部 Mutex 仅用于读取碰撞保护，实际几乎无锁竞争。
pub struct WatchRegistry {
    /// path → subscriber 列表。
    paths: Mutex<HashMap<PathBuf, Vec<Subscriber>>>,
    /// InotifyManager 命令通道。
    inotify_tx: Tx<InotifyCmd>,
    /// WatchScheduler 命令通道。
    scheduler_tx: Tx<SchedulerCmd>,
    /// DeltaBuilder 命令通道。
    builder_tx: Tx<BuilderCmd>,
    /// DeltaRouter 命令通道。
    router_tx: Tx<RouterCmd>,
}

struct Subscriber {
    watch_id: u64,
    cb: AppCallbackServiceClient,
}

impl WatchRegistry {
    pub fn new(
        inotify_tx: Tx<InotifyCmd>,
        scheduler_tx: Tx<SchedulerCmd>,
        builder_tx: Tx<BuilderCmd>,
        router_tx: Tx<RouterCmd>,
    ) -> Self {
        Self {
            paths: Mutex::new(HashMap::new()),
            inotify_tx,
            scheduler_tx,
            builder_tx,
            router_tx,
        }
    }

    /// 订阅路径。若为首个 subscriber 则通知 InotifyManager 启动 watch
    /// 并请求 Reset。需要传入 `is_dir` 以正确配置 inotify。
    #[instrument(skip(self, cb), name = "subscribe")]
    pub async fn subscribe(
        &self,
        watch_id: u64,
        path: PathBuf,
        is_dir: bool,
        cb: AppCallbackServiceClient,
    ) {
        debug!("WatchRegistry subscribe: w={watch_id} path={path:?} is_dir={is_dir}");

        let was_empty = {
            let mut paths = self.paths.lock().await;
            let entry = paths.entry(path.clone()).or_default();
            let was_empty = entry.is_empty();
            entry.push(Subscriber {
                watch_id,
                cb: cb.clone(),
            });
            was_empty
        };

        let _ = self.router_tx.send(RouterCmd::Register {
            watch_id,
            path: path.clone(),
            cb,
        });

        // handle_watch 对已有条目安全幂等：仅更新 is_dir，不重复注册 inotify
        let _ = self.inotify_tx.send(InotifyCmd::Watch {
            path: path.clone(),
            is_dir,
        });

        if was_empty {
            let _ = self
                .scheduler_tx
                .send(SchedulerCmd::Track { path: path.clone() });
        }

        let _ = self
            .builder_tx
            .send(BuilderCmd::Reset { path: path.clone() });
    }

    /// 解除订阅。若为最后一个 subscriber 则通知 InotifyManager 停止 watch。
    #[instrument(skip(self), name = "unsubscribe")]
    pub async fn unsubscribe(&self, watch_id: u64, path: &PathBuf) {
        debug!("WatchRegistry unsubscribe: w={watch_id} path={path:?}");

        let _ = self.router_tx.send(RouterCmd::Unregister {
            watch_id,
            path: path.clone(),
        });

        let became_empty = {
            let mut paths = self.paths.lock().await;
            if let Some(subs) = paths.get_mut(path) {
                subs.retain(|s| s.watch_id != watch_id);
                subs.is_empty()
            } else {
                false
            }
        };

        if became_empty {
            self.paths.lock().await.remove(path);
            let _ = self
                .inotify_tx
                .send(InotifyCmd::Unwatch { path: path.clone() });
            let _ = self
                .scheduler_tx
                .send(SchedulerCmd::Untrack { path: path.clone() });
            let _ = self
                .builder_tx
                .send(BuilderCmd::CancelPath { path: path.clone() });
        }
    }

    /// 请求对指定 watch_id 的目录做全量 Refresh。
    #[instrument(skip(self), name = "request_reset")]
    pub async fn request_reset(&self, watch_id: u64, path: &PathBuf) {
        debug!("WatchRegistry request_reset: w={watch_id} path={path:?}");
        let _ = self
            .scheduler_tx
            .send(SchedulerCmd::RequestReset { path: path.clone() });
        let _ = self
            .builder_tx
            .send(BuilderCmd::Reset { path: path.clone() });
    }
}
