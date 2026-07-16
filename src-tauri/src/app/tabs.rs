//! Tab（标签页）管理器。
//!
//! 管理所有打开的标签页，支持增删改查、跨实例移动和持久化。
//!
//! ## 持久化
//!
//! 存储在 `~/.local/share/hnfm/tabs.json`，格式为 `Vec<TabState>` JSON。
//!
//! ## 跨实例传输
//!
//! 只传输路径 + 视图状态，目标实例自行 `list_dir` 获取目录内容。

use std::{fs, io, path::PathBuf};

use tracing::{debug, error, info, warn};

/// Re-export 以便统一引用。
pub use crate::mesh::types::ui::TabState;

/// 持久化文件路径：`~/.local/share/hnfm/tabs.json`
fn tabs_file_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            PathBuf::from(home).join(".local/share")
        });
    base.join("hnfm/tabs.json")
}

use crate::mesh::types::ui::NavEntry;
use crate::mesh::types::ui::NavTarget;

/// Tab 管理器。
pub struct TabManager {
    /// 所有 tab
    tabs: Vec<TabState>,
    /// 自增 ID 计数器
    next_id: u64,
}

impl TabManager {
    /// 创建空的管理器。
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            next_id: 1,
        }
    }

    /// 从磁盘加载保存的 tab 状态。
    ///
    /// 返回加载后的 TabManager。如果文件不存在或损坏，返回空的管理器。
    pub fn load_from_disk() -> Self {
        let path = tabs_file_path();
        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<Vec<TabState>>(&content) {
                Ok(tabs) => {
                    let max_id = tabs.iter().map(|t| t.id).max().unwrap_or(0);
                    info!("loaded {} tabs from {path:?}", tabs.len());
                    Self {
                        tabs,
                        next_id: max_id + 1,
                    }
                }
                Err(e) => {
                    warn!("failed to parse tabs.json: {e}, starting fresh");
                    Self::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                debug!("no saved tabs found, starting fresh");
                Self::new()
            }
            Err(e) => {
                warn!("failed to read tabs.json: {e}, starting fresh");
                Self::new()
            }
        }
    }

    /// 添加一个 tab，以初始路径开始导航历史。
    pub fn add_tab(&mut self, path: String) -> TabState {
        let id = self.next_id;
        self.next_id += 1;

        let entry = NavEntry {
            target: NavTarget::Filesystem(path),
            selected: Vec::new(),
        };
        let tab = TabState {
            id,
            nav_history: vec![entry],
            nav_index: 0,
        };
        self.tabs.push(tab.clone());
        debug!("added tab id={id}, total={}", self.tabs.len());
        tab
    }

    /// 移除一个 tab。
    pub fn remove_tab(&mut self, id: u64) -> Option<TabState> {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            let removed = self.tabs.remove(pos);
            debug!("removed tab id={id}, total={}", self.tabs.len());
            Some(removed)
        } else {
            warn!("attempted to remove non-existent tab id={id}");
            None
        }
    }

    /// 获取所有 tab。
    pub fn get_all(&self) -> &[TabState] {
        &self.tabs
    }

    /// 获取 tab 总数。
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// 按 ID 查找 tab。
    pub fn get(&self, id: u64) -> Option<&TabState> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// 跨实例传输 tab。
    ///
    /// 从当前实例移除并返回 tab 状态，用于传输到另一个实例。
    pub fn transfer_out(&mut self, id: u64) -> Option<TabState> {
        self.remove_tab(id)
    }

    /// 接收来自其他实例的 tab。
    pub fn transfer_in(&mut self, mut tab: TabState) {
        // 重新分配 ID（避免冲突）
        tab.id = self.next_id;
        self.next_id += 1;
        self.tabs.push(tab);
    }

    /// 持久化当前 tab 状态到磁盘。
    pub fn save_to_disk(&self) {
        let path = tabs_file_path();

        // 确保目录存在
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                error!("failed to create tabs dir {parent:?}: {e}");
                return;
            }
        }

        match serde_json::to_string_pretty(&self.tabs) {
            Ok(content) => {
                if let Err(e) = fs::write(&path, &content) {
                    error!("failed to write tabs to {path:?}: {e}");
                } else {
                    debug!("saved {} tabs to {path:?}", self.tabs.len());
                }
            }
            Err(e) => {
                error!("failed to serialize tabs: {e}");
            }
        }
    }

    /// 使用给定 tab 列表替换当前 tab（用于加载跨实例的 tab 重组）。
    #[allow(dead_code)]
    pub fn replace_all(&mut self, tabs: Vec<TabState>) {
        let max_id = tabs.iter().map(|t| t.id).max().unwrap_or(0);
        self.tabs = tabs;
        self.next_id = max_id + 1;
    }
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}
