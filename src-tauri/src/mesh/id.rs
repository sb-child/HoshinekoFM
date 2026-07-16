//! Mesh 寻址标识符。

use serde::{Deserialize, Serialize};

/// 窗口标识（instance_id + 本地自增 window_id）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowId {
    pub instance: u64,
    pub window: u64,
}

/// 服务标识（instance 内唯一）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceId {
    pub instance: u64,
}

/// Mesh 节点统一标识符。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MeshId {
    /// 窗口
    Window(WindowId),
    /// 本地服务（如 FsService）
    Service(ServiceId),
}

impl MeshId {
    pub fn instance(&self) -> u64 {
        match self {
            MeshId::Window(w) => w.instance,
            MeshId::Service(s) => s.instance,
        }
    }
}
