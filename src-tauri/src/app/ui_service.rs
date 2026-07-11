//! UI 服务 — per-instance 协调器。
//!
//! UIService 是前端所有 `invoke()` 命令的**唯一入口**。不持有业务数据，
//! 只持有其他模块的引用，以及**跨调用存活的协调句柄**（UidToken / Watcher / Progress）。
//!
//! ## 分层
//!
//! ```text
//! 前端 invoke ──→ UIService (命令入口 + 事件编排)
//!                     │  持有: UidToken / Watcher / Progress / ContextId→ops 映射
//!                     ▼
//!                 FsService (纯调度)
//! ```
//!
//! ## Progress 归组
//!
//! FsService 不再感知 tab/window。UIService 通过 `ContextId → 活跃 op_id 集合`
//! 归组任务，实现 `move_tab` 的忙检查与强制取消（把 ContextId 语义从 FsService 上移）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tracing::{debug, error, info, warn};

use crate::app::fs_service::{Canceller, Progress};
use crate::app::state::AppStateManager;
use crate::ipc::protocol::{ClipOp, ClipboardState, ContextId, InstanceMessage, ProgressEvent};
use crate::window_bus::WindowBus;

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
    /// ContextId（tab/window）→ 活跃 op_id 集合（忙检查用）
    contexts: Mutex<HashMap<ContextId, HashSet<u64>>>,
    /// op_id → 取消句柄（强制取消用）
    cancels: Mutex<HashMap<u64, Canceller>>,
}

impl UIService {
    pub fn new(mgr: Arc<AppStateManager>) -> Self {
        Self {
            mgr,
            contexts: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
        }
    }

    // -----------------------------------------------------------------------
    // Init
    // -----------------------------------------------------------------------

    /// 前端 mount 时调用，返回初始状态。
    pub fn init(&self) -> InitState {
        let tabs = self.mgr.tabs.lock().unwrap();
        let clipboard = self.clipboard_inner().lock().unwrap();
        InitState {
            tabs: tabs.get_all().to_vec(),
            active_tab_id: tabs.get_all().first().map(|t| t.id),
            clipboard: clipboard.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // Progress 归组（ContextId 语义在此层，FsService 不感知）
    // -----------------------------------------------------------------------

    /// 接管一个 Progress：注册到给定 context，并后台泵进度 → 前端事件。
    ///
    /// TODO: 目前仅在内部维护归组与冲突默认解决；前端事件桥接（job:progress /
    /// job:complete / job:conflict）待前端事件模型接线后补齐。
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
                        // TODO: emit job:conflict，等前端决策回灌 progress.resolve()
                        // 暂时默认自动改名，避免卡死。
                        progress.resolve(conflict_id, crate::ipc::protocol::ConflictResolution::AutoRename);
                    }
                    ProgressEvent::Done { .. } => {
                        // TODO: emit job:complete
                        break;
                    }
                    _ => {
                        // TODO: emit job:progress
                    }
                }
            }
            this.forget_op(&ctx_ids, op_id);
        });
    }

    /// 清理一个已完成 op 的注册。
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

    /// 返回给定 context 下的活跃 op_id 列表。
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

    /// 取消给定 context 下的所有活跃操作。
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

    /// 尝试移动 tab 到目标窗口。如果 tab 有活跃任务，返回 Blocked 让前端弹窗。
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

    /// 强制移动 tab，取消所有关联任务。
    pub async fn move_tab_force(&self, tab_id: u64, target: u64) {
        let ctx = ContextId::Tab(tab_id);
        self.cancel_contexts(&[ctx]).await;
        self.execute_move_tab(tab_id, target);
    }

    fn execute_move_tab(&self, tab_id: u64, target: u64) {
        let target_instance = target >> 32;
        let is_local = target_instance == self.mgr.instance_bus.self_id();

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

    /// 关闭 tab。
    pub fn close_tab(&self, tab_id: u64) {
        let mut tabs = self.mgr.tabs.lock().unwrap();
        if tabs.remove_tab(tab_id).is_some() {
            tabs.save_to_disk();
        }
    }

    /// 创建新 tab。
    pub fn new_tab(&self, path: Option<String>) -> u64 {
        let mut tabs = self.mgr.tabs.lock().unwrap();
        let p = path.unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| "/".into()));
        let tab = tabs.add_tab(p);
        tabs.save_to_disk();
        tab.id
    }

    /// 接收来自其他实例的 tab 转移。
    pub fn receive_transfer_tab(&self, tab_state: crate::ipc::protocol::TabState) {
        let mut tabs = self.mgr.tabs.lock().unwrap();
        tabs.transfer_in(tab_state);
        tabs.save_to_disk();
        info!("received transferred tab");
    }

    // -----------------------------------------------------------------------
    // 剪贴板
    // -----------------------------------------------------------------------

    fn clipboard_inner(&self) -> &Mutex<ClipboardState> {
        // 临时方案：使用实例全局 clipboard（后续改为 AppStateManager 字段）
        static CLIPBOARD: std::sync::LazyLock<Mutex<ClipboardState>> =
            std::sync::LazyLock::new(|| {
                Mutex::new(ClipboardState {
                    operation: None,
                    files: Vec::new(),
                })
            });
        &CLIPBOARD
    }

    pub fn clip_copy(&self, paths: &[String]) {
        let mut cb = self.clipboard_inner().lock().unwrap();
        cb.operation = Some(ClipOp::Copy);
        cb.files = paths.to_vec();
        debug!("clip_copy: {} files", paths.len());
    }

    pub fn clip_cut(&self, paths: &[String]) {
        let mut cb = self.clipboard_inner().lock().unwrap();
        cb.operation = Some(ClipOp::Cut);
        cb.files = paths.to_vec();
        debug!("clip_cut: {} files", paths.len());
    }

    pub fn clipboard_state(&self) -> ClipboardState {
        self.clipboard_inner().lock().unwrap().clone()
    }

    /// 跨实例剪贴板同步。
    pub fn clipboard_sync(&self, state: ClipboardState) {
        let mut cb = self.clipboard_inner().lock().unwrap();
        *cb = state;
        debug!("clipboard_sync: updated from remote");
    }

    // -----------------------------------------------------------------------
    // Instance Bus 接口（供 MeshServer 调用）
    // -----------------------------------------------------------------------

    /// 在当前实例创建新窗口（从 CLI `hnfm /path` 复用已有实例）。
    ///
    /// `paths` 为空时创建空窗口（导航至根目录）。
    ///
    /// 等待 `WindowBus::init` 完成后才返回，确保窗口在可见时已注册到
    /// `AppStateManager`，避免竞态（实例总线消息在窗口注册前丢失）。
    pub async fn open_window(&self, paths: Vec<String>) {
        let label = self.mgr.next_label();
        match super::commands::create_window(self.mgr.app_handle(), &label, &paths) {
            Ok(window) => {
                let bus =
                    WindowBus::init(self.mgr.instance_bus.clone(), window, self.mgr.clone())
                        .await;
                self.mgr.register(label, bus);
            }
            Err(e) => {
                error!("open_window: failed to create window: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// InitState
// ---------------------------------------------------------------------------

/// 前端 `invoke("init")` 的返回类型。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitState {
    pub tabs: Vec<crate::ipc::protocol::TabState>,
    pub active_tab_id: Option<u64>,
    pub clipboard: ClipboardState,
}
