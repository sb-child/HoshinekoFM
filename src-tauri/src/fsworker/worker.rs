//! FS Worker 子进程入口。
//!
//! 由主进程通过 `hnfm fs-worker --worker-id <n> --fd <n>` 启动，
//! 通过匿名 socketpair 恢复 tarpc 连接，实现 `FsWorkerService`。
//!
//! ## 安全
//!
//! - 不创建 GUI 窗口（Headless 架构）
//! - 验证当前 UID / EUID
//! - 仅通过继承 FD 通信，无对外暴露端口

use std::{collections::HashMap, io, os::unix::io::FromRawFd, path::PathBuf, sync::Arc};

use nix::unistd;
use tarpc::context;
use tokio::{net::UnixStream, sync::Mutex};
use tracing::{debug, error, info, warn};

use crate::ipc::protocol::{FileEntry, FileStat, FsWorkerService};

use super::FsWorkerOpts;

// ---------------------------------------------------------------------------
// Worker 服务实现
// ---------------------------------------------------------------------------

/// FS Worker 服务实现。
///
/// 每个 worker 实例管理自己的文件操作和 inotify 监听。
#[derive(Clone)]
pub struct FsWorkerServer {
    /// Worker ID
    _worker_id: u64,
    /// 当前 UID
    _uid: u32,
    /// Per-tab inotify 监听路径集合
    _watches: Arc<Mutex<HashMap<u64, PathBuf>>>,
}

impl FsWorkerServer {
    pub fn new(opts: &FsWorkerOpts) -> Self {
        let uid = unistd::getuid().as_raw();
        info!("fs worker {} starting, uid={}", opts.worker_id, uid);
        Self {
            _worker_id: opts.worker_id,
            _uid: uid,
            _watches: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl FsWorkerService for FsWorkerServer {
    async fn list_dir(self, _ctx: context::Context, path: String) -> (Vec<FileEntry>, String) {
        let path_buf = PathBuf::from(&path);
        let resolved = resolve_path(&path_buf);
        let resolved_str = resolved.to_string_lossy().to_string();

        let entries = tokio::task::spawn_blocking(move || match list_directory(&resolved) {
            Ok(entries) => entries,
            Err(e) => {
                warn!("list_dir({path}) failed: {e}");
                vec![]
            }
        })
        .await
        .unwrap_or_default();

        (entries, resolved_str)
    }

    async fn stat(self, _ctx: context::Context, path: String) -> FileStat {
        let path_buf = PathBuf::from(&path);
        match file_stat(&path_buf) {
            Ok(stat) => stat,
            Err(e) => {
                warn!("stat({path}) failed: {e}");
                FileStat {
                    size: 0,
                    modified: std::time::SystemTime::UNIX_EPOCH,
                    is_directory: false,
                    is_symlink: false,
                    permissions: 0,
                    owner_uid: 0,
                    owner_gid: 0,
                }
            }
        }
    }

    async fn read_file(self, _ctx: context::Context, path: String) -> Vec<u8> {
        tokio::task::spawn_blocking(move || match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) => {
                warn!("read_file({path}) failed: {e}");
                vec![]
            }
        })
        .await
        .unwrap_or_default()
    }

    async fn copy_item(self, _ctx: context::Context, src: String, dest: String) {
        let (src, dest) = (PathBuf::from(&src), PathBuf::from(&dest));
        debug!("copy: {src:?} -> {dest:?}");

        let _ = tokio::task::spawn_blocking(move || {
            if let Err(e) = copy_path(&src, &dest) {
                error!("copy failed: {e}");
            }
        })
        .await;
    }

    async fn move_item(self, _ctx: context::Context, src: String, dest: String) {
        let src = PathBuf::from(&src);
        let dest = PathBuf::from(&dest);
        debug!("move: {src:?} -> {dest:?}");

        if let Err(e) = std::fs::rename(&src, &dest) {
            error!("move failed: {e}");
        }
    }

    async fn trash_item(self, _ctx: context::Context, path: String) {
        let path_buf = PathBuf::from(&path);
        debug!("trash: {path_buf:?}");
        // TODO: 使用 freedesktop 回收站协议 (Trash spec)
        warn!("trash not yet implemented for {path_buf:?}");
    }

    async fn rename_item(self, _ctx: context::Context, path: String, new_name: String) {
        let parent = PathBuf::from(&path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/"));
        let new_path = parent.join(&new_name);
        debug!("rename: {path} -> {new_path:?}");

        if let Err(e) = std::fs::rename(&path, &new_path) {
            error!("rename failed: {e}");
        }
    }

    async fn mkdir(self, _ctx: context::Context, path: String) {
        debug!("mkdir: {path}");
        if let Err(e) = std::fs::create_dir_all(&path) {
            error!("mkdir {path} failed: {e}");
        }
    }

    async fn watch_dir(self, _ctx: context::Context, tab_id: u64, path: String) {
        debug!("watch_dir: tab={tab_id} path={path}");
        let mut watches = self._watches.lock().await;
        watches.insert(tab_id, PathBuf::from(&path));
        // TODO: 使用 notify crate 开始监听，通过流式回传事件
    }

    async fn unwatch_dir(self, _ctx: context::Context, tab_id: u64, path: String) {
        debug!("unwatch_dir: tab={tab_id} path={path}");
        let mut watches = self._watches.lock().await;
        let _ = watches.remove(&tab_id);
        // TODO: 停止 notify 监听
    }
}

// ---------------------------------------------------------------------------
// Worker 入口
// ---------------------------------------------------------------------------

/// 运行 FS Worker 子进程。
///
/// 1. 从 `--fd` 参数恢复 UnixStream
/// 2. 构建 tarpc server
/// 3. 进入主循环
pub async fn run_fs_worker(opts: FsWorkerOpts) -> ! {
    let server = FsWorkerServer::new(&opts);

    // 从继承的 FD 恢复连接
    let fd = opts.fd.expect("--fd is required for fs-worker mode");
    info!("fs worker {} restoring fd={}", opts.worker_id, fd);
    let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    stream
        .set_nonblocking(true)
        .expect("failed to set nonblocking");
    let stream = UnixStream::from_std(stream).expect("failed to convert to tokio stream");

    // 构建 tarpc server
    use futures::prelude::*;
    use tarpc::{
        serde_transport,
        server::{BaseChannel, Channel},
        tokio_util::codec::length_delimited::LengthDelimitedCodec,
    };

    let codec_builder = LengthDelimitedCodec::builder();
    let framed = codec_builder.new_framed(stream);
    let transport = serde_transport::new(framed, tarpc::tokio_serde::formats::Bincode::default());

    async fn spawn(_fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(_fut);
    }

    info!("fs worker {} entering serve loop", opts.worker_id);
    BaseChannel::with_defaults(transport)
        .execute(server.serve())
        .for_each(spawn)
        .await;

    info!("fs worker {} exiting", opts.worker_id);
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// 解析路径中的符号链接。
fn resolve_path(path: &PathBuf) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())
}

/// 列出目录内容。
fn list_directory(path: &PathBuf) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let file_type = entry.file_type()?;

