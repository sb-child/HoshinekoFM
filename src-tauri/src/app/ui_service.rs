//! UI 服务 — 前端状态机后端。
//!
//! UIService 是前端所有 `invoke()` 调的**唯一入口**。全权管理 tab 导航历史、
//! 目录 watcher、选中状态、剪贴板。前端为纯渲染层，所有数据通过 event 推送。
//!
//! ## 分层
//!
//! ```text
//! 前端 invoke ──→ UIService (状态机 + 事件编排)
//!                     │ emit: hf:tabs / hf:file-list / hf:nav-state / hf:dashboard / hf:clipboard / hf:selection / hf:progress
//!                     │ 持有: TabNavState / JoinHandle<()> / Progress / ContextId→ops 映射
//!                     ▼
//!                 FsService (纯调度, 不感知 tab)
//! ```
//!
//! ## 设计原则
//!
//! - 前端只发意图，后端 push 所有状态变化（事件驱动）。
//! - 决不暴露裸 `uid: u32`。所有方法通过 `tab_id` 间接获取 `UidToken`。
//! - 导航先入栈，后验证。不可访问目录也记录在历史中。
//! - 每个窗口只有一个活跃 tab，非活跃 tab 的 watcher 停止推送。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use tauri::Emitter;
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, error, info, warn};

use crate::app::fs_service::{Canceller, Op, Progress};
use crate::app::state::AppStateManager;
use crate::fsworker::UidToken;
use crate::ipc::protocol::{
    ClipOp, ClipboardState, ContextId, EntryKind, InstanceMessage, NavEntry, NavStatePayload,
    NavTarget, ProgressEvent, TabInfo, TabsPayload, WatchDelta,
};

// ---------------------------------------------------------------------------
// MoveTabResult
// ---------------------------------------------------------------------------

/// 阻塞 move_tab 的活跃任务信息（供前端弹窗展示）。
#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub task_id: u64,
    pub description: String,
}

/// move_tab 的结果。
#[derive(Debug, Clone)]
pub enum MoveTabResult {
    Ok,
    Blocked { tasks: Vec<TaskInfo> },
}

// ---------------------------------------------------------------------------
// 运行时 Tab 状态（含 UidToken）
// ---------------------------------------------------------------------------

/// 单个 tab 的完整运行时导航状态。
pub struct TabNavState {
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

    fn nav_target(&self) -> NavTarget {
        self.nav_history
            .get(self.nav_index)
            .map(|e| e.target.clone())
            .unwrap_or(NavTarget::Filesystem("/".to_string()))
    }
}

// ---------------------------------------------------------------------------
// UIService
// ---------------------------------------------------------------------------

pub struct UIService {
    pub mgr: Arc<AppStateManager>,

    /// tab_id → 运行时导航状态
    tabs: RwLock<HashMap<u64, TabNavState>>,
    /// window_label → active_tab_id
    active_tabs: RwLock<HashMap<String, u64>>,
    /// tab_id → watcher 后台任务句柄（abort = drop Watcher → unwatch）
    file_watchers: RwLock<HashMap<u64, tokio::task::JoinHandle<()>>>,

    /// ContextId（tab/window）→ 活跃 op_id 集合
    contexts: Mutex<HashMap<ContextId, HashSet<u64>>>,
    /// op_id → 取消句柄
    cancels: Mutex<HashMap<u64, Canceller>>,

    /// 默认 UidToken（当前用户 uid），所有普通 tab 从此 clone
    default_token: TokioMutex<Option<UidToken>>,
}

impl UIService {
    pub fn new(mgr: Arc<AppStateManager>) -> Self {
        Self {
            mgr,
            tabs: RwLock::new(HashMap::new()),
            active_tabs: RwLock::new(HashMap::new()),
            file_watchers: RwLock::new(HashMap::new()),
            contexts: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            default_token: TokioMutex::new(None),
        }
    }

    /// 确保默认 UidToken 已初始化（懒加载）。
    async fn ensure_default_token(&self) -> Result<UidToken, String> {
        let mut guard = self.default_token.lock().await;
        if let Some(ref token) = *guard {
            return Ok(token.clone());
        }
        let uid = unsafe { libc::geteuid() };
        let token = self.mgr.fs_service.try_request_uid_token(uid).await?;
        *guard = Some(token.clone());
        Ok(token)
    }

