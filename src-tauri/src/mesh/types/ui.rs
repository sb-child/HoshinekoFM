//! UI 层相关类型定义。
//!
//! 包括：Tab 状态、导航、剪贴板、拖放、上下文、仪表盘、设备管理。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 通用文件系统类型
// ---------------------------------------------------------------------------

/// 创建条目的类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Dir,
}

// ---------------------------------------------------------------------------
// Tab 持久化状态
// ---------------------------------------------------------------------------

/// Tab（标签页）的可传输/持久化状态。
///
/// 不含 `UidToken`（运行时的）。接收方通过 `FsService.try_request_uid_token(uid)` 重建。
/// `uid` 由 TabManager 在创建/恢复时赋值，不在 TabState 中传输。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabState {
    pub id: u64,
    pub nav_history: Vec<NavEntry>,
    pub nav_index: usize,
}

// ---------------------------------------------------------------------------
// 上下文 / 剪贴板 / 拖放
// ---------------------------------------------------------------------------

/// 通用任务/资源关联标识。UIService 用它按 tab/window 归组 Progress。
#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ContextId {
    Tab(u64),
    Window(u64),
}

/// 拖放操作类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DragOp {
    Copy,
    Move,
}

/// 剪贴板操作类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ClipOp {
    Copy,
    Cut,
}

/// 剪贴板状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardState {
    pub operation: Option<ClipOp>,
    pub files: Vec<String>,
}

// ---------------------------------------------------------------------------
// UI 导航与设备管理
// ---------------------------------------------------------------------------

/// 导航目标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NavTarget {
    Dashboard,
    Filesystem(String),
}

/// 导航历史中的一个节点。
///
/// `accessible` 不存此处——它是当前 watcher 的 live 属性，不是历史快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavEntry {
    pub target: NavTarget,
    pub selected: Vec<String>,
}

/// Tab 列表事件载荷（`hf:tabs`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabsPayload {
    pub tabs: Vec<TabInfo>,
    pub active_tab_id: u64,
}

/// 轻量 Tab 信息（不含文件内容）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: u64,
    pub title: String,
    pub nav_target: NavTarget,
}

/// 导航状态事件载荷（`hf:nav-state`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavStatePayload {
    pub tab_id: u64,
    pub target: NavTarget,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

/// 面包屑事件载荷（`hf:breadcrumbs`）。
///
/// 后端 watch 线程在导航目标变更或路径段属性变化时推送。
/// 前端应以此为准渲染面包屑，不再自行调用 `getHomeMap` / `getMountMap` / `checkSymlinks`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreadcrumbsPayload {
    pub tab_id: u64,
    pub entries: Vec<BreadcrumbEntry>,
}

/// 面包屑单个路径段。
///
/// 后端通过 `watch_stat` 监听每个祖先目录的属性变化，
/// 实时更新 home / mount / symlink / accessible 等信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreadcrumbEntry {
    /// 段名（如 "Documents"）
    pub name: String,
    /// 截至该段的完整路径（如 "/home/sbchild/Documents"）
    pub path: String,
    /// 是否为符号链接
    pub is_symlink: bool,
    /// 软链接目标路径（仅当 is_symlink=true 时有值）
    pub symlink_target: Option<String>,
    /// 是否为挂载点
    pub is_mount_point: bool,
    /// 挂载源名称（如 "devtmpfs"、"tmpfs"，仅当 is_mount_point=true 时有值）
    pub mount_source: Option<String>,
    /// 是否为某个用户的家目录
    pub is_home: bool,
    /// 家目录对应的用户名（仅当 is_home=true 时有值）
    pub home_username: Option<String>,
    /// 路径是否可访问（权限不足/路径不存在时为 false）
    pub accessible: bool,
}

/// 仪表盘内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardData {
    pub storage: StorageSummary,
    pub common_locations: Vec<CommonLocation>,
}

/// 存储空间汇总。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSummary {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
}

/// 常用位置（按 uid 的 home 解析）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommonLocation {
    pub name: String,
    pub path: String,
    pub exists: bool,
}

/// 设备条目（FsWorker 上报，自主裁决 can_mount/can_unmount）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub name: String,
    pub device_path: String,
    pub mount_point: Option<String>,
    pub fs_type: String,
    pub size_bytes: u64,
    pub available_bytes: u64,
    pub can_mount: bool,
    pub can_unmount: bool,
}

/// 设备变更增量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeviceDelta {
    Reset(Vec<DeviceEntry>),
    Upsert(DeviceEntry),
    Remove(String),
    ConnectionLost,
}
