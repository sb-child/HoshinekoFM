//! 应用全局状态管理器。
//!
//! ## 结构
//!
//! - `AppStateManager` — 通过 Tauri `.manage()` 注入的全局管理器
//!   - `windows: Mutex<HashMap<label, WindowState>>` — 每个窗口的 per-window 状态
//!   - `window_registry: Mutex<HashMap<window_id, WebviewWindow>>` — 本地 emit 查询
//!   - `tabs` / `fs_service` / `instance_bus` — 全局共享
//!
//! ## 使用
//!
//! Commands 通过 `State<'_, Arc<AppStateManager>>` 访问。
//! Window label（如 "w0", "w1"）由 process-local counter 生成，标签与 window_id 分离。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tauri::WebviewWindow;

use crate::instance_bus::InstanceBus;
use crate::window_bus::WindowBus;

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
    /// 实例间通信总线
    pub instance_bus: Arc<InstanceBus>,

    /// process-local label 计数器 → "w0", "w1" ...
    label_counter: AtomicU64,
    /// process-local window_id 计数器（与 instance_id 编码为全局唯一 ID）
    window_id_counter: AtomicU64,
}

/// 每个窗口的私有状态。
pub struct WindowState {
    pub window_id: u64,
    pub window_bus: WindowBus,
}

impl AppStateManager {
    /// 创建全局状态管理器。
    pub fn new(tab_manager: TabManager, instance_bus: Arc<InstanceBus>) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            window_registry: Mutex::new(HashMap::new()),
            tabs: Arc::new(Mutex::new(tab_manager)),
            fs_service: FsService::new(),
            instance_bus,
            label_counter: AtomicU64::new(0),
            window_id_counter: AtomicU64::new(0),
        }
    }

    /// 生成下一个 window label（"w0", "w1", ...）。
    pub fn next_label(&self) -> String {
        format!("w{}", self.label_counter.fetch_add(1, Ordering::Relaxed))
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
        self.windows
            .lock()
            .unwrap()
            .get(label)
            .map(|s| s.window_id)
    }

    /// 抢占全局唯一 window_id（冲突检测）。
    ///
    /// window_id = `(instance_id << 32) | local_counter`。
    /// 确定性编码在数学上保证全局唯一（instance_id 全局唯一），
    /// 此方法额外检查路由表和本地 registry 以防万一。
    pub async fn claim_window_id(&self, bus: &InstanceBus) -> u64 {
        loop {
            let local = self.window_id_counter.fetch_add(1, Ordering::Relaxed);
            let candidate = (bus.self_id() << 32) | local;

            // 冲突检测：全局路由表
            if bus.window_instance(candidate).await.is_some() {
                tracing::warn!(
                    "window_id {} conflict in global route table, retrying",
                    candidate
                );
                continue;
            }

            // 冲突检测：本地 registry（新创建、尚未 broadcast 的窗口）
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

    /// 注册窗口到 AppStateManager。
    pub fn register(&self, label: String, window_bus: WindowBus) {
        let window_id = window_bus.window_id();
        self.windows
            .lock()
            .unwrap()
            .insert(label, WindowState { window_id, window_bus });
    }

    /// 注销窗口。
    pub fn unregister(&self, label: &str) {
        self.windows.lock().unwrap().remove(label);
    }
}
