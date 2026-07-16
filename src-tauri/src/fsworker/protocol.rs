//! FsWorker RPC 服务与共享数据类型。
//!
//! 包括：文件条目、Watcher 增量事件、批处理进度/冲突、FsWorker RPC 接口。
//!
//! ## 传输层
//!
//! - 实例→Worker：匿名 socketpair + tarpc（主进程=client, Worker=server）
//! - Worker→实例回调：匿名 socketpair + tarpc（Worker=client, 主进程=server）

use std::{path::PathBuf, time::SystemTime};

use serde::{Deserialize, Serialize};

pub use crate::mesh::types::ui::EntryKind;

// ---------------------------------------------------------------------------
// 文件系统条目
// ---------------------------------------------------------------------------

/// 文件系统条目信息。
///
/// mime / thumbnail 是**渐进式**字段：初次快照时可能为 `None`，
/// Worker 后台算好后通过 `WatchDelta::Upsert` 补发。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    /// 文件名（不含路径）
    pub name: String,
    /// 完整路径
    pub path: PathBuf,
    /// 大小（字节），目录为 0
    pub size: u64,
    /// 修改时间
    pub modified: SystemTime,
    /// 是否为目录
    pub is_directory: bool,
    /// 是否为符号链接
    pub is_symlink: bool,
    /// Unix 权限位
    pub permissions: u32,
    /// 属主 UID
    pub owner_uid: u32,
    /// 属主 GID
    pub owner_gid: u32,
    /// MIME 类型（渐进式，初次可能为 None）
    pub mime: Option<String>,
    /// 缩略图 PNG 字节（渐进式，初次为 None；仅图片类）
    pub thumbnail: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Watcher 增量事件
// ---------------------------------------------------------------------------

/// 目录/文件监视的增量事件。
///
/// - 首帧（或 `refresh()` 后）为 `Reset`，给出全量文件。
/// - 之后为增量：新增/修改/元数据补全均为 `Upsert`，删除为 `Remove`，重命名为 `Rename`。
/// - `Inaccessible` 表示目标目录暂不可访问，Worker 正在尝试上级目录恢复。
/// - `Recovering` 表示级联恢复中，已回退到某层祖先等待 target 可用。
/// - `FatalError` 表示连 `/` 都无法访问，watcher 彻底失效 (Dead)。
/// - `ConnectionLost` 表示 Worker 连接断开，上层应暂停依赖此 watcher 的 UI。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WatchDelta {
    /// 全量快照（首帧 / refresh 后）
    Reset(Vec<File>),
    /// 新增或更新一个文件（也用于渐进式补发 mime / 缩略图）
    Upsert(File),
    /// 批量新增或更新（用于目录初始流式加载）
    UpsertBatch(Vec<File>),
    /// 删除一个文件
    Remove(PathBuf),
    /// 重命名
    Rename { from: PathBuf, to: PathBuf },
    /// 目标暂不可访问，Worker 已回退到上级目录等待恢复
    Inaccessible {
        /// 目标 canonical path
        path: PathBuf,
        /// 当前回退到了哪层祖先
        ancestor: PathBuf,
        /// 回退层级 (0=target 本身, 1=parent, …)
        level: u32,
        /// 失败原因
        reason: String,
    },
    /// 级联恢复中：等待 target 可用
    Recovering {
        /// 目标 canonical path
        path: PathBuf,
        /// 当前挂载 inotify 的祖先路径
        ancestor: PathBuf,
        /// 回退层级
        level: u32,
    },
    /// 连 / 都无法访问 — watcher 已彻底失效
    FatalError {
        /// 目标路径
        path: PathBuf,
        /// 失败原因
        reason: String,
    },
    /// Worker 连接断开
    ConnectionLost {
        watch_id: u64,
        reason: String,
        reconnecting: bool,
    },
    /// 面包屑路径段信息（home/mount 判断结果）。
    ///
    /// FsWorker 监听 /proc/mounts 变化，当挂载状态改变时重新推送。
    /// 首帧或 refresh 后也会推送。
    BreadcrumbSegments(Vec<BreadcrumbSegment>),
}

// ---------------------------------------------------------------------------
// 批处理进度 / 冲突
// ---------------------------------------------------------------------------

/// 单个条目的处理结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ItemStatus {
    /// 成功
    Ok,
    /// 成功但目标被自动重命名
    Renamed(PathBuf),
    /// 跳过
    Skipped,
    /// 失败及原因
    Failed(String),
}

/// 一个待解决的重名冲突。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictItem {
    /// 源路径
    pub src: PathBuf,
    /// 目标路径（已存在）
    pub dst: PathBuf,
}

/// 冲突解决方式（上层 → Worker）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// 跳过该项
    Skip,
    /// 覆盖目标
    Overwrite,
    /// 自动改名（新名由 Worker 内部决定，如 `foo (1).txt`）
    AutoRename,
    /// 手动指定新名
    Rename(String),
    /// 取消整个操作
    CancelAll,
}

