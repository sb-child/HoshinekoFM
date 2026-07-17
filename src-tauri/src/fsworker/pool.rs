//! FsWorkerPool -- Worker 进程池 + UidToken + LeaseSentinel。

use crate::lock::LockSafe;
use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU32, Ordering},
    },
};

use tokio::sync::Mutex as TokioMutex;
use tracing::{info, warn, Instrument};

use crate::channel;
use crate::channel::oneshot;
use crate::error::AppError;

use super::callback::CallbackRegistry;
use super::relay::{WorkerRelay, kill_fs_worker};
use super::{
    CONNECTING_RETRY_DELAY, DisconnectReason, WorkerRequest, WorkerRequestContent, WorkerResponse,
    WorkerStatus,
};

// --
// LeaseSentinel
// --

/// token 的引用计数哨兵。最后一个 token drop 时通知 reaper。
pub(crate) struct LeaseSentinel {
    uid: u32,
    reaper_tx: channel::Tx<u32>,
}

impl Drop for LeaseSentinel {
    fn drop(&mut self) {
        let _ = self.reaper_tx.send(self.uid);
    }
}

// --
// UidToken
// --

/// UID 访问凭证（RAII）。
///
/// 持有 `request_tx` 和 `registry`，保证操作期间 Worker 存活。
/// 所有该 uid 的 token drop 后，对应 Worker 立即销毁（无宽限期）。
#[derive(Clone)]
pub(crate) struct UidToken {
    uid: u32,
    request_tx: channel::Tx<WorkerRequest>,
    /// 共享的 CallbackRegistry，用于注册 watcher/op 回调
    pub registry: Arc<CallbackRegistry>,
    _sentinel: Arc<LeaseSentinel>,
}

impl UidToken {
    /// 目标 UID。
    pub fn uid(&self) -> u32 {
        self.uid
    }

    /// 向 WorkerRelay 发送请求并等待响应。
    ///
    /// 若 Worker 尚未连接（返回 `Connecting`），内部自动等待重试。
    pub async fn send_request(
        &self,
        content: WorkerRequestContent,
    ) -> Result<WorkerResponse, AppError> {
        loop {
            let (response_tx, response_rx) = oneshot::oneshot();
            let request = WorkerRequest {
                content: content.clone(),
                response_tx,
            };

            self.request_tx
                .send(request)
                .map_err(|_| AppError::Other("request channel closed".to_string()))?;

            match response_rx.await {
                Ok(WorkerResponse::Connecting) => {
                    tokio::time::sleep(CONNECTING_RETRY_DELAY).await;
                }
                Ok(other) => return Ok(other),
                Err(_) => return Err(AppError::Other("response dropped".to_string())),
            }
        }
    }
}

// --
// RelaySlot
// --

pub(crate) struct RelaySlot {
    /// FsService -> Relay 请求通道
    pub(crate) request_tx: channel::Tx<WorkerRequest>,
    /// 共享回调路由
    pub(crate) registry: Arc<CallbackRegistry>,
    /// 存活 token 的弱引用；`strong_count() == 0` 表示无 token
    pub(crate) sentinel: Weak<LeaseSentinel>,
    /// 子进程 PID（由 WorkerRelay 更新）
    pub(crate) pid: Arc<AtomicU32>,
    /// 中继器实例（含 CancellationToken，用于 cancel 后加速退出）
    pub(crate) relay: WorkerRelay,
    /// Relay loop 的 abort handle（用 relay.cancel.cancel() 先取消再 abort）
    pub(crate) _abort_handle: tokio::task::AbortHandle,
}

// --
// FsWorkerPool
// --

/// Worker 进程池，按目标 UID 索引。
pub(crate) struct FsWorkerPool {
    slots: Arc<Mutex<HashMap<u32, RelaySlot>>>,
    status_tx: channel::Tx<WorkerStatus>,
    status_rx: Arc<TokioMutex<channel::RxAsync<WorkerStatus>>>,
    reaper_tx: channel::Tx<u32>,
}

impl FsWorkerPool {
    /// 创建进程池（必须在 tokio 运行时内调用）。
    pub fn new() -> Self {
        let slots: Arc<Mutex<HashMap<u32, RelaySlot>>> = Arc::new(Mutex::new(HashMap::new()));
        let (status_tx, status_rx) = channel::unbounded();
        let reaper_tx = start_reaper(slots.clone(), status_tx.clone());

        Self {
            slots,
            status_tx,
            status_rx: Arc::new(TokioMutex::new(status_rx)),
            reaper_tx,
        }
    }

