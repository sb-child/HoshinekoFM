//! 导航历史 + tab 管理 + watcher 生命周期。
//!
//! ## Watcher 架构
//!
//! 每个窗口一个持久 watch 线程（`start_watch`），内部维护所有 tab 的 watcher 任务 +
//! 文件列表快照。tab 切换时直接推送缓存快照（零 RPC / 零 I/O），然后接增量事件。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::Emitter;
use tracing::{debug, info, warn};

use crate::app::state::AppStateManager;
use crate::channel;
use crate::channel::oneshot;
use crate::fsworker::UidToken;
use crate::fsworker::protocol::WatchDelta;
use crate::mesh::types::instance::InstanceMsg;
use crate::mesh::types::ui::{
    ContextId, NavEntry, NavStatePayload, NavTarget, TabInfo, TabsPayload,
};

use super::UIService;
use super::breadcrumbs::{BreadcrumbCommand, BreadcrumbManager};

// ---------------------------------------------------------------------------
// 运行时 Tab 状态（含 UidToken）
// ---------------------------------------------------------------------------

/// 单个 tab 的完整运行时导航状态。
pub(super) struct TabNavState {
    pub id: u64,
    pub uid_token: UidToken,
    pub nav_history: Vec<NavEntry>,
    pub nav_index: usize,
}

impl TabNavState {
    fn title(&self) -> String {
        self.nav_history
            .get(self.nav_index)
            .map(|e| match &e.target {
                NavTarget::Dashboard => "Dashboard".to_string(),
                NavTarget::Filesystem(p) => {
                    let p = std::path::Path::new(&p);
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.to_string_lossy().to_string())
                }
            })
            .unwrap_or_else(|| "New Tab".to_string())
    }

    pub(super) fn nav_target(&self) -> NavTarget {
        self.nav_history
            .get(self.nav_index)
            .map(|e| e.target.clone())
            .unwrap_or(NavTarget::Filesystem("/".to_string()))
    }
}

// ---------------------------------------------------------------------------
// Watch 线程内部类型
// ---------------------------------------------------------------------------

/// 给 per-window watch 线程的控制命令。
pub(super) enum WatchCommand {
    /// 活跃 tab 切换。线程推送该 tab 的快照，然后接增量事件。
    TabSwitch { tab_id: u64 },
    /// Tab 导航目标变更。线程更新 tab_targets，必要时重建 watcher。
    NavUpdate { tab_id: u64, target: WatchTarget },
    /// Tab 关闭。线程清理 watcher + 快照。
    TabClosed { tab_id: u64 },
    /// 用户刷新（F5）。对 active tab 的 watcher 发 Refresh RPC。
    Refresh,
    /// 窗口关闭。清理所有 watcher 任务并退出。
    Shutdown,
}

/// 导航目标（watch 线程视角）。
#[derive(Clone)]
pub(super) enum WatchTarget {
    Filesystem { path: PathBuf, token: UidToken },
    Dashboard,
}

/// Per-watcher 最新状态快照。随 watcher 事件增量更新。
/// Tab 切换时直接 push 快照避免重新 list_dir。
enum WatchSnapshot {
    /// 正常直播中，path → File 映射
    Live {
        files: HashMap<PathBuf, crate::fsworker::protocol::File>,
    },
    /// 目录不可访问（权限拒绝等）
    Inaccessible {
        path: PathBuf,
        ancestor: PathBuf,
        level: u32,
        reason: String,
    },
    /// 级联恢复中
    Recovering {
        path: PathBuf,
        ancestor: PathBuf,
        level: u32,
    },
    /// Watcher 彻底失效
    FatalError { path: PathBuf, reason: String },
    /// Worker 连接断开
    ConnectionLost {
        watch_id: u64,
        reason: String,
        reconnecting: bool,
    },
}

impl WatchSnapshot {
    /// 初始空状态（首帧 Reset 到来前）。
    fn empty() -> Self {
        WatchSnapshot::Live {
            files: HashMap::new(),
        }
    }

