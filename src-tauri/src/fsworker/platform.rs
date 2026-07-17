//! 平台相关辅助函数（Unix / AppImage）。

use std::{io, os::unix::io::RawFd};

use nix::fcntl::{FcntlArg, FdFlag, fcntl};

/// Worker 孤儿退出码：Worker 检测到主进程消失时以此码退出。
pub(crate) const ORPHAN_EXIT_CODE: i32 = 100;

/// 清除文件描述符的 CLOEXEC 标志，使子进程能继承。
pub(crate) fn clear_cloexec(fd: RawFd) -> io::Result<()> {
    use std::os::fd::BorrowedFd;
    // SAFETY: fd 此刻有效
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut flags = FdFlag::from_bits_retain(
        fcntl(borrowed, FcntlArg::F_GETFD).map_err(|e| io::Error::other(e.to_string()))?,
    );
    flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(borrowed, FcntlArg::F_SETFD(flags)).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(())
}

/// 获取当前可执行文件路径（处理 AppImage）。
pub(crate) fn get_exe_path() -> std::path::PathBuf {
    if let Ok(appimage) = std::env::var("APPIMAGE") {
        std::path::PathBuf::from(appimage)
    } else {
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("hnfm"))
    }
}

/// 是否在 AppImage 环境中运行。
pub(crate) fn is_appimage() -> bool {
    std::env::var("APPIMAGE").is_ok()
}