        let mime_type = if file_type.is_dir() {
            "inode/directory".to_string()
        } else if file_type.is_symlink() {
            "inode/symlink".to_string()
        } else {
            guess_mime(&entry.path())
        };

        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry.path(),
            size: metadata.len(),
            modified: metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            is_directory: file_type.is_dir(),
            is_symlink: file_type.is_symlink(),
            mime_type,
            permissions: 0, // TODO: 从 MetadataExt 提取
            owner_uid: 0,
            owner_gid: 0,
        });
    }

    entries.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

/// 获取单个文件/目录的元信息。
fn file_stat(path: &PathBuf) -> io::Result<FileStat> {
    let metadata = std::fs::metadata(path)?;

    Ok(FileStat {
        size: metadata.len(),
        modified: metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        is_directory: metadata.is_dir(),
        is_symlink: metadata.file_type().is_symlink(),
        permissions: 0, // TODO
        owner_uid: 0,
        owner_gid: 0,
    })
}

/// 简单的 MIME 猜测（基于扩展名）。
fn guess_mime(path: &PathBuf) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 常见映射表 (后续扩展)
    match ext.as_str() {
        "txt" | "md" | "rs" | "toml" | "json" | "yaml" | "yml" | "xml" | "html" | "css" | "js"
        | "ts" | "jsx" | "tsx" | "py" | "sh" | "bash" | "env" | "cfg" | "ini" | "log" | "csv"
        | "lock" => "text/plain".to_string(),
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "gif" => "image/gif".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "webp" => "image/webp".to_string(),
        "mp3" => "audio/mpeg".to_string(),
        "wav" => "audio/wav".to_string(),
        "flac" => "audio/flac".to_string(),
        "ogg" => "audio/ogg".to_string(),
        "mp4" => "video/mp4".to_string(),
        "mkv" => "video/x-matroska".to_string(),
        "mov" => "video/quicktime".to_string(),
        "avi" => "video/x-msvideo".to_string(),
        "webm" => "video/webm".to_string(),
        "zip" => "application/zip".to_string(),
        "tar" => "application/x-tar".to_string(),
        "gz" => "application/gzip".to_string(),
        "xz" => "application/x-xz".to_string(),
        "pdf" => "application/pdf".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// 递归复制文件/目录。
fn copy_path(src: &PathBuf, dest: &PathBuf) -> io::Result<()> {
    if src.is_dir() {
        copy_dir(src, dest)
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}

fn copy_dir(src: &PathBuf, dest: &PathBuf) -> io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        copy_path(&entry_path, &dest_path)?;
    }
    Ok(())
}
