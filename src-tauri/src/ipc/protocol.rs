//! RPC 服务定义 (tarpc service traits) 和共享数据类型。
//!
//! ## 传输层
//!
//! | 场景            | transport               | 方向                          |
//! | --------------- | ----------------------- | ----------------------------- |
//! | 实例→实例       | named UDS + tarpc       | 全连接 Mesh，每个实例既 server 又 client |
//! | 实例→Worker     | 匿名 socketpair + tarpc | 主进程=client, Worker=server  |
//! | Worker→实例回调 | 匿名 socketpair + tarpc | Worker=client, 主进程=server  |
//!
//! ## 设计原则
//!
//! - **无 list_dir / 无 stat**：快照会变，一切皆订阅（`watch_dir` / `watch_stat`）。
//! - **所有变更操作都假设可能很慢**（软盘/NFS/坏块）→ 一律立刻返回，进度通过反向回调汇报。
//! - `File` 的 mime / 缩略图是**渐进式**读取的，先出基础 stat，算好后再补发 `Upsert`。

use std::{path::PathBuf, time::SystemTime};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 共享数据类型
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

/// 创建条目的类型。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EntryKind {
    /// 空文件
    File,
    /// 目录
    Dir,
}

/// Tab（标签页）的完整可传输状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabState {
    pub id: u64,
    pub path: PathBuf,
    pub view_state: String,
}

// ---------------------------------------------------------------------------
// Watcher 增量事件
// ---------------------------------------------------------------------------

/// 目录/文件监视的增量事件。
///
/// - 首帧（或 `refresh()` 后）为 `Reset`，给出全量文件。
/// - 之后为增量：新增/修改/元数据补全均为 `Upsert`，删除为 `Remove`，重命名为 `Rename`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WatchDelta {
    /// 全量快照（首帧 / refresh 后）
    Reset(Vec<File>),
    /// 新增或更新一个文件（也用于渐进式补发 mime / 缩略图）
    Upsert(File),
    /// 删除一个文件
    Remove(PathBuf),
    /// 重命名
    Rename { from: PathBuf, to: PathBuf },
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
    Tick { done: u64, total: u64, current: PathBuf },
    /// 抛出一个冲突，等待上层通过 `Progress::resolve` 决策
    Conflict { conflict_id: u64, item: ConflictItem },
    /// 操作结束
    Done {
        succeeded: u64,
        failed: u64,
        cancelled: bool,
    },
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

/// 窗口间消息 — WindowBus 专用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WindowMessage {
    /// DnD 拖拽开始（广播给同实例所有窗口 + 跨实例转发）
    DndSessionActive {
        session_id: u64,
        files: Vec<String>,
        operation: DragOp,
    },
    /// DnD 放置完成（发回源窗口）
    DndSessionCompleted { session_id: u64 },
    /// Tab 已附加到目标窗口
    TabAttached { tab: TabState },
}

/// 实例间消息 — InstanceBus 专用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstanceMessage {
    /// 窗口注册广播
    WindowRegistered { window_id: u64, instance_id: u64 },
    /// 窗口注销广播
    WindowUnregistered { window_id: u64 },
    /// 剪贴板同步广播
    ClipboardSync { state: ClipboardState },
    /// Tab 转移到指定实例
    TransferTab { tab: TabState, to_instance: u64 },
    /// 转发 WindowMessage 到指定窗口
    ForwardToWindow { window_id: u64, msg: WindowMessage },
}

// ---------------------------------------------------------------------------
// 实例间 RPC: InstanceService
// ---------------------------------------------------------------------------

/// 实例间 RPC 服务。
///
/// Mesh 全连接：每个实例既是 server（监听连接）又是 client（连接其他实例）。
/// 所有方法都是 P2P 直连，不经过中转。
#[tarpc::service]
pub trait InstanceService {
    /// 请求在当前实例打开新窗口（CLI `hnfm /path` 复用已有实例时使用）。
    ///
    /// `paths` 为空时创建空窗口（导航至根目录）。
    async fn open_window(paths: Vec<String>);

    /// 将一个 tab 从其他实例转移到当前实例。
    async fn transfer_tab(tab: TabState);

    /// 窗口注册（fan-out 广播给所有实例）。
    async fn window_register(window_id: u64, instance_id: u64);

    /// 窗口注销（fan-out 广播给所有实例）。
    async fn window_unregister(window_id: u64);

    /// 转发 WindowMessage 到当前实例的指定窗口。
    async fn forward(window_id: u64, msg: WindowMessage);

    /// 剪贴板同步（fan-out 广播给所有实例）。
    async fn clipboard_sync(state: ClipboardState);

    /// 存活检查。
    async fn ping() -> bool;
}

// ---------------------------------------------------------------------------
// Worker RPC: FsWorkerService（主进程 → Worker）
// ---------------------------------------------------------------------------

/// FS Worker 子进程提供的 RPC 服务。
///
/// 主进程为 client，Worker 为 server。传输层为匿名 socketpair + tarpc。
///
/// 所有方法都**立刻返回**（仅表示派发是否被接受），真正的结果 / 进度 / 增量
/// 通过反向的 `AppCallbackService` 推送。`watch_id` / `op_id` 由主进程分配，
/// 用于把反向回调路由回对应的 `Watcher` / `Progress`。
#[tarpc::service]
pub trait FsWorkerService {
    /// 开始监视目录。首帧全量 + 后续增量通过 `watch_delta` 推送。
    async fn watch_dir(watch_id: u64, path: String) -> Result<(), String>;
    /// 开始监视单个文件/目录的属性（面包屑用）。
    async fn watch_stat(watch_id: u64, path: String) -> Result<(), String>;
    /// 立刻触发一次全量刷新（重新推 `Reset`）。
    async fn refresh(watch_id: u64);
    /// 停止监视并释放资源。
    async fn unwatch(watch_id: u64);

    /// 创建文件或目录。
    async fn run_create(op_id: u64, path: String, kind: EntryKind) -> Result<(), String>;
    /// 重命名。
    async fn run_rename(op_id: u64, path: String, new_name: String) -> Result<(), String>;
    /// 批量移动（同 UID）。`items` 为 `(src, dst)` 列表。
    async fn run_move(op_id: u64, items: Vec<(String, String)>) -> Result<(), String>;
    /// 批量复制（同 UID）。
    async fn run_copy(op_id: u64, items: Vec<(String, String)>) -> Result<(), String>;
    /// 取消一个进行中的批处理操作。
    async fn cancel_op(op_id: u64);
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