    /// 更新快照以反应一次 WatchDelta 事件。
    fn apply(&mut self, delta: &WatchDelta) {
        match delta {
            WatchDelta::Reset(files) => {
                let mut map = HashMap::with_capacity(files.len());
                for f in files {
                    map.insert(f.path.clone(), f.clone());
                }
                *self = WatchSnapshot::Live { files: map };
            }
            WatchDelta::Upsert(file) => {
                if let WatchSnapshot::Live { files } = self {
                    files.insert(file.path.clone(), file.clone());
                }
            }
            WatchDelta::UpsertBatch(batch) => {
                if let WatchSnapshot::Live { files } = self {
                    for f in batch {
                        files.insert(f.path.clone(), f.clone());
                    }
                }
            }
            WatchDelta::Remove(path) => {
                if let WatchSnapshot::Live { files } = self {
                    files.remove(path);
                }
            }
            WatchDelta::Rename { from, to } => {
                if let WatchSnapshot::Live { files } = self {
                    if let Some(mut f) = files.remove(from) {
                        f.name = to
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        f.path = to.clone();
                        files.insert(to.clone(), f);
                    }
                }
            }
            WatchDelta::Inaccessible {
                path,
                ancestor,
                level,
                reason,
            } => {
                *self = WatchSnapshot::Inaccessible {
                    path: path.clone(),
                    ancestor: ancestor.clone(),
                    level: *level,
                    reason: reason.clone(),
                };
            }
            WatchDelta::Recovering {
                path,
                ancestor,
                level,
            } => {
                if matches!(self, WatchSnapshot::Live { .. }) {
                    // 不覆盖 Live 快照，Recovering 只是临时指示
                } else {
                    *self = WatchSnapshot::Recovering {
                        path: path.clone(),
                        ancestor: ancestor.clone(),
                        level: *level,
                    };
                }
            }
            WatchDelta::FatalError { path, reason } => {
                *self = WatchSnapshot::FatalError {
                    path: path.clone(),
                    reason: reason.clone(),
                };
            }
            WatchDelta::ConnectionLost {
                watch_id,
                reason,
                reconnecting,
            } => {
                *self = WatchSnapshot::ConnectionLost {
                    watch_id: *watch_id,
                    reason: reason.clone(),
                    reconnecting: *reconnecting,
                };
            }
            WatchDelta::BreadcrumbSegments(_) => {
                // 面包屑事件由 BreadcrumbManager 独立处理，
                // 文件列表快照无需处理此变体。
            }
        }
    }

    /// 将快照转为可 emit 的 WatchDelta。
    fn to_delta(&self) -> WatchDelta {
        match self {
            WatchSnapshot::Live { files } => WatchDelta::Reset(files.values().cloned().collect()),
            WatchSnapshot::Inaccessible {
                path,
                ancestor,
                level,
                reason,
            } => WatchDelta::Inaccessible {
                path: path.clone(),
                ancestor: ancestor.clone(),
                level: *level,
                reason: reason.clone(),
            },
            WatchSnapshot::Recovering {
                path,
                ancestor,
                level,
            } => WatchDelta::Recovering {
                path: path.clone(),
                ancestor: ancestor.clone(),
                level: *level,
            },
            WatchSnapshot::FatalError { path, reason } => WatchDelta::FatalError {
                path: path.clone(),
                reason: reason.clone(),
            },
            WatchSnapshot::ConnectionLost {
                watch_id,
                reason,
                reconnecting,
            } => WatchDelta::ConnectionLost {
                watch_id: *watch_id,
                reason: reason.clone(),
                reconnecting: *reconnecting,
            },
        }
    }
}

/// Watch 线程维护的 per-tab watcher 元数据。
struct TabWatchEntry {
    /// Watcher 转发任务句柄（abort = drop Watcher → unwatch）
    handle: tokio::task::JoinHandle<()>,
    /// 该 tab 当前文件列表快照
    snapshot: WatchSnapshot,
    /// Watch ID（由 watcher task 通过 oneshot 回报，0 表示尚未回报）
    watch_id: u64,
    /// 等待 watch_id 的 oneshot receiver（Some 表示尚未收到）
    watch_id_rx: Option<oneshot::RxOneshot<u64>>,
    /// UID Token（用于 Refresh RPC + 保活 Worker）
    token: UidToken,
    /// 代际计数器：每次 NavUpdate 递增，校验事件是否来自当前 watcher
    generation: u64,
}

