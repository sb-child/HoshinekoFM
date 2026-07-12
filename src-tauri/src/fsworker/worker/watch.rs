//! Watch 池 + 尽力而为级联恢复。
//!
//! ## 设计
//!
//! - [`WatchPool`]：按 canonical path 去重，同一路径多个 watch_id 共享一个 [`WatchShared`]。
//! - 尽力而为：从 target 开始逐级上溯挂 inotify，每层汇报状态，
//!   直到 `/` 都失败则标记 Dead 并推送 `FatalError`。
//! - 不 list 中间层级目录，只在 target 可访问后发 `Reset`。
//! - `WatchShared` 只有一个后台任务（run），统一管理三个状态（Recovering / Live / Dead）。

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use notify::{RecursiveMode, Watcher};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};

use crate::ipc::protocol::{AppCallbackServiceClient, WatchDelta};

use super::files::{build_file, list_dir_files, resolve_path};

// ---------------------------------------------------------------------------
// WatchPool
// ---------------------------------------------------------------------------

/// 全局 Watch 池。canonical path → 共享条目，同路径复用。
pub struct WatchPool {
    entries: Mutex<HashMap<PathBuf, Arc<WatchShared>>>,
}

impl WatchPool {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
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
        let canonical = resolve_path(&path);
        let mut entries = self.entries.lock().await;

        if let Some(existing) = entries.get(&canonical) {
            existing.add_subscriber(watch_id, cb.clone()).await;
            // 新 subscriber 立即收到首帧数据
            existing.push_reset_to(watch_id, &cb).await;
            return existing.clone();
        }

        let shared = Arc::new(WatchShared {
            target: canonical.clone(),
            is_dir,
            subscribers: Mutex::new(HashMap::from([(watch_id, cb)])),
        });

        entries.insert(canonical, shared.clone());
        drop(entries);

        let s = shared.clone();
        tokio::spawn(async move { s.run().await });