    // -----------------------------------------------------------------------
    // 访问内部 tab 状态
    // -----------------------------------------------------------------------

    fn get_tab_token(&self, tab_id: u64) -> Result<UidToken, String> {
        let tabs = self.tabs.read().unwrap();
        tabs.get(&tab_id)
            .map(|t| t.uid_token.clone())
            .ok_or_else(|| format!("tab {tab_id} not found"))
    }

    fn get_active_tab(&self, window_label: &str) -> Option<u64> {
        self.active_tabs.read().unwrap().get(window_label).copied()
    }

    fn set_active_tab(&self, window_label: String, tab_id: u64) {
        self.active_tabs.write().unwrap().insert(window_label, tab_id);
    }

    // -----------------------------------------------------------------------
    // Ready — 前端就绪初始化
    // -----------------------------------------------------------------------

    /// 前端 mount 后调用，由 UIService 重建运行时状态并推送初始事件。
    pub async fn ready(&self, window: &tauri::Window) -> Result<(), String> {
        let token = self.ensure_default_token().await?;

        let persis_tabs: Vec<crate::ipc::protocol::TabState> = {
            let tabs = self.mgr.tabs.lock().unwrap();
            tabs.get_all().to_vec()
        };

        // 用默认 token 重建运行时 TabNavState
        {
            let mut runtime = self.tabs.write().unwrap();
            for ts in &persis_tabs {
                runtime.entry(ts.id).or_insert_with(|| TabNavState {
                    id: ts.id,
                    uid_token: token.clone(),
                    nav_history: ts.nav_history.clone(),
                    nav_index: ts.nav_index,
                });
            }
        }

        // 首 tab 为活跃 tab
        if let Some(first) = persis_tabs.first() {
            self.set_active_tab(window.label().to_string(), first.id);
        } else {
            // 无持久化 tab → 自动创建一个
            let tab_id = self
                .create_tab_internal(
                    &token,
                    std::env::var("HOME").unwrap_or_else(|_| "/".into()),
                )
                .await;
            self.set_active_tab(window.label().to_string(), tab_id);
        }

        // emit hf:tabs
        self.emit_tabs(window);
        // emit hf:clipboard
        let _ = window.emit("hf:clipboard", &self.clipboard_state());

        // 为活跃 tab 启动 watcher
        if let Some(active_id) = self.get_active_tab(window.label()) {
            self.emit_nav_state(window, active_id);
            self.start_watch(window.clone(), active_id).await;
        } else {
            return Err("no tabs available".into());
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // 切换活跃 Tab
    // -----------------------------------------------------------------------

    /// 切换到指定 tab（前端点击 tab bar）。
    pub async fn switch_tab(&self, window: &tauri::Window, tab_id: u64) -> Result<(), String> {
        let old_label = window.label().to_string();
        if let Some(old_id) = self.get_active_tab(&old_label) {
            if old_id == tab_id {
                return Ok(()); // 已经是活跃 tab
            }
            self.stop_watch(old_id);
        }

        if !self.tabs.read().unwrap().contains_key(&tab_id) {
            return Err(format!("tab {tab_id} not found"));
        }

        self.set_active_tab(window.label().to_string(), tab_id);
        self.emit_tabs(window);
        self.emit_nav_state(window, tab_id);
        self.start_watch(window.clone(), tab_id).await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Emit 事件辅助方法
    // -----------------------------------------------------------------------

    fn emit_tabs(&self, window: &tauri::Window) {
        let tabs = self.tabs.read().unwrap();
        let active_id = self
            .get_active_tab(window.label())
            .unwrap_or_default();
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

    fn emit_nav_state(&self, window: &tauri::Window, tab_id: u64) {
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
    // Watcher 生命周期
    // -----------------------------------------------------------------------

    /// 为 tab 启动文件 watcher 后台任务。若是 Dashboard 则 emit dashboard 数据。
    async fn start_watch(&self, window: tauri::Window, tab_id: u64) {
        let token = match self.get_tab_token(tab_id) {
            Ok(t) => t,
            Err(e) => {
                warn!("start_watch: {e}");
                return;
            }
        };

        let target = {
            let tabs = self.tabs.read().unwrap();
            tabs.get(&tab_id)
                .and_then(|t| t.nav_history.get(t.nav_index))
                .map(|e| e.target.clone())
                .unwrap_or(NavTarget::Filesystem("/".to_string()))
        };

        match &target {
            NavTarget::Dashboard => {
                match self.build_dashboard(&token).await {
                    Ok(data) => {
                        let _ = window.emit("hf:dashboard", &data);
                    }
                    Err(e) => {
                        warn!("dashboard build failed: {e}");
                    }
                }
            }
            NavTarget::Filesystem(path) => {
                match self.mgr.fs_service.watch_dir(&token, path).await {
                    Ok(watcher) => {
                        let handle = tokio::spawn(async move {
                            let mut watcher = watcher;
                            while let Some(delta) = watcher.events.recv().await {
                                let _ = window.emit("hf:file-list", &delta);
                            }
                        });
                        self.file_watchers
                            .write()
                            .unwrap()
                            .insert(tab_id, handle);
                    }
                    Err(e) => {
                        warn!("watch_dir failed for {path}: {e}");
                        let _ = window.emit(
                            "hf:file-list",
                            &WatchDelta::Inaccessible {
                                path: path.clone(),
                                reason: e,
                            },
                        );
                    }
                }
            }
        }
    }

    /// 停止 tab 的文件 watcher 后台任务。
    fn stop_watch(&self, tab_id: u64) {
        if let Some(handle) = self.file_watchers.write().unwrap().remove(&tab_id) {
            handle.abort();
        }
    }

    // -----------------------------------------------------------------------
    // 导航
    // -----------------------------------------------------------------------

    /// 导航到指定目标。push 历史 → 启动 watcher → emit 事件。
    pub async fn nav_to(
        &self,
        window: &tauri::Window,
        tab_id: u64,
        target: NavTarget,
    ) -> Result<(), String> {
        let _token = self.get_tab_token(tab_id)?;

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

        self.stop_watch(tab_id);
        self.emit_nav_state(window, tab_id);
        self.emit_tabs(window);
        self.start_watch(window.clone(), tab_id).await;
        Ok(())
    }

    /// 后退。
    pub async fn nav_back(
        &self,
        window: &tauri::Window,
        tab_id: u64,
    ) -> Result<(), String> {
        {
            let mut tabs = self.tabs.write().unwrap();
            let tab = tabs
                .get_mut(&tab_id)
                .ok_or_else(|| format!("tab {tab_id} not found"))?;
            if tab.nav_index == 0 {
                return Err("already at oldest entry".to_string());
            }
            tab.nav_index -= 1;
        }
        self.stop_watch(tab_id);
        self.emit_nav_state(window, tab_id);
        self.start_watch(window.clone(), tab_id).await;
        Ok(())
    }

    /// 前进。
    pub async fn nav_forward(
        &self,
        window: &tauri::Window,
        tab_id: u64,
    ) -> Result<(), String> {
        {
            let mut tabs = self.tabs.write().unwrap();
            let tab = tabs
                .get_mut(&tab_id)
                .ok_or_else(|| format!("tab {tab_id} not found"))?;
            if tab.nav_index + 1 >= tab.nav_history.len() {
                return Err("already at newest entry".to_string());
            }
            tab.nav_index += 1;
        }
        self.stop_watch(tab_id);
        self.emit_nav_state(window, tab_id);
        self.start_watch(window.clone(), tab_id).await;
        Ok(())
    }

    /// 更新当前 nav 位置的选中文件列表，并 emit hf:selection。
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

    // -----------------------------------------------------------------------
    // 权限变更
    // -----------------------------------------------------------------------

    /// 将 tab 提升到指定 uid。
    pub async fn elevate_tab(
        &self,
        window: &tauri::Window,
        tab_id: u64,
        target_uid: u32,
    ) -> Result<(), String> {
        let new_token = self
            .mgr
            .fs_service
            .try_request_uid_token(target_uid)
            .await?;

        self.stop_watch(tab_id);

        {
            let mut tabs = self.tabs.write().unwrap();
            match tabs.get_mut(&tab_id) {
                Some(tab) => {
                    tab.uid_token = new_token;
                }
                None => return Err(format!("tab {tab_id} not found")),
            }
        }

        self.start_watch(window.clone(), tab_id).await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Dashboard 构建
    // -----------------------------------------------------------------------

    async fn build_dashboard(
        &self,
        token: &UidToken,
    ) -> Result<crate::ipc::protocol::DashboardData, String> {
        use crate::ipc::protocol::{CommonLocation, DashboardData, StorageSummary};

        let (total, free) = {
            let resp = token
                .send_request(crate::fsworker::WorkerRequestContent::StatVfs {
                    path: "/".to_string(),
                })
                .await
                .map_err(|e| format!("statvfs: {e}"))?;
            match resp {
                crate::fsworker::WorkerResponse::StatVfsResult {
                    total_bytes,
                    free_bytes,
                } => (total_bytes, free_bytes),
                crate::fsworker::WorkerResponse::Err(e) => return Err(e),
                _ => return Err("unexpected worker response".to_string()),
            }
        };

        let used = total.saturating_sub(free);
        let storage = StorageSummary {
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
        };

        let uid = token.uid();
        let home = if uid == 0 {
            "/root".to_string()
        } else {
            let euid = unsafe { libc::geteuid() };
            if uid == euid {
                std::env::var("HOME").unwrap_or_else(|_| format!("/home/{uid}"))
            } else {
                format!("/home/{uid}")
            }
        };

        let locations = vec![
            ("sidebar.home", home.clone()),
            ("sidebar.desktop", format!("{home}/Desktop")),
            ("sidebar.documents", format!("{home}/Documents")),
            ("sidebar.downloads", format!("{home}/Downloads")),
            ("sidebar.music", format!("{home}/Music")),
            ("sidebar.pictures", format!("{home}/Pictures")),
            ("sidebar.videos", format!("{home}/Videos")),
        ];

        let common_locations: Vec<CommonLocation> = locations
            .into_iter()
            .map(|(name, path)| CommonLocation {
                name: name.to_string(),
                exists: std::path::Path::new(&path).exists(),
                path,
            })
            .collect();

        Ok(DashboardData {
            storage,
            common_locations,
        })
    }

    // -----------------------------------------------------------------------
    // Progress 归组
    // -----------------------------------------------------------------------

    pub fn track_op(self: &Arc<Self>, context_ids: &[ContextId], progress: Progress) {
        let op_id = progress.op_id();
        {
            let mut ctxs = self.contexts.lock().unwrap();
            for cid in context_ids {
                ctxs.entry(*cid).or_default().insert(op_id);
            }
        }
        self.cancels
            .lock()
            .unwrap()
            .insert(op_id, progress.canceller());

        let ctx_ids: Vec<ContextId> = context_ids.to_vec();
        let this = self.clone();
        let mut progress = progress;
        tokio::spawn(async move {
            while let Some(ev) = progress.events.recv().await {
                match ev {
                    ProgressEvent::Conflict { conflict_id, .. } => {
                        progress.resolve(
                            conflict_id,
                            crate::ipc::protocol::ConflictResolution::AutoRename,
                        );
                    }
                    ProgressEvent::Done { .. }
                    | ProgressEvent::ConnectionLost {
                        reconnecting: false, ..
                    } => {
                        break;
                    }
                    _ => {}
                }
            }
            this.forget_op(&ctx_ids, op_id);
        });
    }

    fn forget_op(&self, context_ids: &[ContextId], op_id: u64) {
        let mut ctxs = self.contexts.lock().unwrap();
        for cid in context_ids {
            if let Some(set) = ctxs.get_mut(cid) {
                set.remove(&op_id);
            }
        }
        ctxs.retain(|_, v| !v.is_empty());
        self.cancels.lock().unwrap().remove(&op_id);
    }

    pub fn context_busy(&self, ids: &[ContextId]) -> Vec<u64> {
        let ctxs = self.contexts.lock().unwrap();
        let mut ops = Vec::new();
        for id in ids {
            if let Some(set) = ctxs.get(id) {
                ops.extend(set.iter().copied());
            }
        }
        ops
    }

    pub async fn cancel_contexts(&self, ids: &[ContextId]) {
        let cancellers: Vec<Canceller> = {
            let ctxs = self.contexts.lock().unwrap();
            let cancels = self.cancels.lock().unwrap();
            let mut v = Vec::new();
            for id in ids {
                if let Some(set) = ctxs.get(id) {
                    for op in set {
                        if let Some(c) = cancels.get(op) {
                            v.push(c.clone());
                        }
                    }
                }
            }
            v
        };
        for c in cancellers {
            c.cancel().await;
        }
    }

    // -----------------------------------------------------------------------
    // Tab 生命周期
    // -----------------------------------------------------------------------

    pub fn move_tab(&self, tab_id: u64, target: u64) -> MoveTabResult {
        let ctx = ContextId::Tab(tab_id);
        let busy = self.context_busy(&[ctx]);
        if !busy.is_empty() {
            return MoveTabResult::Blocked {
                tasks: busy
                    .into_iter()
                    .map(|op_id| TaskInfo {
                        task_id: op_id,
                        description: "active file operation".into(),
                    })
                    .collect(),
            };
        }
        self.execute_move_tab(tab_id, target);
        MoveTabResult::Ok
    }

    pub async fn move_tab_force(&self, tab_id: u64, target: u64) {
        let ctx = ContextId::Tab(tab_id);
        self.cancel_contexts(&[ctx]).await;
        self.execute_move_tab(tab_id, target);
    }

    fn execute_move_tab(&self, tab_id: u64, target: u64) {
        let target_instance = target >> 32;
        let is_local = target_instance == self.mgr.instance_bus.self_id();

        self.tabs.write().unwrap().remove(&tab_id);
        self.stop_watch(tab_id);

        // 清理此 tab 在各个窗口的 active 记录
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

            let instance_bus = self.mgr.instance_bus.clone();
            let msg = InstanceMessage::TransferTab {
                tab: tab_state,
                to_instance: target_instance,
            };
            tokio::spawn(async move {
                instance_bus.send_to(target_instance, &msg).await;
            });
            info!("tab {tab_id} moved to remote instance {target}");
        }
    }

    /// 关闭 tab。若关闭的是活跃 tab，自动切换到下一个。
    pub fn close_tab(self: &Arc<Self>, window: &tauri::Window, tab_id: u64) {
        let was_active = self
            .get_active_tab(window.label())
            .map_or(false, |id| id == tab_id);

        self.tabs.write().unwrap().remove(&tab_id);
        self.stop_watch(tab_id);
        self.clear_active_tab_any(tab_id);

        let mut tabs = self.mgr.tabs.lock().unwrap();
        if tabs.remove_tab(tab_id).is_some() {
            tabs.save_to_disk();
        }
        drop(tabs);

        if was_active {
            // 找下一个 tab 作为活跃 tab
            if let Some(next_id) = self.tabs.read().unwrap().keys().next().copied() {
                self.set_active_tab(window.label().to_string(), next_id);
                let window_clone = window.clone();
                let this = self.clone();
                tokio::spawn(async move {
                    this.start_watch(window_clone, next_id).await;
                });
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

        let tab_id = self.create_tab_internal(&token, p).await;

        // 停止旧活跃 tab 的 watcher
        if let Some(old_id) = self.get_active_tab(window.label()) {
            self.stop_watch(old_id);
        }
        self.set_active_tab(window.label().to_string(), tab_id);

        self.emit_tabs(window);
        self.emit_nav_state(window, tab_id);
        self.start_watch(window.clone(), tab_id).await;

        Ok(tab_id)
    }

    async fn create_tab_internal(&self, token: &UidToken, path: String) -> u64 {
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

    /// 接收来自其他实例的 tab 转移。
    pub async fn receive_transfer_tab(&self, tab_state: crate::ipc::protocol::TabState) {
        let token = self.ensure_default_token().await;
        let tab_id;
        {
            let mut tabs = self.mgr.tabs.lock().unwrap();
            tabs.transfer_in(tab_state.clone());
            tabs.save_to_disk();
            tab_id = tab_state.id;
        }
        if let Ok(token) = token {
            self.tabs.write().unwrap().insert(
                tab_id,
                TabNavState {
                    id: tab_id,
                    uid_token: token,
                    nav_history: tab_state.nav_history,
                    nav_index: tab_state.nav_index,
                },
            );
        }
        info!("received transferred tab {tab_id}");
    }

    fn clear_active_tab_any(&self, tab_id: u64) {
        let mut active = self.active_tabs.write().unwrap();
        active.retain(|_, v| *v != tab_id);
    }

    // -----------------------------------------------------------------------
    // 剪贴板
    // -----------------------------------------------------------------------

    fn clipboard_inner(&self) -> &Mutex<ClipboardState> {
        static CLIPBOARD: std::sync::LazyLock<Mutex<ClipboardState>> =
            std::sync::LazyLock::new(|| {
                Mutex::new(ClipboardState {
                    operation: None,
                    files: Vec::new(),
                })
            });
        &CLIPBOARD
    }

    pub fn clip_copy(&self, window: &tauri::Window, paths: &[String]) {
        {
            let mut cb = self.clipboard_inner().lock().unwrap();
            cb.operation = Some(ClipOp::Copy);
            cb.files = paths.to_vec();
        }
        let _ = window.emit("hf:clipboard", &self.clipboard_state());
        debug!("clip_copy: {} files", paths.len());
    }

    pub fn clip_cut(&self, window: &tauri::Window, paths: &[String]) {
        {
            let mut cb = self.clipboard_inner().lock().unwrap();
            cb.operation = Some(ClipOp::Cut);
            cb.files = paths.to_vec();
        }
        let _ = window.emit("hf:clipboard", &self.clipboard_state());
        debug!("clip_cut: {} files", paths.len());
    }

    pub fn clipboard_state(&self) -> ClipboardState {
        self.clipboard_inner().lock().unwrap().clone()
    }

    pub fn clipboard_sync(&self, state: ClipboardState) {
        {
            let mut cb = self.clipboard_inner().lock().unwrap();
            *cb = state;
        }
        // broadcast clipboard to all local windows
        let reg = self.mgr.window_registry.lock().unwrap();
        let cb_state = self.clipboard_state();
        for window in reg.values() {
            let _ = window.emit("hf:clipboard", &cb_state);
        }
        debug!("clipboard_sync: updated from remote");
    }

    // -----------------------------------------------------------------------
    // 文件操作入口
    // -----------------------------------------------------------------------

    pub async fn create(
        self: &Arc<Self>,
        tab_id: u64,
        path: &str,
        kind: EntryKind,
        ctx_id: ContextId,
    ) -> Result<(), String> {
        let token = self.get_tab_token(tab_id)?;
        let progress = self.mgr.fs_service.create(&token, path, kind).await?;
        self.track_op(&[ctx_id], progress);
        Ok(())
    }

    pub async fn rename(
        self: &Arc<Self>,
        tab_id: u64,
        path: &str,
        new_name: &str,
        ctx_id: ContextId,
    ) -> Result<(), String> {
        let token = self.get_tab_token(tab_id)?;
        let progress = self.mgr.fs_service.rename(&token, path, new_name).await?;
        self.track_op(&[ctx_id], progress);
        Ok(())
    }

    pub async fn move_files(
        self: &Arc<Self>,
        tab_id: u64,
        ops: Vec<Op>,
        ctx_id: ContextId,
    ) -> Result<(), String> {
        let _token = self.get_tab_token(tab_id)?;
        let progress = self.mgr.fs_service.move_(ops).await?;
        self.track_op(&[ctx_id], progress);
        Ok(())
    }

    pub async fn copy_files(
        self: &Arc<Self>,
        tab_id: u64,
        ops: Vec<Op>,
        ctx_id: ContextId,
    ) -> Result<(), String> {
        let _token = self.get_tab_token(tab_id)?;
        let progress = self.mgr.fs_service.copy(ops).await?;
        self.track_op(&[ctx_id], progress);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Instance Bus 接口
    // -----------------------------------------------------------------------

    pub async fn open_window(&self, paths: Vec<String>) {
        let label = self.mgr.next_label();
        match crate::app::commands::create_window(self.mgr.app_handle(), &label, &paths) {
            Ok(window) => {
                self.mgr
                    .register_window(self.mgr.instance_bus.clone(), window, label)
                    .await;
            }
            Err(e) => {
                error!("open_window: failed to create window: {e}");
            }
        }
    }
}
