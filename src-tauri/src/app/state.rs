//! 应用全局状态管理器。
//!
//! ## 结构
//!
//! - `AppStateManager` — 通过 Tauri `.manage()` 注入的全局管理器
//!   - `windows: Mutex<HashMap<label, WindowState>>` — 每个窗口的 per-window 状态
//!   - `window_registry: Mutex<HashMap<window_id, WebviewWindow>>` — 本地 emit 查询
//!   - `tabs` / `fs_service` / `mesh` — 全局共享
//!
//! ## 使用
//!
//! Commands 通过 `State<'_, Arc<AppStateManager>>` 访问。
//! Window label（如 "w0", "w1"）由 process-local counter 生成，标签与 window_id 分离。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tauri::WebviewWindow;
use tracing::warn;

use crate::mesh::types::ui::DragOp;
use crate::mesh::{Mesh, WindowProxy};

use super::fs_service::FsService;
use super::tabs::TabManager;

/// 应用全局状态管理器。
pub struct AppStateManager {
    /// label → per-window 状态
    pub windows: Mutex<HashMap<String, WindowState>>,
    /// window_id → WebviewWindow（本地消息投递用）
    pub window_registry: Mutex<HashMap<u64, WebviewWindow>>,
    /// 全局 Tab 管理器
    pub tabs: Arc<Mutex<TabManager>>,
    /// 文件系统调度服务（内部含 Worker 进程池 + reaper）
    pub fs_service: FsService,
    /// Mesh 通信层
    pub mesh: Arc<Mesh>,

    /// Tauri AppHandle（setup 阶段注入，用于跨服务创建窗口）
    app_handle: OnceLock<tauri::AppHandle>,

    /// process-local label 计数器 → "w0", "w1" ...
    label_counter: AtomicU64,
    /// process-local window_id 计数器（与 instance_id 编码为全局唯一 ID）
    window_id_counter: AtomicU64,
}

/// 每个窗口的私有状态。
pub struct WindowState {
    pub window_id: u64,
    pub window_proxy: WindowProxy,
}

impl AppStateManager {
    /// 创建全局状态管理器。
    pub fn new(tab_manager: TabManager, mesh: Arc<Mesh>) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            window_registry: Mutex::new(HashMap::new()),
            tabs: Arc::new(Mutex::new(tab_manager)),
            fs_service: FsService::new(),
            mesh,
            app_handle: OnceLock::new(),
            label_counter: AtomicU64::new(0),
            window_id_counter: AtomicU64::new(0),
        }
    }

    /// 生成下一个 window label（"w0", "w1", ...）。
    pub fn next_label(&self) -> String {
        format!("w{}", self.label_counter.fetch_add(1, Ordering::Relaxed))
    }

    /// 注入 Tauri AppHandle（`setup` 闭包中调用一次）。
    pub fn set_app_handle(&self, h: tauri::AppHandle) {
        if self.app_handle.set(h).is_err() {
            warn!("set_app_handle called more than once; ignoring");
        }
    }

    /// 获取已注入的 Tauri AppHandle（在 `setup` 之后可用）。
    pub fn app_handle(&self) -> &tauri::AppHandle {
        self.app_handle
            .get()
            .expect("AppHandle not yet set; call set_app_handle first")
    }

    /// 当前窗口数量（按 label 注册表计）。
    pub fn window_count(&self) -> usize {
        self.windows.lock().unwrap().len()
    }

    /// 当前 registry 数量。
    pub fn registry_count(&self) -> usize {
        self.window_registry.lock().unwrap().len()
    }

    /// 根据 window label 获取 window_id。
    pub fn window_id_by_label(&self, label: &str) -> Option<u64> {
        self.windows.lock().unwrap().get(label).map(|s| s.window_id)
    }

    /// 抢占全局唯一 window_id（冲突检测）。
    ///
    /// window_id = `(instance_id << 32) | local_counter`。
    pub async fn claim_window_id(&self) -> u64 {
        let instance_bus = self.mesh.instance_bus();
        loop {
            let local = self.window_id_counter.fetch_add(1, Ordering::Relaxed);
            let candidate = (instance_bus.self_id() << 32) | local;

            if instance_bus.window_instance(candidate).is_some() {
                tracing::warn!(
                    "window_id {} conflict in global route table, retrying",
                    candidate
                );
                continue;
            }

            {
                let reg = self.window_registry.lock().unwrap();
                if reg.contains_key(&candidate) {
                    tracing::warn!(
                        "window_id {} conflict in local registry, retrying",
                        candidate
                    );
                    continue;
                }
            }

            return candidate;
        }
    }

    /// 统一窗口注册入口：抢占 window_id + 通过 Mesh 创建 WindowProxy。
    pub async fn register_window(
        self: &Arc<Self>,
        window: tauri::WebviewWindow,
        label: String,
    ) -> WindowProxy {
        let window_id = self.claim_window_id().await;

        let handler: Arc<dyn crate::mesh::WindowHandler> = Arc::new(NoopWindowHandler);
        let proxy = self.mesh.create_window(handler);

        self.window_registry
            .lock()
            .unwrap()
            .insert(window_id, window);

        self.windows.lock().unwrap().insert(
            label,
            WindowState {
                window_id,
                window_proxy: proxy.clone(),
            },
        );
        proxy
    }

    /// 注销窗口。
    pub fn unregister(&self, label: &str) {
        self.windows.lock().unwrap().remove(label);
    }
}

/// No-op `WindowHandler`。
struct NoopWindowHandler;

impl crate::mesh::WindowHandler for NoopWindowHandler {
    fn on_dnd_active(&self, _session_id: u64, _files: Vec<String>, _op: DragOp) {}
    fn on_dnd_completed(&self, _session_id: u64) {}
    fn on_tab_attached(&self, _tab: crate::mesh::types::ui::TabState) {}
}