    /// 请求某 uid 的访问凭证。
    ///
    /// 找到/创建对应 WorkerRelay，返回 [`UidToken`]。创建失败时返回错误。
    pub async fn request_token(&self, uid: u32) -> io::Result<UidToken> {
        // 快路径：已有 slot
        {
            let g = self.slots.lock_safe();
            if let Some(slot) = g.get(&uid) {
                if let Some(sentinel) = slot.sentinel.upgrade() {
                    return Ok(UidToken {
                        uid,
                        request_tx: slot.request_tx.clone(),
                        registry: slot.registry.clone(),
                        _sentinel: sentinel,
                    });
                }
            }
        }

        // 慢路径：创建新 WorkerRelay（不持锁 await）
        let registry = Arc::new(CallbackRegistry::new());
        let (request_tx, request_rx) = channel::unbounded::<WorkerRequest>();
        let pid = Arc::new(AtomicU32::new(0));

        let sentinel = Arc::new(LeaseSentinel {
            uid,
            reaper_tx: self.reaper_tx.clone(),
        });

        let (relay, abort_handle) = WorkerRelay::spawn(
            uid,
            pid.clone(),
            registry.clone(),
            request_rx,
            self.status_tx.clone(),
            sentinel.clone(),
        );

        let slot = RelaySlot {
            request_tx: request_tx.clone(),
            registry: registry.clone(),
            sentinel: Arc::downgrade(&sentinel),
            pid,
            relay,
            _abort_handle: abort_handle,
        };

        // 并发双 spawn 检查
        {
            let mut g = self.slots.lock_safe();
            if let Some(existing) = g.get(&uid) {
                if let Some(existing_sentinel) = existing.sentinel.upgrade() {
                    warn!("concurrent request_token for uid {uid}, reusing existing slot");
                    return Ok(UidToken {
                        uid,
                        request_tx: existing.request_tx.clone(),
                        registry: existing.registry.clone(),
                        _sentinel: existing_sentinel,
                    });
                }
            }
            g.insert(uid, slot);
        }

        Ok(UidToken {
            uid,
            request_tx,
            registry,
            _sentinel: sentinel,
        })
    }

    /// 获取状态接收端（供上层监听 Worker 连接状态）。
    pub fn status_receiver(&self) -> Arc<TokioMutex<channel::RxAsync<WorkerStatus>>> {
        self.status_rx.clone()
    }
}

impl Default for FsWorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

// --
// Reaper -- token 监控
// --

/// 启动 reaper 后台任务。
///
/// 收到 uid（某 token drop）-> 若该 uid 已无存活 token，立即清理 slot。
fn start_reaper(
    slots: Arc<Mutex<HashMap<u32, RelaySlot>>>,
    status_tx: channel::Tx<WorkerStatus>,
) -> channel::Tx<u32> {
    let (tx, rx) = channel::unbounded::<u32>();
    tokio::spawn(async move {
        while let Ok(uid) = rx.recv().await {
            let to_remove = {
                let g = slots.lock_safe();
                match g.get(&uid) {
                    Some(slot) if slot.sentinel.strong_count() == 0 => {
                        let child_pid = slot.pid.load(Ordering::Relaxed);
                        let relay_cancel = slot.relay.cancel.clone();
                        Some((child_pid, relay_cancel))
                    }
                    _ => None,
                }
            };

            if let Some((child_pid, relay_cancel)) = to_remove {
                relay_cancel.cancel();
                // 先 kill 子进程（WorkerRelay loop 会自行退出）
                if child_pid > 0 {
                    kill_fs_worker(uid, child_pid);
                }
                slots.lock_safe().remove(&uid);
                let _ = status_tx.send(WorkerStatus::Disconnected {
                    uid,
                    reason: DisconnectReason::Other {
                        message: "all tokens dropped".into(),
                    },
                    reconnecting: false,
                });
                info!("reaped worker slot for uid {uid}");
            }
        }
    }.instrument(tracing::info_span!("pool::reaper_loop")));
    tx
}
