//! RPC 服务定义 (tarpc service traits) 和共享数据类型。
//!
//! ## 传输层
//!
//! | 场景        | transport               | 方向                         |
//! | ----------- | ----------------------- | ---------------------------- |
//! | 实例→实例   | named UDS + tarpc       | client 连接 → 调用 server    |
//! | 实例→Worker | 匿名 socketpair + tarpc | 主进程=client, Worker=server |

use std::{
    path::PathBuf,
    time::SystemTime,
};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 共享数据类型
// ---------------------------------------------------------------------------

/// 文件系统条目信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// 相对于列表目录的路径
    pub name: String,
    /// 绝对路径
    pub path: PathBuf,
    /// 文件大小 (字节)；目录为 0
    pub size: u64,
    /// 修改时间
    pub modified: SystemTime,
    /// 是否为目录
    pub is_directory: bool,
    /// 是否为符号链接
    pub is_symlink: bool,
    /// MIME 类型 (文件) 或 "inode/directory" (目录)
    pub mime_type: String,
    /// Unix 权限模式 (e.g. 0o644)
    pub permissions: u32,
    /// 所有者 UID
    pub owner_uid: u32,
    /// 组 GID
    pub owner_gid: u32,
}

/// 单个文件/目录的元信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    pub size: u64,
    pub modified: SystemTime,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub permissions: u32,
    pub owner_uid: u32,
    pub owner_gid: u32,
}

/// Tab（标签页）的完整可传输状态。
///
/// 用于跨实例移动 tab 以及持久化到磁盘。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabState {
    /// 实例内唯一 ID
    pub id: u64,
    /// 当前浏览的路径
    pub path: PathBuf,
    /// 前端自定义视图状态 (JSON)
    pub view_state: String,
}

/// 目录监视事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DirChangeEvent {
    /// 文件 or 目录已创建
    Created(PathBuf),
    /// 文件 or 目录已删除
    Removed(PathBuf),
    /// 文件内容或元数据已变更
    Modified(PathBuf),
    /// 条目已重命名 (旧名, 新名)
    Renamed(PathBuf, PathBuf),
}

// ---------------------------------------------------------------------------
// 实例间 RPC: InstanceService
// ---------------------------------------------------------------------------

/// 实例间 RPC 服务。
///
/// 主实例作为 server，其他实例（或 CLI `--attach`）作为 client。
#[tarpc::service]
pub trait InstanceService {
    /// 请求在当前实例中打开新窗口（每个 path 作为一个 tab）。
    async fn open_tabs(paths: Vec<String>);

    /// 将一个 tab 从其他实例移动到当前实例。
    async fn transfer_tab(tab: TabState);

    /// 健康检查。
    async fn ping() -> bool;
}

// ---------------------------------------------------------------------------
// Worker RPC: FsWorkerService
// ---------------------------------------------------------------------------

/// FS Worker 子进程提供的 RPC 服务。
///
/// 主进程为 client，Worker 为 server。
/// 传输层为匿名 socketpair + tarpc `serde-transport`。
#[tarpc::service]
pub trait FsWorkerService {
    /// 列出目录内容。
    ///
    /// 返回 `(entries, resolved_path)`，resolved_path 是经过符号链接解析后的实际路径。
    async fn list_dir(path: String) -> (Vec<FileEntry>, String);

    /// 获取单个路径的元信息。
    async fn stat(path: String) -> FileStat;

    /// 读取文件内容 (小文件一次性传输)。
    async fn read_file(path: String) -> Vec<u8>;

    /// 复制文件或目录。
    async fn copy_item(src: String, dest: String) -> ();

    /// 移动文件或目录。
    async fn move_item(src: String, dest: String) -> ();

    /// 移动到回收站。
    async fn trash_item(path: String) -> ();

    /// 重命名。
    async fn rename_item(path: String, new_name: String) -> ();

    /// 创建目录。
    async fn mkdir(path: String) -> ();

    /// 开始监听目录变化 (每个 tab 一个监听)。
    ///
    /// 变化事件通过 tarpc 服务端推送发送给客户端。
    async fn watch_dir(tab_id: u64, path: String) -> ();

    /// 停止监听目录变化。
    async fn unwatch_dir(tab_id: u64, path: String) -> ();
}