        shared
    }

    /// 移除一个 watcher 的订阅。若 subscriber 数为 0 则删除 WatchShared。
    pub async fn unregister(&self, watch_id: u64, canonical: &PathBuf) {
        let removed = {
            let entries = self.entries.lock().await;
            if let Some(shared) = entries.get(canonical) {
                let mut subs = shared.subscribers.lock().await;
                subs.remove(&watch_id);
                subs.is_empty()
            } else {
                false
            }
        };
        if removed {
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
}

impl WatchShared {
    async fn add_subscriber(&self, watch_id: u64, cb: AppCallbackServiceClient) {
        self.subscribers.lock().await.insert(watch_id, cb);
    }

    /// 对单个 watch_id 推送 Reset（用于新 subscriber 加入已有 WatchShared）。
    async fn push_reset_to(&self, watch_id: u64, cb: &AppCallbackServiceClient) {
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
        let _ = cb.watch_delta(tarpc::context::current(), watch_id, delta).await;
    }

    async fn has_subscribers(&self) -> bool {
        !self.subscribers.lock().await.is_empty()
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
    /// 与 push_all 不同，此方法不做 extra clone——调用者确保 delta 不共享引用。
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

    // -----------------------------------------------------------------------
    // 主循环
    // -----------------------------------------------------------------------

    /// 尽力而为的主循环：
    /// - 从 target (level=0) 开始逐级上溯
    /// - 每层：挂 inotify → 尝试访问 target → 成功则进入 Live 事件泵
    /// - 失败则汇报状态 → 继续上一级 → 直到 `/` 都失败则 Dead
    async fn run(&self) {
        const MAX_CASCADE: usize = 64;

        for level in 0..MAX_CASCADE {
            if !self.has_subscribers().await {
                return;
            }

            let ancestor = self.ancestor_at(level);

            // 1. 挂 inotify
            let (watcher, mut evt_rx) = match self.setup_inotify(&ancestor) {
                Ok(p) => p,
                Err(e) => {
                    if ancestor.as_os_str() == "/" {
                        self.push_all(&WatchDelta::FatalError {
                            path: self.target.clone(),
                            reason: format!("cannot watch /: {e}"),
                        })
                        .await;
                        return;
                    }
                    warn!("inotify failed at {ancestor:?} level={level}: {e}");
                    continue;
                }
            };

            // 2. 尝试访问 target
            match self.try_access() {
                AccessResult::Ok => {
                    self.push_reset().await;
                    // 排空 inotify 初始 burst（Reset 已涵盖全量）
                    while let Ok(_) = evt_rx.try_recv() {}
                    self.run_event_pump(watcher, evt_rx).await;
                    return;
                }
                AccessResult::PermissionDenied => {
                    self.push_all(&WatchDelta::Inaccessible {
                        path: self.target.clone(),
                        ancestor: ancestor.clone(),
                        level: level as u32,
                        reason: format!("permission denied: {}", ancestor.display()),
                    })
                    .await;
                    drop(watcher);
                    continue;
                }
                AccessResult::Other(_) => {
                    // 无法访问（非权限原因），在当前级等待恢复
                    debug!("watch: target not accessible from {ancestor:?} level={level}, entering recovery");
                    self.recovery_loop(level, &ancestor, watcher, evt_rx)
                        .await;
                    return;
                }
            }
        }

        self.push_all(&WatchDelta::FatalError {
            path: self.target.clone(),
            reason: "exceeded max cascade levels".into(),
        })
        .await;
    }

    // -----------------------------------------------------------------------
    // 恢复循环：等待 target 可访问，进入 Live
    // -----------------------------------------------------------------------

    async fn recovery_loop(
        &self,
        level: usize,
        ancestor: &PathBuf,
        watcher: notify::RecommendedWatcher,
        mut evt_rx: mpsc::UnboundedReceiver<Vec<PathBuf>>,
    ) {
        let ancestor = ancestor.clone();
        let report_interval = Duration::from_secs(10);
        let mut last_report = tokio::time::Instant::now();

        self.push_all(&WatchDelta::Recovering {
            path: self.target.clone(),
            ancestor: ancestor.clone(),
            level: level as u32,
        })
        .await;

        loop {
            if !self.has_subscribers().await {
                return;
            }

            // 等待 inotify 事件或超时
            match tokio::time::timeout(report_interval, evt_rx.recv()).await {
                Err(_) => {
                    // 超时：推送 Recovering 心跳
                    if last_report.elapsed() >= report_interval {
                        self.push_all(&WatchDelta::Recovering {
                            path: self.target.clone(),
                            ancestor: ancestor.clone(),
                            level: level as u32,
                        })
                        .await;
                        last_report = tokio::time::Instant::now();
                    }
                }
                Ok(None) => {
                    // 通道关闭 → 掉出此循环，由外层 run 从当前 level 重入
                    return;
                }
                Ok(Some(_paths)) => {
                    // inotify 事件 → 检查 target
                    match self.try_access() {
                        AccessResult::Ok => {
                            self.push_reset().await;
                            // watcher 已在祖先目录上，继续使用
                            drop(evt_rx);
                            // 重新在 target 上挂 watcher 并进入事件泵
                            // 注意：当前的 watcher 在 ancestor 上，需要重建
                            drop(watcher);
                            self.try_live_pump().await;
                            return;
                        }
                        AccessResult::PermissionDenied => {
                            self.push_all(&WatchDelta::Inaccessible {
                                path: self.target.clone(),
                                ancestor: ancestor.clone(),
                                level: level as u32,
                                reason: "permission revoked during recovery".into(),
                            })
                            .await;
                            return;
                            // 由外层 run 从 level+1 重入
                        }
                        AccessResult::Other(_) => {
                            // 继续等待
                            if last_report.elapsed() >= report_interval {
                                self.push_all(&WatchDelta::Recovering {
                                    path: self.target.clone(),
                                    ancestor: ancestor.clone(),
                                    level: level as u32,
                                })
                                .await;
                                last_report = tokio::time::Instant::now();
                            }
                        }
                    }
                }
            }
        }
    }

    /// 尝试在 target 上建立 watcher 并进入 Live 事件泵。
    async fn try_live_pump(&self) {
        match self.setup_inotify(&self.target) {
            Ok((watcher, mut evt_rx)) => {
                while let Ok(_) = evt_rx.try_recv() {}
                self.run_event_pump(watcher, evt_rx).await;
            }
            Err(e) => {
                warn!("recovered target inotify failed: {e}");
                // fallback: 重新跑 run() 从 level=0 开始级联
            }
        }
    }

    // -----------------------------------------------------------------------
    // Live 事件泵
    // -----------------------------------------------------------------------

    async fn run_event_pump(
        &self,
        _watcher: notify::RecommendedWatcher,
        mut evt_rx: mpsc::UnboundedReceiver<Vec<PathBuf>>,
    ) {
        debug!("event_pump started for {:?}", self.target);

        while let Some(paths) = evt_rx.recv().await {
            if !self.has_subscribers().await {
                return;
            }

            if self.is_dir {
                // 对每个变更路径：exists → Upsert，否则 Remove
                let deltas: Vec<WatchDelta> = tokio::task::spawn_blocking(move || {
                    paths
                        .into_iter()
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
        }
    }

    // -----------------------------------------------------------------------
    // 工具
    // -----------------------------------------------------------------------

    fn setup_inotify(
        &self,
        path: &PathBuf,
    ) -> Result<(notify::RecommendedWatcher, mpsc::UnboundedReceiver<Vec<PathBuf>>), String> {
        let (tx, rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();
        let p = path.clone();

        let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
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

    fn try_access(&self) -> AccessResult {
        if self.is_dir {
            match std::fs::read_dir(&self.target) {
                Ok(_) => AccessResult::Ok,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    AccessResult::PermissionDenied
                }
                Err(e) => AccessResult::Other(e.to_string()),
            }
        } else {
            match std::fs::metadata(&self.target) {
                Ok(_) => AccessResult::Ok,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    AccessResult::PermissionDenied
                }
                Err(e) => AccessResult::Other(e.to_string()),
            }
        }
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
// 小类型
// ---------------------------------------------------------------------------

enum AccessResult {
    Ok,
    PermissionDenied,
    #[allow(dead_code)]
    Other(String),
}
