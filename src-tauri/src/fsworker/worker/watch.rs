//! Watch 池 + 尽力而为级联恢复。
//!
//! ## 设计
//!
//! - [`WatchPool`]：按 normalized path 去重（不跟随符号链接），
//!   同一路径多个 watch_id 共享一个 [`WatchShared`]。
//! - 尽力而为：从 target 开始逐级上溯挂 inotify，每层汇报状态，
//!   直到 `/` 都失败则标记 Dead 并推送 `FatalError`。
//! - 不 list 中间层级目录，只在 target 可访问后发 `Reset`。
//! - `WatchShared` 只有一个后台任务（run），统一管理三个分支：
//!   虚拟文件 poll 模式 / 实体文件 inotify 级联恢复 / Live 事件泵。
//! - 零 timeout、零 sleep：退出通过 `tokio::sync::Notify` 驱动，
//!   事件泵纯 select! 阻塞。

use std::{collections::HashMap, io, os::unix::fs::OpenOptionsExt, path::PathBuf, sync::Arc};

use notify::{RecursiveMode, Watcher};
use tokio::{
    io::unix::AsyncFd,
    select,
    sync::{Mutex, Notify},
};
use tracing::{debug, warn};

use crate::channel;
use crate::ipc::protocol::{AppCallbackServiceClient, WatchDelta};

use super::files::{build_file, list_dir_files, normalize_path_no_symlink};

// ---------------------------------------------------------------------------
// WatchPool
// ---------------------------------------------------------------------------

/// 全局 Watch 池。normalized path → 共享条目，同路径复用。
pub struct WatchPool {
    entries: Mutex<HashMap<PathBuf, Arc<WatchShared>>>,
}

impl WatchPool {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(super) async fn get_shared(&self, canonical: &PathBuf) -> Option<Arc<WatchShared>> {
        self.entries.lock().await.get(canonical).cloned()
    }

    /// 注册一个 watcher。返回已有或新建的 WatchShared。
    /// 若为新条目则 spawn 恢复/事件循环。
    pub async fn register(
        &self,
        watch_id: u64,
        path: PathBuf,
        is_dir: bool,
        cb: AppCallbackServiceClient,
    ) -> Arc<WatchShared> {
        let normalized = normalize_path_no_symlink(&path);
        let mut entries = self.entries.lock().await;

        if let Some(existing) = entries.get(&normalized) {
            existing.add_subscriber(watch_id, cb.clone()).await;
            existing.push_reset_to(watch_id, &cb).await;
            return existing.clone();
        }

        let shared = Arc::new(WatchShared {
            target: normalized.clone(),
            is_dir,
            subscribers: Mutex::new(HashMap::from([(watch_id, cb)])),
            exit_notify: Notify::new(),
        });

        entries.insert(normalized, shared.clone());
        drop(entries);

        let s = shared.clone();
        tokio::spawn(async move { s.run().await });

        shared
    }

    /// 移除一个 watcher 的订阅。若 subscriber 数为 0 则删除 WatchShared
    /// 并通知后台任务退出。
    pub async fn unregister(&self, watch_id: u64, canonical: &PathBuf) {
        let (_removed, shared_to_notify) = {
            let entries = self.entries.lock().await;
            if let Some(shared) = entries.get(canonical) {
                let mut subs = shared.subscribers.lock().await;
                subs.remove(&watch_id);
                let empty = subs.is_empty();
                (empty, if empty { Some(shared.clone()) } else { None })
            } else {
                (false, None)
            }
        };
        if let Some(shared) = shared_to_notify {
            shared.exit_notify.notify_waiters();
            self.entries.lock().await.remove(canonical);
        }
    }
}

// ---------------------------------------------------------------------------
// WatchShared
// ---------------------------------------------------------------------------

