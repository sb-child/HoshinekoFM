//! 导航历史 + tab 管理 + watcher 生命周期。

use std::path::{Path, PathBuf};

use tauri::Emitter;
use tracing::{debug, info, warn};

use crate::fsworker::UidToken;
use crate::ipc::protocol::{
    ContextId, InstanceMessage, NavEntry, NavStatePayload, NavTarget, TabInfo, TabsPayload,
    WatchDelta,
};

use super::UIService;

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

    fn nav_target(&self) -> NavTarget {
        self.nav_history
            .get(self.nav_index)
            .map(|e| e.target.clone())
            .unwrap_or(NavTarget::Filesystem("/".to_string()))
    }
}

// ---------------------------------------------------------------------------
// impl UIService — 导航与 Tab 管理
// ---------------------------------------------------------------------------

impl UIService {
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
    // Watcher 生命周期
    // -----------------------------------------------------------------------

    /// 为 tab 启动文件 watcher 后台任务。若是 Dashboard 则 emit dashboard 数据。
    pub(super) async fn start_watch(&self, window: tauri::Window, tab_id: u64) {
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
                match self
                    .mgr
                    .fs_service
                    .watch_dir(&token, Path::new(path))
                    .await
                {
                    Ok(watcher) => {
                        let tab_id_pump = tab_id;
                        let path_pump = path.clone();
                        let handle = tokio::spawn(async move {
                            let mut watcher = watcher;
                            info!(
                                "start_watch pump started for tab={tab_id_pump} path={path_pump}"
                            );
                            while let Some(delta) = watcher.events.recv().await {
                                match &delta {
                                    WatchDelta::ConnectionLost {
                                        reason,
                                        reconnecting,
                                        ..
                                    } => {
                                        info!("start_watch pump tab={tab_id_pump}: ConnectionLost reason={reason} reconnecting={reconnecting}");
                                    }
                                    WatchDelta::Reset(files) => {
                                        debug!("start_watch pump tab={tab_id_pump}: Reset {} entries", files.len());
                                    }
                                    _ => {}
                                }
                                let _ = window.emit("hf:file-list", &delta);
                            }
                            info!(
                                "start_watch pump ended for tab={tab_id_pump} path={path_pump}"
                            );
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
                                path: PathBuf::from(path.clone()),
                                ancestor: PathBuf::from(path),
                                level: 0,
                                reason: e,
                            },
                        );
                    }
                }
            }
        }
    }

    /// 停止 tab 的文件 watcher 后台任务。
    pub(super) fn stop_watch(&self, tab_id: u64) {
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
    pub fn update_selection(
        &self,
        window: &tauri::Window,
        tab_id: u64,
        selected: Vec<String>,
    ) {
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
    pub fn close_tab(self: &std::sync::Arc<Self>, window: &tauri::Window, tab_id: u64) {
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