// ---------------------------------------------------------------------------
// spawn_watcher_task — 为一个 tab 创建 watcher 转发任务
// ---------------------------------------------------------------------------

/// 创建 watcher 转发任务。任务持有 Watcher（RAII cleanup），
/// 将事件通过统一 channel 转发给主线程。
///
/// 返回 (JoinHandle, watch_id_rx)。
fn spawn_watcher_task(
    tab_id: u64,
    generation: u64,
    path: PathBuf,
    token: UidToken,
    event_tx: channel::Tx<(u64, u64, WatchDelta)>,
    mgr: Arc<AppStateManager>,
) -> (tokio::task::JoinHandle<()>, oneshot::RxOneshot<u64>) {
    let (id_tx, id_rx) = oneshot::oneshot();
    let handle = tokio::spawn(async move {
        let watcher = match mgr.fs_service.watch_dir(&token, &path).await {
            Ok(w) => w,
            Err(e) => {
                warn!("spawn_watcher_task tab={tab_id}: watch_dir failed: {e}");
                let _ = event_tx.send((
                    tab_id,
                    generation,
                    WatchDelta::FatalError {
                        path: path.clone(),
                        reason: e,
                    },
                ));
                let _ = id_tx.send(0);
                return;
            }
        };
        let watch_id = watcher.watch_id();
        let _ = id_tx.send(watch_id);
        debug!("spawn_watcher_task tab={tab_id} watch_id={watch_id} started");

        while let Ok(delta) = watcher.events.recv().await {
            if event_tx.send((tab_id, generation, delta)).is_err() {
                break;
            }
        }
        debug!("spawn_watcher_task tab={tab_id} gen={generation} watch_id={watch_id} ended");
    });
    (handle, id_rx)
}

// ---------------------------------------------------------------------------
// impl UIService — 导航与 Tab 管理
// ---------------------------------------------------------------------------

impl UIService {
    // -----------------------------------------------------------------------
    // 切换活跃 Tab
    // -----------------------------------------------------------------------