/// 批处理进度事件（Worker → 上层，通过反向回调）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressEvent {
    /// 操作开始，给出总条目数
    Started { total: u64 },
    /// 单个条目完成
    Item {
        src: PathBuf,
        dst: PathBuf,
        status: ItemStatus,
    },
    /// 进度心跳
    Tick {
        done: u64,
        total: u64,
        current: PathBuf,
    },
    /// 抛出一个冲突，等待上层通过 `Progress::resolve` 决策
    Conflict {
        conflict_id: u64,
        item: ConflictItem,
    },
    /// 操作结束
    Done {
        succeeded: u64,
        failed: u64,
        cancelled: bool,
    },
    /// Worker 连接断开
    ConnectionLost {
        op_id: u64,
        reason: String,
        reconnecting: bool,
    },
}

// ---------------------------------------------------------------------------
// Worker RPC: FsWorkerService（主进程 → Worker）
// ---------------------------------------------------------------------------

/// 面包屑路径段信息（FsWorker → 主进程）。
///
/// FsWorker 内部读取 /etc/passwd 和 /proc/mounts，对每个祖先目录段
/// 返回 home / mount 判断结果。主进程无需直接访问这些系统文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreadcrumbSegment {
    /// 段名（如 "Documents"）
    pub name: String,
    /// 截至该段的完整路径（如 "/home/sbchild/Documents"）
    pub path: String,
    /// 是否为某个用户的家目录
    pub is_home: bool,
    /// 家目录对应的用户名（仅当 is_home=true 时有值）
    pub home_username: Option<String>,
    /// 是否为挂载点
    pub is_mount_point: bool,
    /// 挂载源名称（如 "devtmpfs"、"tmpfs"，仅当 is_mount_point=true 时有值）
    pub mount_source: Option<String>,
}

/// FS Worker 子进程提供的 RPC 服务。
///
/// 主进程为 client，Worker 为 server。传输层为匿名 socketpair + tarpc。
///
/// 所有方法都**立刻返回**（仅表示派发是否被接受），真正的结果 / 进度 / 增量
/// 通过反向的 `AppCallbackService` 推送。`watch_id` / `op_id` 由主进程分配，
/// 用于把反向回调路由回对应的 `Watcher` / `Progress`。
#[tarpc::service]
pub trait FsWorkerService {
    /// 存活检查。
    async fn ping() -> bool;
    /// 开始监视目录。首帧全量 + 后续增量通过 `watch_delta` 推送。
    async fn watch_dir(watch_id: u64, path: PathBuf) -> Result<(), String>;
    /// 开始监视单个文件/目录的属性（面包屑用）。
    async fn watch_stat(watch_id: u64, path: PathBuf) -> Result<(), String>;
    /// 立刻触发一次全量刷新（重新推 `Reset`）。
    async fn refresh(watch_id: u64);
    /// 停止监视并释放资源。
    async fn unwatch(watch_id: u64);

    /// 创建文件或目录。
    async fn run_create(op_id: u64, path: PathBuf, kind: EntryKind) -> Result<(), String>;
    /// 重命名。
    async fn run_rename(op_id: u64, path: PathBuf, new_name: String) -> Result<(), String>;
    /// 批量移动（同 UID）。`items` 为 `(src, dst)` 列表。
    async fn run_move(op_id: u64, items: Vec<(PathBuf, PathBuf)>) -> Result<(), String>;
    /// 批量复制（同 UID）。
    async fn run_copy(op_id: u64, items: Vec<(PathBuf, PathBuf)>) -> Result<(), String>;
    /// 取消一个进行中的批处理操作。
    async fn cancel_op(op_id: u64);
    /// 文件系统空间查询。返回 `(total_bytes, free_bytes)`。
    async fn stat_vfs(path: PathBuf) -> Result<(u64, u64), String>;
    /// 监听面包屑路径段信息。首帧立即推送，后续 /proc/mounts 变化时重推。
    async fn watch_breadcrumb(watch_id: u64, path: PathBuf) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// 反向回调 RPC: AppCallbackService（Worker → 主进程）
// ---------------------------------------------------------------------------

/// Worker 回调主进程的 RPC 服务。
///
/// Worker 为 client，主进程为 server。通过第二条匿名 socketpair 建立。
/// 用于把 watcher 增量、批处理进度、冲突询问推回主进程。
#[tarpc::service]
pub trait AppCallbackService {
    /// 推送 watcher 增量。
    async fn watch_delta(watch_id: u64, delta: WatchDelta);
    /// 推送批处理进度。
    async fn progress(op_id: u64, ev: ProgressEvent);
    /// 询问冲突如何解决（阻塞式，直到上层给出决策）。
    async fn ask_conflict(op_id: u64, conflict_id: u64, item: ConflictItem) -> ConflictResolution;
}
