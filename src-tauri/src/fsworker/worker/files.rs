//! 文件系统工具：目录扫描、文件构建、MIME 猜测、创建/复制/移动/重命名。

use std::{
    ffi::CString,
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    time::SystemTime,
};

use tracing::warn;

use crate::fsworker::protocol::File;
use crate::mesh::types::ui::EntryKind;

/// 解析符号链接到规范路径（失败则原样返回）。
pub fn resolve_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 规范化路径（解析 `.` 和 `..`），不跟随符号链接。
/// 与 `resolve_path` 不同：`/bin -> /usr/bin` 时返回 `/bin` 而非 `/usr/bin`。
pub fn normalize_path_no_symlink(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut components = Vec::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                if !matches!(components.last(), Some(Component::RootDir)) {
                    components.pop();
                }
            }
            Component::CurDir => {}
            c => components.push(c),
        }
    }
    components.into_iter().collect()
}

/// 列出目录内容为 `File` 列表（含 mime；缩略图 TODO）。
pub(crate) fn list_dir_files(dir: &Path) -> Vec<File> {
    let mut files = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            warn!("list_dir_files({dir:?}) failed: {e}");
            return files;
        }
    };
    for entry in read.flatten() {
        if let Some(f) = build_file(&entry.path()) {
            files.push(f);
        }
    }
    files.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    files
}

/// 构建单个 `File`（用 symlink 元数据判断软链接，用 stat 取属性）。
pub(crate) fn build_file(path: &Path) -> Option<File> {
    let symlink_meta = std::fs::symlink_metadata(path).ok()?;
    let is_symlink = symlink_meta.file_type().is_symlink();
    let meta = std::fs::metadata(path).unwrap_or(symlink_meta);

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_directory = meta.is_dir();
    let mime = if is_directory {
        Some("inode/directory".to_string())
    } else if is_symlink && std::fs::metadata(path).is_err() {
        Some("inode/symlink".to_string())
    } else {
        Some(guess_mime(path))
    };

    Some(File {
        name,
        path: path.to_path_buf(),
        size: meta.len(),
        modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        is_directory,
        is_symlink,
        permissions: meta.mode(),
        owner_uid: meta.uid(),
        owner_gid: meta.gid(),
        mime,
        thumbnail: None,
    })
}

/// 创建文件或目录。
pub fn create_entry(path: &Path, kind: EntryKind) -> io::Result<()> {
    match kind {
        EntryKind::Dir => std::fs::create_dir_all(path),
        EntryKind::File => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map(|_| ())
        }
    }
}

/// rename/move 的执行策略，用于关闭 TOCTOU 窗口。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProceedStrategy {
    /// 目标在检查时不存在 -> renameat2(RENAME_NOREPLACE)
    NoConflict,
    /// 覆盖
    Overwrite,
    /// 自动或手动重命名
    Rename,
}

/// 根据策略执行 rename。
pub fn rename_with_strategy(src: &Path, dst: &Path, strategy: ProceedStrategy) -> io::Result<()> {
    match strategy {
        ProceedStrategy::NoConflict | ProceedStrategy::Rename => {
            let src_c = CString::new(src.as_os_str().as_bytes())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            let dst_c = CString::new(dst.as_os_str().as_bytes())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            let ret = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    src_c.as_ptr(),
                    libc::AT_FDCWD,
                    dst_c.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if ret != 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        ProceedStrategy::Overwrite => std::fs::rename(src, dst),
    }
}

/// 移动路径：先 rename，跨设备（EXDEV）时退化为 copy + 删除。
pub fn move_path(src: &Path, dst: &Path) -> io::Result<()> {
    move_path_inner(src, dst, false)
}

/// 带 NOREPLACE 策略的移动。
pub fn move_path_noreplace(src: &Path, dst: &Path) -> io::Result<()> {
    move_path_inner(src, dst, true)
}

fn move_path_inner(src: &Path, dst: &Path, noreplace: bool) -> io::Result<()> {
    let rename_result = rename_with_strategy(
        src,
        dst,
        if noreplace {
            ProceedStrategy::NoConflict
        } else {
            ProceedStrategy::Overwrite
        },
    );
    match rename_result {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(nix::errno::Errno::EXDEV as i32) => {
            copy_path(src, dst, noreplace)?;
            if src.is_dir() {
                std::fs::remove_dir_all(src)
            } else {
                std::fs::remove_file(src)
            }
        }
        Err(e) => Err(e),
    }
}

/// 递归复制文件/目录。`noreplace=true` 时目标文件使用 create_new 避免截断覆盖。
pub fn copy_path(src: &Path, dst: &Path, noreplace: bool) -> io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_path(&entry.path(), &dst.join(entry.file_name()), noreplace)?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if noreplace {
            let src_f = std::fs::File::open(src)?;
            let dst_f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dst)?;
            let mut src_f = src_f;
            let mut dst_f = dst_f;
            std::io::copy(&mut src_f, &mut dst_f).map(|_| ())
        } else {
            std::fs::copy(src, dst).map(|_| ())
        }
    }
}

/// 为已存在的目标生成唯一路径：`foo.txt` -> `foo (1).txt` -> `foo (2).txt` …
pub fn unique_path(dst: &Path) -> PathBuf {
    let parent = dst
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"));
    let stem = dst
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = dst.extension().map(|s| s.to_string_lossy().into_owned());

    for i in 1..10_000 {
        let name = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}.{}", nix::unistd::getpid().as_raw()))
}

/// 基于扩展名的 MIME 猜测。
fn guess_mime(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

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

// --
// 虚拟文件系统 & poll 支持
// --

/// 检测路径是否在虚拟文件系统上（/proc、/sys）。
/// 这些文件系统不产生 inotify 事件，必须用 kernel poll (POLLPRI) 监听。
pub fn is_virtual_fs(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/proc/") || s.starts_with("/sys/")
}

/// 以 O_RDONLY | O_NONBLOCK 打开文件，用于 AsyncFd 包装。
pub fn open_nonblock(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

/// 将路径拆分为逐级祖先目录。
pub fn split_path_segments(path: &Path) -> Vec<PathBuf> {
    let mut segments = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if current == Path::new("/") {
            continue;
        }
        segments.push(current.clone());
    }
    segments
}
