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
//!
//! ## 子模块
//!
//! - `nav` — 导航历史 + tab 管理 + watcher 生命周期
//! - `clipboard` — 剪贴板 + DnD 状态
//! - `progress` — 操作归组 + 进度泵

pub mod clipboard;
pub mod nav;
pub mod progress;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use tauri::Emitter;
use tokio::sync::Mutex as TokioMutex;
use tracing::{error, info};

use crate::app::fs_service::{Canceller, Op};
use crate::app::state::AppStateManager;
use crate::channel;
use crate::fsworker::UidToken;
use crate::ipc::protocol::{EntryKind, NavTarget};

use nav::TabNavState;

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
// UIService
// ---------------------------------------------------------------------------

pub struct UIService {
    pub mgr: Arc<AppStateManager>,

    /// tab_id → 运行时导航状态
    tabs: RwLock<HashMap<u64, TabNavState>>,
    /// window_label → active_tab_id
    active_tabs: RwLock<HashMap<String, u64>>,
    /// window_label → WatchCommand sender（per-window 持久 watch 线程）
    watch_txs: RwLock<HashMap<String, channel::Tx<nav::WatchCommand>>>,

    /// ContextId（tab/window）→ 活跃 op_id 集合
    contexts: Mutex<HashMap<crate::ipc::protocol::ContextId, HashSet<u64>>>,
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
            watch_txs: RwLock::new(HashMap::new()),
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
        self.active_tabs
            .write()
            .unwrap()
            .insert(window_label, tab_id);
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
                .create_tab_internal(&token, std::env::var("HOME").unwrap_or_else(|_| "/".into()))
                .await;
            self.set_active_tab(window.label().to_string(), tab_id);
        }

        // emit hf:tabs
        self.emit_tabs(window);
        // emit hf:clipboard
        let _ = window.emit("hf:clipboard", &self.clipboard_state());

        // 启动 per-window 持久 watch 线程
        self.start_watch(window.clone());

        // 为所有 tab 发送初始 NavUpdate
        {
            let tabs = self.tabs.read().unwrap();
            for tab in tabs.values() {
                let target = tab.nav_target();
                let watch_target = match target {
                    NavTarget::Dashboard => nav::WatchTarget::Dashboard,
                    NavTarget::Filesystem(p) => nav::WatchTarget::Filesystem {
                        path: PathBuf::from(&p),
                        token: tab.uid_token.clone(),
                    },
                };
                if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
                    let _ = tx.send(nav::WatchCommand::NavUpdate {
                        tab_id: tab.id,
                        target: watch_target,
                    });
                }
            }
        }

        // 切换到活跃 tab
        if let Some(active_id) = self.get_active_tab(window.label()) {
            self.emit_nav_state(window, active_id);
            if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
                let _ = tx.send(nav::WatchCommand::TabSwitch { tab_id: active_id });
            }
        } else {
            return Err("no tabs available".into());
        }

        Ok(())
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

        {
            let mut tabs = self.tabs.write().unwrap();
            match tabs.get_mut(&tab_id) {
                Some(tab) => {
                    tab.uid_token = new_token.clone();
                }
                None => return Err(format!("tab {tab_id} not found")),
            }
        }

        // 通知 watch 线程：token 已更换
        if let Some(tx) = self.watch_txs.read().unwrap().get(window.label()) {
            let target = {
                let tabs = self.tabs.read().unwrap();
                tabs.get(&tab_id)
                    .map(|t| t.nav_target())
                    .unwrap_or(NavTarget::Filesystem("/".to_string()))
            };
            let watch_target = match target {
                NavTarget::Dashboard => nav::WatchTarget::Dashboard,
                NavTarget::Filesystem(p) => nav::WatchTarget::Filesystem {
                    path: PathBuf::from(&p),
                    token: new_token,
                },
            };
            let _ = tx.send(nav::WatchCommand::NavUpdate {
                tab_id,
                target: watch_target,
            });
        }
        Ok(())
    }

    /// 窗口关闭时通知 watch 线程清理。
    pub fn shutdown_watch(&self, window_label: &str) {
        if let Some(tx) = self.watch_txs.write().unwrap().remove(window_label) {
            let _ = tx.send(nav::WatchCommand::Shutdown);
        }
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
                    path: PathBuf::from("/"),
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
    // 文件操作入口
    // -----------------------------------------------------------------------

    pub async fn create(
        self: &Arc<Self>,
        tab_id: u64,
        path: &Path,
        kind: EntryKind,
        ctx_id: crate::ipc::protocol::ContextId,
    ) -> Result<(), String> {
        let token = self.get_tab_token(tab_id)?;
        let progress = self.mgr.fs_service.create(&token, path, kind).await?;
        self.track_op(&[ctx_id], progress);
        Ok(())
    }

    pub async fn rename(
        self: &Arc<Self>,
        tab_id: u64,
        path: &Path,
        new_name: &str,
        ctx_id: crate::ipc::protocol::ContextId,
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
        ctx_id: crate::ipc::protocol::ContextId,
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
        ctx_id: crate::ipc::protocol::ContextId,
    ) -> Result<(), String> {
        let _token = self.get_tab_token(tab_id)?;
        let progress = self.mgr.fs_service.copy(ops).await?;
        self.track_op(&[ctx_id], progress);
        Ok(())
    }

    /// 通过路径对构造 Op 并执行 move（供 Tauri command 使用）。
    /// 所有 src/dst 使用 tab 的默认 token（跨 UID 尚未实现）。
    pub async fn move_files_by_paths(
        self: &Arc<Self>,
        tab_id: u64,
        pairs: Vec<(PathBuf, PathBuf)>,
        ctx_id: crate::ipc::protocol::ContextId,
    ) -> Result<(), String> {
        let token = self.get_tab_token(tab_id)?;
        let ops: Vec<Op> = pairs
            .into_iter()
            .map(|(src, dst)| Op {
                src: (token.clone(), src),
                dst: (token.clone(), dst),
            })
            .collect();
        let progress = self.mgr.fs_service.move_(ops).await?;
        self.track_op(&[ctx_id], progress);
        Ok(())
    }

    /// 通过路径对构造 Op 并执行 copy（供 Tauri command 使用）。
    pub async fn copy_files_by_paths(
        self: &Arc<Self>,
        tab_id: u64,
        pairs: Vec<(PathBuf, PathBuf)>,
        ctx_id: crate::ipc::protocol::ContextId,
    ) -> Result<(), String> {
        let token = self.get_tab_token(tab_id)?;
        let ops: Vec<Op> = pairs
            .into_iter()
            .map(|(src, dst)| Op {
                src: (token.clone(), src),
                dst: (token.clone(), dst),
            })
            .collect();
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
}