    /// 切换到指定 tab（前端点击 tab bar）。
    /// 仅更新状态 + 通知 watch 线程，不再直接管理 watcher。
    pub async fn switch_tab(&self, window: &tauri::Window, tab_id: u64) -> Result<(), String> {
        let label = window.label().to_string();
        if self.get_active_tab(&label) == Some(tab_id) {
            return Ok(());
        }

        if !self.tabs.read().unwrap().contains_key(&tab_id) {
            return Err(format!("tab {tab_id} not found"));
        }

        self.set_active_tab(window.label().to_string(), tab_id);
        self.emit_tabs(window);
        self.emit_nav_state(window, tab_id);

        if let Some(tx) = self.watch_txs.read().unwrap().get(&label) {
            let _ = tx.send(WatchCommand::TabSwitch { tab_id });
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Emit 事件辅助方法
    // -----------------------------------------------------------------------

    pub(super) fn emit_tabs(&self, window: &tauri::Window) {
        let tabs = self.tabs.read().unwrap();
        let active_id = self.get_active_tab(window.label()).unwrap_or_default();
        let tab_info: Vec<TabInfo> = tabs
            .values()
            .map(|t| TabInfo {
                id: t.id,
                title: t.title(),
                nav_target: t.nav_target(),
            })
            .collect();
        let _ = window.emit(
            "hf:tabs",
            TabsPayload {
                tabs: tab_info,
                active_tab_id: active_id,
            },
        );
    }

    pub(super) fn emit_nav_state(&self, window: &tauri::Window, tab_id: u64) {
        let (can_back, can_fwd, target) = {
            let tabs = self.tabs.read().unwrap();
            tabs.get(&tab_id)
                .map(|t| {
                    (
                        t.nav_index > 0,
                        t.nav_index + 1 < t.nav_history.len(),
                        t.nav_target(),
                    )
                })
                .unwrap_or((false, false, NavTarget::Filesystem("/".to_string())))
        };
        let _ = window.emit(
            "hf:nav-state",
            NavStatePayload {
                tab_id,
                target,
                can_go_back: can_back,
                can_go_forward: can_fwd,
            },
        );
    }

    // -----------------------------------------------------------------------
    // Watcher 生命周期 — 每窗口一个持久线程
    // -----------------------------------------------------------------------

    /// 为窗口启动持久 watch 线程（每个窗口调用一次）。
    ///
    /// 线程内部维护所有 tab 的 watcher 任务 + 文件列表快照。
    /// Tab 切换时直接 push 缓存快照，然后接增量事件。
    pub fn start_watch(&self, window: tauri::Window) {
        let label = window.label().to_string();
        let (cmd_tx, cmd_rx) = channel::unbounded::<WatchCommand>();
        let (event_tx, event_rx) = channel::unbounded::<(u64, u64, WatchDelta)>();

        self.watch_txs
            .write()
            .unwrap()
            .insert(label.clone(), cmd_tx);

        let mgr = self.mgr.clone();

        // 启动面包屑管理器（独立任务）
        let breadcrumb_mgr = BreadcrumbManager::start(window.clone(), mgr.clone());

        let h = tokio::spawn(async move {
            let mut tab_targets: HashMap<u64, WatchTarget> = HashMap::new();
            let mut tab_watches: HashMap<u64, TabWatchEntry> = HashMap::new();
            let mut tab_generations: HashMap<u64, u64> = HashMap::new();
            let mut active_tab: Option<u64> = None;

            loop {
                // 收取所有已就绪的 watch_id
                for entry in tab_watches.values_mut() {
                    if let Some(rx) = entry.watch_id_rx.as_mut() {
                        match rx.try_recv() {
                            Ok(wid) => {
                                entry.watch_id = wid;
                                entry.watch_id_rx = None;
                            }
                            Err(crossfire::TryRecvError::Empty) => {}
                            Err(crossfire::TryRecvError::Disconnected) => {
                                entry.watch_id_rx = None;
                            }
                        }
                    }
                }

                enum ThreadEvent {
                    Command(WatchCommand),
                    WatcherDelta {
                        tab_id: u64,
                        generation: u64,
                        delta: WatchDelta,
                    },
                }

                let result = {
                    let cmd_fut = cmd_rx.recv();
                    tokio::pin!(cmd_fut);

                    tokio::select! {
                        c = &mut cmd_fut => match c {
                            Ok(cmd) => ThreadEvent::Command(cmd),
                            Err(_) => break,
                        },
                        pair = event_rx.recv() => match pair {
                            Ok((tab_id, generation, delta)) => ThreadEvent::WatcherDelta { tab_id, generation, delta },
                            Err(_) => ThreadEvent::Command(WatchCommand::Shutdown),
                        },
                    }
                };

                match result {
                    ThreadEvent::Command(cmd) => match cmd {
                        WatchCommand::TabSwitch { tab_id } => {
                            active_tab = Some(tab_id);
                            if let Some(entry) = tab_watches.get(&tab_id) {
                                if entry.watch_id != 0 {
                                    let _ = window.emit("hf:file-list", &entry.snapshot.to_delta());
                                }
                            }
                            // 面包屑：推送该 tab 的缓存面包屑
                            breadcrumb_mgr.send(BreadcrumbCommand::TabSwitch { tab_id });
                        }
                        WatchCommand::NavUpdate { tab_id, target } => {
                            tab_targets.insert(tab_id, target.clone());
                            if let Some(old) = tab_watches.remove(&tab_id) {
                                old.handle.abort();
                            }
                            let generation = tab_generations
                                .entry(tab_id)
                                .and_modify(|c| *c += 1)
                                .or_insert(1);
                            let generation = *generation;
                            if let WatchTarget::Filesystem { path, token } = target {
                                let (handle, id_rx) = spawn_watcher_task(
                                    tab_id,
                                    generation,
                                    path.clone(),
                                    token.clone(),
                                    event_tx.clone(),
                                    mgr.clone(),
                                );
                                tab_watches.insert(
                                    tab_id,
                                    TabWatchEntry {
                                        handle,
                                        snapshot: WatchSnapshot::empty(),
                                        watch_id: 0,
                                        watch_id_rx: Some(id_rx),
                                        token: token.clone(),
                                        generation,
                                    },
                                );
                                // 面包屑：重建该 tab 的面包屑 watcher
                                breadcrumb_mgr.send(BreadcrumbCommand::NavUpdate {
                                    tab_id,
                                    path,
                                    token,
                                });
                            }
                        }
                        WatchCommand::TabClosed { tab_id } => {
                            if let Some(entry) = tab_watches.remove(&tab_id) {
                                entry.handle.abort();
                            }
                            tab_targets.remove(&tab_id);
                            tab_generations.remove(&tab_id);
                            if active_tab == Some(tab_id) {
                                active_tab = None;
                            }
                            // 面包屑：清理该 tab 的 watcher
                            breadcrumb_mgr.send(BreadcrumbCommand::TabClosed { tab_id });
                        }
                        WatchCommand::Refresh => {
                            // 对 active tab 发 Refresh RPC
                            if let Some(id) = active_tab {
                                if let Some(entry) = tab_watches.get(&id) {
                                    let token = entry.token.clone();
                                    let watch_id = entry.watch_id;
                                    if watch_id != 0 {
                                        tokio::spawn(async move {
                                            let _ = token
                                                .send_request(
                                                    crate::fsworker::WorkerRequestContent::Refresh {
                                                        watch_id,
                                                    },
                                                )
                                                .await;
                                        });
                                    } else {
                                        warn!("Refresh: watch_id=0 for active tab {id}, skipping");
                                    }
                                }
                            }
                        }
                        WatchCommand::Shutdown => {
                            breadcrumb_mgr.send(BreadcrumbCommand::Shutdown);
                            for (_, entry) in tab_watches.drain() {
                                entry.handle.abort();
                            }
                            return;
                        }
                    },
                    ThreadEvent::WatcherDelta {
                        tab_id,
                        generation,
                        delta,
                    } => {
                        // 校验代际：忽略旧 watcher 的残留事件
                        let Some(entry) = tab_watches.get_mut(&tab_id) else {
                            continue;
                        };
                        if entry.generation != generation {
                            continue;
                        }
                        entry.snapshot.apply(&delta);
                        // 仅 active tab 转发给前端
                        if Some(tab_id) == active_tab {
                            let _ = window.emit("hf:file-list", &delta);
                        }
                    }
                }
            }
        });
        // 后台监控 panic：watch 线程是整个窗口的文件系统事件入口，panic 必须可见
        let l = label.clone();
        tokio::spawn(async move {
            if let Err(e) = h.await {
                tracing::error!(window = %l, "watch thread panicked: {e}");
            }
        });

        info!("watch thread started for window={label}");
    }

    // -----------------------------------------------------------------------
    // 导航
    // -----------------------------------------------------------------------

    /// 导航到指定目标。更新历史 → 通知 watch 线程。
    pub async fn nav_to(
        &self,
        window: &tauri::Window,
        tab_id: u64,
        target: NavTarget,
    ) -> Result<(), String> {
        let token = self.get_tab_token(tab_id)?;

        {
            let mut tabs = self.tabs.write().unwrap();
            let tab = tabs
                .get_mut(&tab_id)
                .ok_or_else(|| format!("tab {tab_id} not found"))?;
            tab.nav_history.truncate(tab.nav_index + 1);
            tab.nav_history.push(NavEntry {
                target: target.clone(),
                selected: Vec::new(),
            });
            tab.nav_index = tab.nav_history.len() - 1;
        }

        self.emit_nav_state(window, tab_id);
        self.emit_tabs(window);

        let watch_target = match target {
            NavTarget::Dashboard => WatchTarget::Dashboard,
            NavTarget::Filesystem(path) => WatchTarget::Filesystem {
                path: PathBuf::from(&path),
                token,
            },
        };

        if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
            let _ = tx.send(WatchCommand::NavUpdate {
                tab_id,
                target: watch_target,
            });
        }

        Ok(())
    }

    /// 后退。
    pub async fn nav_back(&self, window: &tauri::Window, tab_id: u64) -> Result<(), String> {
        let (target, token) = {
            let mut tabs = self.tabs.write().unwrap();
            let tab = tabs
                .get_mut(&tab_id)
                .ok_or_else(|| format!("tab {tab_id} not found"))?;
            if tab.nav_index == 0 {
                return Err("already at oldest entry".to_string());
            }
            tab.nav_index -= 1;
            (tab.nav_target(), tab.uid_token.clone())
        };

        self.emit_nav_state(window, tab_id);

        let watch_target = match target {
            NavTarget::Dashboard => WatchTarget::Dashboard,
            NavTarget::Filesystem(path) => WatchTarget::Filesystem {
                path: PathBuf::from(&path),
                token,
            },
        };

        if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
            let _ = tx.send(WatchCommand::NavUpdate {
                tab_id,
                target: watch_target,
            });
        }

        Ok(())
    }

