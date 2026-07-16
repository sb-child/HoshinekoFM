//! FsService 端点：文件系统消息与 Handler trait。
//!
//! `FsServiceMsg` 是主进程向 FsWorker 发送的操作请求。
//! `FsServiceHandler` 是接收 FsService 消息的 trait。
//!
//! Phase 3 仅定义类型占位，Phase 4 将完整实现 FsService 接入 Mesh。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::ui::EntryKind;

// --
// FsServiceMsg
// --

/// FsService 端点能收到的消息（主进程 -> FsWorker 操作请求）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FsServiceMsg {
    WatchDir {
        watch_id: u64,
        path: PathBuf,
    },
    WatchStat {
        watch_id: u64,
        path: PathBuf,
    },
    Refresh {
        watch_id: u64,
    },
    Unwatch {
        watch_id: u64,
    },
    Create {
        op_id: u64,
        path: PathBuf,
        kind: EntryKind,
    },
    Rename {
        op_id: u64,
        path: PathBuf,
        new_name: String,
    },
    Move {
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    },
    Copy {
        op_id: u64,
        items: Vec<(PathBuf, PathBuf)>,
    },
    CancelOp {
        op_id: u64,
    },
    StatVfs {
        path: PathBuf,
    },
    WatchBreadcrumb {
        watch_id: u64,
        path: PathBuf,
    },
}

// --
// FsServiceHandler
// --

/// FsService 消息处理器。
///
/// 实现此 trait 的端点可以接收 FsService 消息。
/// Phase 4 中 FsWorker 将实现此 trait，通过 Mesh 注册接收操作请求。
pub trait FsServiceHandler: Send + Sync + 'static {
    fn on_watch_dir(&self, watch_id: u64, path: PathBuf);
    fn on_watch_stat(&self, watch_id: u64, path: PathBuf);
    fn on_refresh(&self, watch_id: u64);
    fn on_unwatch(&self, watch_id: u64);
    fn on_create(&self, op_id: u64, path: PathBuf, kind: EntryKind);
    fn on_rename(&self, op_id: u64, path: PathBuf, new_name: String);
    fn on_move(&self, op_id: u64, items: Vec<(PathBuf, PathBuf)>);
    fn on_copy(&self, op_id: u64, items: Vec<(PathBuf, PathBuf)>);
    fn on_cancel_op(&self, op_id: u64);
    fn on_stat_vfs(&self, path: PathBuf);
    fn on_watch_breadcrumb(&self, watch_id: u64, path: PathBuf);
}

// --
// dispatch -- 由宏生成
// --

crate::endpoint_dispatch!(
    /// 分发 `FsServiceMsg` 到 `FsServiceHandler`。
    FsServiceMsg -> FsServiceHandler,
    dispatch: dispatch_fs_msg,
    WatchDir { watch_id, path } => on_watch_dir,
    WatchStat { watch_id, path } => on_watch_stat,
    Refresh { watch_id } => on_refresh,
    Unwatch { watch_id } => on_unwatch,
    Create { op_id, path, kind } => on_create,
    Rename { op_id, path, new_name } => on_rename,
    Move { op_id, items } => on_move,
    Copy { op_id, items } => on_copy,
    CancelOp { op_id } => on_cancel_op,
    StatVfs { path } => on_stat_vfs,
    WatchBreadcrumb { watch_id, path } => on_watch_breadcrumb,
);