/// 共享的 watch 条目。由 [`WatchPool`] 管理，拥有唯一后台任务。
pub struct WatchShared {
    pub target: PathBuf,
    is_dir: bool,
    /// watch_id → 回调 client
    subscribers: Mutex<HashMap<u64, AppCallbackServiceClient>>,
    /// 订阅归零时唤醒，通知后台任务退出
    exit_notify: Notify,
}

impl WatchShared {
    async fn add_subscriber(&self, watch_id: u64, cb: AppCallbackServiceClient) {
        self.subscribers.lock().await.insert(watch_id, cb);
    }

    /// 对单个 watch_id 推送 Reset（用于新 subscriber 加入已有 WatchShared）。
    ///
    /// 先检查 target 可访问性：不可达时推送 Inaccessible 而非空 Reset，
    /// 避免前端 Inaccessible UI 被清空。
    pub(super) async fn push_reset_to(&self, watch_id: u64, cb: &AppCallbackServiceClient) {
        let ctx = tarpc::context::current();

        match self.try_access().await {
            AccessResult::Ok => {}
            AccessResult::PermissionDenied => {
                let _ = cb
                    .watch_delta(
                        ctx,
                        watch_id,
                        WatchDelta::Inaccessible {
                            path: self.target.clone(),
                            ancestor: self.target.clone(),
                            level: 0,
                            reason: "permission denied".into(),
                        },
                    )
                    .await;
                return;
            }
            AccessResult::Other(reason) => {
                let _ = cb
                    .watch_delta(
                        ctx,
                        watch_id,
                        WatchDelta::Inaccessible {
                            path: self.target.clone(),
                            ancestor: self.target.clone(),
                            level: 0,
                            reason,
                        },
                    )
                    .await;
                return;
            }
        }

        let delta = if self.is_dir {
            let dir = self.target.clone();
            let files = tokio::task::spawn_blocking(move || list_dir_files(&dir))
                .await
                .unwrap_or_default();
            WatchDelta::Reset(files)
        } else {
            let p = self.target.clone();
            let file = tokio::task::spawn_blocking(move || build_file(&p))
                .await
                .ok()
                .flatten();
            let files = file.map(|f| vec![f]).unwrap_or_default();
            WatchDelta::Reset(files)
        };
        let _ = cb.watch_delta(ctx, watch_id, delta).await;
    }

    /// 向每个 subscriber 推送 delta（带上各自的 watch_id）。
    /// **栈内 await**，不 spawn，避免突发并发风暴。
    async fn push_all(&self, delta: &WatchDelta) {
        let subs: Vec<_> = self
            .subscribers
            .lock()
            .await
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        for (wid, cb) in subs {
            let _ = cb
                .watch_delta(tarpc::context::current(), wid, delta.clone())
                .await;
        }
    }

    /// 事件泵专用：对多个 subscriber 推送单个增量条目。
    async fn push_delta(&self, delta: WatchDelta) {
        let subs: Vec<_> = self
            .subscribers
            .lock()
            .await
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        for (wid, cb) in subs {
            let _ = cb
                .watch_delta(tarpc::context::current(), wid, delta.clone())
                .await;
        }
    }

    /// 向所有 subscriber 推送 target 的 Reset。
    async fn push_reset(&self) {
        if self.is_dir {
            let dir = self.target.clone();
            let files = tokio::task::spawn_blocking(move || list_dir_files(&dir))
                .await
                .unwrap_or_default();
            self.push_all(&WatchDelta::Reset(files)).await;
        } else {
            let p = self.target.clone();
            let file = tokio::task::spawn_blocking(move || build_file(&p))
                .await
                .ok()
                .flatten();
            let files = file.map(|f| vec![f]).unwrap_or_default();
            self.push_all(&WatchDelta::Reset(files)).await;
        }
    }