    /// 前进。
    pub async fn nav_forward(&self, window: &tauri::Window, tab_id: u64) -> Result<(), String> {
        let (target, token) = {
            let mut tabs = self.tabs.write().unwrap();
            let tab = tabs
                .get_mut(&tab_id)
                .ok_or_else(|| format!("tab {tab_id} not found"))?;
            if tab.nav_index + 1 >= tab.nav_history.len() {
                return Err("already at newest entry".to_string());
            }
            tab.nav_index += 1;
            (tab.nav_target(), tab.uid_token.clone())
        };

        self.emit_nav_state(window, tab_id);

        let watch_target = match target {
            NavTarget::Dashboard => WatchTarget::Dashboard,
            NavTarget::Filesystem(path) => WatchTarget::Filesystem {
                path: PathBuf::from(&path),
                token,
            },
        };

        if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
            let _ = tx.send(WatchCommand::NavUpdate {
                tab_id,
                target: watch_target,
            });
        }

        Ok(())
    }

    /// 更新选中文件，并 emit hf:selection。
    pub fn update_selection(&self, window: &tauri::Window, tab_id: u64, selected: Vec<String>) {
        {
            let mut tabs = self.tabs.write().unwrap();
            if let Some(tab) = tabs.get_mut(&tab_id) {
                if let Some(entry) = tab.nav_history.get_mut(tab.nav_index) {
                    entry.selected = selected.clone();
                }
            }
        }
        let _ = window.emit("hf:selection", &selected);
    }

    /// 发送 Refresh 命令到 watch 线程。
    pub fn refresh_tab(&self, window: &tauri::Window) {
        if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
            let _ = tx.send(WatchCommand::Refresh);
        }
    }

    // -----------------------------------------------------------------------
    // Tab 生命周期
    // -----------------------------------------------------------------------

    pub fn move_tab(&self, tab_id: u64, target: u64) -> super::MoveTabResult {
        let ctx = ContextId::Tab(tab_id);
        let busy = self.context_busy(&[ctx]);
        if !busy.is_empty() {
            return super::MoveTabResult::Blocked {
                tasks: busy
                    .into_iter()
                    .map(|op_id| super::TaskInfo {
                        task_id: op_id,
                        description: "active file operation".into(),
                    })
                    .collect(),
            };
        }
        self.execute_move_tab(tab_id, target);
        super::MoveTabResult::Ok
    }

    pub async fn move_tab_force(&self, tab_id: u64, target: u64) {
        let ctx = ContextId::Tab(tab_id);
        self.cancel_contexts(&[ctx]).await;
        self.execute_move_tab(tab_id, target);
    }

    fn execute_move_tab(&self, tab_id: u64, target: u64) {
        let target_instance = target >> 32;
        let is_local = target_instance == self.mgr.mesh.instance_bus().self_id();

        self.tabs.write().unwrap().remove(&tab_id);

        // 通知所有窗口的 watch 线程此 tab 已关闭
        for tx in self.watch_txs.read().unwrap().values() {
            let _ = tx.send(WatchCommand::TabClosed { tab_id });
        }

        {
            let mut active = self.active_tabs.write().unwrap();
            active.retain(|_, v| *v != tab_id);
        }

        if is_local {
            let mut tabs = self.mgr.tabs.lock().unwrap();
            if tabs.remove_tab(tab_id).is_some() {
                tabs.save_to_disk();
                info!("tab {tab_id} moved to local window {target}");
            }
        } else {
            let tab_state = {
                let mut tabs = self.mgr.tabs.lock().unwrap();
                match tabs.transfer_out(tab_id) {
                    Some(state) => {
                        tabs.save_to_disk();
                        state
                    }
                    None => {
                        warn!("tab {tab_id} not found for transfer");
                        return;
                    }
                }
            };

            let instance_bus = self.mgr.mesh.instance_bus().clone();
            let msg = InstanceMsg::TransferTab { tab: tab_state };
            tokio::spawn(async move {
                instance_bus.send_to(target_instance, &msg).await;
            });
            info!("tab {tab_id} moved to remote instance {target}");
        }
    }

    /// 关闭 tab。若关闭的是活跃 tab，自动切换到下一个。
    pub fn close_tab(self: &Arc<Self>, window: &tauri::Window, tab_id: u64) {
        let label = window.label().to_string();
        let was_active = self.get_active_tab(&label).map_or(false, |id| id == tab_id);

        self.tabs.write().unwrap().remove(&tab_id);
        self.clear_active_tab_any(tab_id);

        {
            let mut tabs = self.mgr.tabs.lock().unwrap();
            if tabs.remove_tab(tab_id).is_some() {
                tabs.save_to_disk();
            }
        }

        // 通知 watch 线程
        if let Some(tx) = self.watch_txs.read().unwrap().get(&label) {
            let _ = tx.send(WatchCommand::TabClosed { tab_id });
        }

        if was_active {
            if let Some(next_id) = self.tabs.read().unwrap().keys().next().copied() {
                self.set_active_tab(label.clone(), next_id);
                if let Some(tx) = self.watch_txs.read().unwrap().get(&label) {
                    let _ = tx.send(WatchCommand::TabSwitch { tab_id: next_id });
                }
            } else {
                info!("last tab closed — window may exit");
            }
        }

        self.emit_tabs(window);
    }

    /// 创建新 tab 并自动切换为活跃。
    pub async fn new_tab(
        &self,
        window: &tauri::Window,
        path: Option<String>,
    ) -> Result<u64, String> {
        let token = self.ensure_default_token().await?;
        let p = path.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));

        let tab_id = self.create_tab_internal(&token, p.clone()).await;

        self.set_active_tab(window.label().to_string(), tab_id);

        self.emit_tabs(window);
        self.emit_nav_state(window, tab_id);

        if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
            // 先推送 NavUpdate（带完整路径和 token）
            let _ = tx.send(WatchCommand::NavUpdate {
                tab_id,
                target: WatchTarget::Filesystem {
                    path: PathBuf::from(&p),
                    token,
                },
            });
            // 再切到新 tab
            let _ = tx.send(WatchCommand::TabSwitch { tab_id });
        }

        Ok(tab_id)
    }

    pub(super) async fn create_tab_internal(&self, token: &UidToken, path: String) -> u64 {
        let tab_id;
        {
            let mut tabs = self.mgr.tabs.lock().unwrap();
            let tab = tabs.add_tab(path.clone());
            tab_id = tab.id;
            tabs.save_to_disk();
        }
        {
            let mut runtime = self.tabs.write().unwrap();
            runtime.insert(
                tab_id,
                TabNavState {
                    id: tab_id,
                    uid_token: token.clone(),
                    nav_history: vec![NavEntry {
                        target: NavTarget::Filesystem(path),
                        selected: Vec::new(),
                    }],
                    nav_index: 0,
                },
            );
        }
        tab_id
    }

    fn clear_active_tab_any(&self, tab_id: u64) {
        let mut active = self.active_tabs.write().unwrap();
        active.retain(|_, v| *v != tab_id);
    }
}
