//! 反向回调路由（CallbackRegistry + CallbackServer）。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tarpc::context;
use tokio::net::UnixStream;
use tracing::info;

use crate::channel;
use crate::channel::oneshot;
use crate::ipc::protocol::{
    AppCallbackService, ConflictItem, ConflictResolution, ProgressEvent, WatchDelta,
};

use super::DisconnectReason;

/// 把 Worker 反向回调路由到对应 `Watcher` / `Progress` 的通道。
///
/// `watch_id` / `op_id` 全局分配（由 FsService）。
pub struct CallbackRegistry {
    /// watch_id → watcher 增量 sender
    watches: Mutex<HashMap<u64, channel::Tx<WatchDelta>>>,
    /// op_id → 进度 sender
    ops: Mutex<HashMap<u64, channel::Tx<ProgressEvent>>>,
    /// (op_id, conflict_id) → 冲突决策应答
    conflicts: Mutex<HashMap<(u64, u64), oneshot::TxOneshot<ConflictResolution>>>,
}

impl CallbackRegistry {
    pub(crate) fn new() -> Self {
        Self {
            watches: Mutex::new(HashMap::new()),
            ops: Mutex::new(HashMap::new()),
            conflicts: Mutex::new(HashMap::new()),
        }
    }

    /// 注册一个 watcher，返回增量接收端。
    pub fn register_watch(&self, watch_id: u64) -> channel::RxAsync<WatchDelta> {
        let (tx, rx) = channel::unbounded();
        self.watches.lock().unwrap().insert(watch_id, tx);
        rx
    }

    /// 注销 watcher。
    pub fn unregister_watch(&self, watch_id: u64) {
        self.watches.lock().unwrap().remove(&watch_id);
    }

    /// 注册一个批处理操作，返回进度接收端。
    pub fn register_op(&self, op_id: u64) -> channel::RxAsync<ProgressEvent> {
        let (tx, rx) = channel::unbounded();
        self.ops.lock().unwrap().insert(op_id, tx);
        rx
    }

    /// 注销操作及其残留冲突。
    pub fn unregister_op(&self, op_id: u64) {
        self.ops.lock().unwrap().remove(&op_id);
        self.conflicts
            .lock()
            .unwrap()
            .retain(|(oid, _), _| *oid != op_id);
    }

    /// 上层给出冲突决策，唤醒等待中的 `ask_conflict`。
    pub fn resolve_conflict(&self, op_id: u64, conflict_id: u64, res: ConflictResolution) {
        if let Some(tx) = self.conflicts.lock().unwrap().remove(&(op_id, conflict_id)) {
            let _ = tx.send(res);
        }
    }

    /// 推送 ConnectionLost 到所有注册的 watcher 和 op。
    pub fn notify_connection_lost(&self, reason: DisconnectReason, reconnecting: bool) {
        let reason_str = format!("{:?}", reason);
        let watch_count: usize;
        let op_count: usize;
        {
            let watches = self.watches.lock().unwrap();
            watch_count = watches.len();
            for (watch_id, tx) in watches.iter() {
                let _ = tx.send(WatchDelta::ConnectionLost {
                    watch_id: *watch_id,
                    reason: reason_str.clone(),
                    reconnecting,
                });
            }
        }
        {
            let ops = self.ops.lock().unwrap();
            op_count = ops.len();
            for (op_id, tx) in ops.iter() {
                let _ = tx.send(ProgressEvent::ConnectionLost {
                    op_id: *op_id,
                    reason: reason_str.clone(),
                    reconnecting,
                });
            }
        }
        info!(
            "notify_connection_lost: reason={reason:?} reconnecting={reconnecting} watches={watch_count} ops={op_count}"
        );
    }

    // --- 由回调 server 调用 ---

    pub(crate) fn push_watch_delta(&self, watch_id: u64, delta: WatchDelta) {
        if let Some(tx) = self.watches.lock().unwrap().get(&watch_id) {
            let _ = tx.send(delta);
        }
    }

    pub(crate) fn push_progress(&self, op_id: u64, ev: ProgressEvent) {
        if let Some(tx) = self.ops.lock().unwrap().get(&op_id) {
            let _ = tx.send(ev);
        }
    }

    pub(crate) async fn ask_conflict(
        &self,
        op_id: u64,
        conflict_id: u64,
        item: ConflictItem,
    ) -> ConflictResolution {
        self.push_progress(op_id, ProgressEvent::Conflict { conflict_id, item });
        let (tx, rx) = oneshot::oneshot();
        self.conflicts
            .lock()
            .unwrap()
            .insert((op_id, conflict_id), tx);
        rx.await.unwrap_or(ConflictResolution::CancelAll)
    }
}

// ---------------------------------------------------------------------------
// 反向回调 server（app 侧实现 AppCallbackService）
// ---------------------------------------------------------------------------

/// app 侧的 [`AppCallbackService`] 实现，把回调路由进 [`CallbackRegistry`]。
#[derive(Clone)]
struct CallbackServer {
    registry: Arc<CallbackRegistry>,
}

impl AppCallbackService for CallbackServer {
    async fn watch_delta(self, _ctx: context::Context, watch_id: u64, delta: WatchDelta) {
        self.registry.push_watch_delta(watch_id, delta);
    }

    async fn progress(self, _ctx: context::Context, op_id: u64, ev: ProgressEvent) {
        self.registry.push_progress(op_id, ev);
    }

    async fn ask_conflict(
        self,
        _ctx: context::Context,
        op_id: u64,
        conflict_id: u64,
        item: ConflictItem,
    ) -> ConflictResolution {
        self.registry.ask_conflict(op_id, conflict_id, item).await
    }
}

/// 在给定流上启动 AppCallbackService server。
pub(crate) fn serve_callback(stream: UnixStream, registry: Arc<CallbackRegistry>) {
    use futures::prelude::*;
    use tarpc::server::Channel;

    let transport = tarpc::serde_transport::new(
        crate::ipc::frame_stream(stream),
        tarpc::tokio_serde::formats::Bincode::default(),
    );
    let server = CallbackServer { registry };

    async fn spawn(fut: impl futures::Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }

    tokio::spawn(
        tarpc::server::BaseChannel::with_defaults(transport)
            .execute(server.serve())
            .for_each(spawn),
    );
}