    /// 给 exit_notify.notified() 套一层可借号短命的 future。
    /// select! 要求所有分支的 future 都需 `&mut` 借用并在此期间存活，
    /// 因此不能直接写 `self.exit_notify.notified()` —— 其返回的 future 会
    /// borrow &self/&Notify 太久。用此函数把 notified future 立刻取下
    /// 再 pin_mut 即可避开 borrow checker 限制。
    fn exit_signal(&self) -> impl std::future::Future<Output = ()> + '_ {
        let n = self.exit_notify.notified();
        async move {
            n.await;
        }
    }

    // -----------------------------------------------------------------------
    // 主循环
    // -----------------------------------------------------------------------

    /// 统一分发循环：虚拟文件走 poll，实体文件走 inotify 级联。
    /// run_poll / cascade 返回时重新检测文件类型并路由到正确分支，
    /// 处理暂停期间文件类型被替换的场景（普通→虚拟 或 虚拟→普通）。
    /// cascade() 返回 false 时表示 FatalError（级联全失败），永久休眠等 unregister。
    async fn run(&self) {
        loop {
            if self.subscribers.lock().await.is_empty() {
                return;
            }
            if !self.is_dir && is_virtual_fs(&self.target) {
                self.run_poll().await;
                continue;
            }
            if self.cascade().await {
                // run_event_pump / recovery_loop 正常结束，重入检查
                continue;
            }
            // FatalError: 级联全失败，永久休眠等 unregister
            self.exit_signal().await;
            return;
        }
    }

    /// 尽力而为的级联恢复。
    /// - 从 target (level=0) 开始逐级上溯
    /// - 每层：挂 inotify → 尝试访问 target → 成功则进入 Live 事件泵
    /// - 失败则汇报状态 → 继续上一级 → 直到 `/` 都失败则 Dead
    /// - 返回 true: 恢复成功或 subscriber 清空；false: FatalError（放弃）
    async fn cascade(&self) -> bool {
        const MAX_CASCADE: usize = 64;

        for level in 0..MAX_CASCADE {
            if self.subscribers.lock().await.is_empty() {
                return true;
            }

            let ancestor = self.ancestor_at(level);

            let (watcher, evt_rx) = match self.setup_inotify(&ancestor) {
                Ok(p) => p,
                Err(e) => {
                    self.push_all(&WatchDelta::Inaccessible {
                        path: self.target.clone(),
                        ancestor: ancestor.clone(),
                        level: level as u32,
                        reason: format!("cannot watch {}: {}", ancestor.display(), e),
                    })
                    .await;
                    if ancestor.as_os_str() == "/" {
                        self.push_all(&WatchDelta::FatalError {
                            path: self.target.clone(),
                            reason: format!("cannot watch /: {e}"),
                        })
                        .await;
                        return false;
                    }
                    continue;
                }
            };

            match self.try_access().await {
                AccessResult::Ok => {
                    self.push_reset().await;
                    while let Ok(_) = evt_rx.try_recv() {}
                    self.run_event_pump(watcher, evt_rx).await;
                    return true;
                }
                AccessResult::PermissionDenied => {
                    self.push_all(&WatchDelta::Inaccessible {
                        path: self.target.clone(),
                        ancestor: ancestor.clone(),
                        level: level as u32,
                        reason: format!("permission denied: {}", self.target.display()),
                    })
                    .await;
                    self.recovery_loop(level, &ancestor, watcher, evt_rx).await;
                    return true;
                }
                AccessResult::Other(_) => {
                    debug!(
                        "watch: target not accessible from {ancestor:?} level={level}, entering recovery"
                    );
                    self.recovery_loop(level, &ancestor, watcher, evt_rx).await;
                    return true;
                }
            }
        }

        self.push_all(&WatchDelta::FatalError {
            path: self.target.clone(),
            reason: "exceeded max cascade levels".into(),
        })
        .await;
        false
    }

    // -----------------------------------------------------------------------
    // 虚拟文件 poll 事件循环
    // -----------------------------------------------------------------------

    /// 用 `poll(2)` / `AsyncFd` 监听虚拟文件系统（如 /proc/mounts）。
    /// inotify 对这类文件永不触发，必须通过 kernel poll (POLLPRI | POLLERR)
    /// 来感知变化。零 timeout、零 busy-wait。
    async fn run_poll(&self) {
        debug!("poll mode started for {:?}", self.target);
        self.push_reset().await;

        let fd = match open_nonblock(&self.target) {
            Ok(f) => f,
            Err(e) => {
                warn!(
                    "run_poll open {:?} failed: {e}, falling back to inotify",
                    self.target
                );
                return;
            }
        };

        let async_fd = match AsyncFd::with_interest(fd, tokio::io::Interest::PRIORITY) {
            Ok(af) => af,
            Err(e) => {
                warn!("run_poll AsyncFd::with_interest {:?} failed: {e}", self.target);
                return;
            }
        };

        loop {
            let exit = self.exit_signal();
            tokio::pin!(exit);

            select! {
                biased;

                _ = &mut exit => {
                    return;
                }

                result = async_fd.readable() => {
                    match result {
                        Ok(mut guard) => {
                            guard.clear_ready();
                            if !is_virtual_fs(&self.target) {
                                debug!(
                                    "poll: {:?} no longer virtual, switching to inotify",
                                    self.target
                                );
                                return;
                            }
                            // 文件内容可能变了，重置 seek 并推送当前状态
                            self.push_reset().await;
                        }
                        Err(e) => {
                            warn!("run_poll {:?} readable error: {e}", self.target);
                            return;
                        }
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 恢复循环：等待 target 可访问，进入 Live
    // -----------------------------------------------------------------------

    async fn recovery_loop(
        &self,
        level: usize,
        ancestor: &PathBuf,
        watcher: notify::RecommendedWatcher,
        evt_rx: channel::RxAsync<Vec<PathBuf>>,
    ) {
        let ancestor = ancestor.clone();

        self.push_all(&WatchDelta::Recovering {
            path: self.target.clone(),
            ancestor: ancestor.clone(),
            level: level as u32,
        })
        .await;

        loop {
            let exit = self.exit_signal();
            tokio::pin!(exit);

            let paths = select! {
                biased;

                _ = &mut exit => {
                    return;
                }

                p = evt_rx.recv() => p,
            };

            match paths {
                Err(_) => return,
                Ok(_paths) => {
                    // 文件类型可能在暂停期间变化（如普通文件被替换为虚拟文件）
                    if !self.is_dir && is_virtual_fs(&self.target) {
                        debug!(
                            "recovery: {:?} became virtual, switching to poll",
                            self.target
                        );
                        return;
                    }
            match self.try_access().await {
                        AccessResult::Ok => {
                            self.push_reset().await;
                            drop(evt_rx);
                            drop(watcher);
                            self.try_live_pump().await;
                            return;
                        }
                        _ => {
                            // 仍不可达，继续等待下一个 inotify 事件
                        }
                    }
                }
            }
        }
    }

    /// 尝试在 target 上建立 watcher 并进入 Live 事件泵。
    async fn try_live_pump(&self) {
        match self.setup_inotify(&self.target) {
            Ok((watcher, evt_rx)) => {
                while let Ok(_) = evt_rx.try_recv() {}
                self.run_event_pump(watcher, evt_rx).await;
            }
            Err(e) => {
                warn!("recovered target inotify failed: {e}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Live 事件泵
    // -----------------------------------------------------------------------

    async fn run_event_pump(
        &self,
        _watcher: notify::RecommendedWatcher,
        evt_rx: channel::RxAsync<Vec<PathBuf>>,
    ) {
        debug!("event_pump started for {:?}", self.target);

        loop {
            let exit = self.exit_signal();
            tokio::pin!(exit);

            let paths = select! {
                biased;

                _ = &mut exit => {
                    return;
                }

                p = evt_rx.recv() => p,
            };

            match paths {
                Err(_) => return,
                Ok(paths) => {
                    if self.is_dir {
                        let target = self.target.clone();
                        let deltas: Vec<WatchDelta> = tokio::task::spawn_blocking(move || {
                            paths
                                .into_iter()
                                .filter(|p| *p != target)
                                .filter_map(|p| {
                                    if p.exists() {
                                        build_file(&p).map(WatchDelta::Upsert)
                                    } else {
                                        Some(WatchDelta::Remove(p))
                                    }
                                })
                                .collect()
                        })
                        .await
                        .unwrap_or_default();
                        for d in deltas {
                            self.push_delta(d).await;
                        }
                    } else {
                        let target = self.target.clone();
                        let delta = tokio::task::spawn_blocking(move || {
                            if target.exists() {
                                build_file(&target).map(WatchDelta::Upsert)
                            } else {
                                Some(WatchDelta::Remove(target))
                            }
                        })
                        .await
                        .unwrap_or(None);
                        if let Some(d) = delta {
                            self.push_delta(d).await;
                        }
                    }

                    // 文件类型可能在暂停期间变化（如普通文件被替换为虚拟文件）
                    if !self.is_dir && is_virtual_fs(&self.target) {
                        debug!(
                            "event_pump: {:?} became virtual, switching to poll",
                            self.target
                        );
                        return;
                    }
                    // 检查 target 是否变得不可访问（如权限变化、挂载点卸载等）
                    if let AccessResult::Ok = self.try_access().await {
                    } else {
                        debug!(
                            "event_pump: {:?} became inaccessible, re-entering cascade",
                            self.target
                        );
                        return;
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 工具
    // -----------------------------------------------------------------------

    fn setup_inotify(
        &self,
        path: &PathBuf,
    ) -> Result<(notify::RecommendedWatcher, channel::RxAsync<Vec<PathBuf>>), String> {
        let (tx, rx) = channel::unbounded::<Vec<PathBuf>>();
        let p = path.clone();

        let mut watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.send(event.paths);
                }
            })
            .map_err(|e| format!("create watcher: {e}"))?;

        watcher
            .watch(&p, RecursiveMode::NonRecursive)
            .map_err(|e| format!("watch path: {e}"))?;

        Ok((watcher, rx))
    }

    async fn try_access(&self) -> AccessResult {
        let target = self.target.clone();
        let is_dir = self.is_dir;

        tokio::task::spawn_blocking(move || {
            if is_dir {
                match std::fs::read_dir(&target) {
                    Ok(_) => AccessResult::Ok,
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        AccessResult::PermissionDenied
                    }
                    Err(e) => AccessResult::Other(e.to_string()),
                }
            } else {
                match std::fs::metadata(&target) {
                    Ok(_) => AccessResult::Ok,
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        AccessResult::PermissionDenied
                    }
                    Err(e) => AccessResult::Other(e.to_string()),
                }
            }
        })
        .await
        .unwrap_or(AccessResult::Other("spawn_blocking panicked".into()))
    }

    fn ancestor_at(&self, level: usize) -> PathBuf {
        let mut path = self.target.clone();
        for _ in 0..level {
            match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => path = p.to_path_buf(),
                _ => return PathBuf::from("/"),
            }
        }
        path
    }
}

// ---------------------------------------------------------------------------
// 小类型 & 工具函数
// ---------------------------------------------------------------------------

enum AccessResult {
    Ok,
    PermissionDenied,
    #[allow(dead_code)]
    Other(String),
}

/// 检测路径是否在虚拟文件系统上（/proc、/sys）。
/// 这些文件系统不产生 inotify 事件，必须用 kernel poll (POLLPRI) 监听。
fn is_virtual_fs(path: &PathBuf) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/proc/") || s.starts_with("/sys/")
}

/// 以 O_RDONLY | O_NONBLOCK 打开文件，用于 AsyncFd 包装。
fn open_nonblock(path: &PathBuf) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}
