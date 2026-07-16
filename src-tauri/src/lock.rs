//! 安全锁定辅助 -- 避免 poisoned mutex 级联 panic。
//!
//! AGENTS.md 要求检查所有 `.lock().unwrap()` 使用场景。这些辅助方法
//! 在锁被 poisoning 时不会 panic，而是直接恢复内部数据。

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// `std::sync::Mutex` 的安全锁定扩展。
pub(crate) trait LockSafe<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T>;
}

impl<T> LockSafe<T> for Mutex<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `std::sync::RwLock` 的安全只读锁定扩展。
pub(crate) trait ReadSafe<T> {
    fn read_safe(&self) -> RwLockReadGuard<'_, T>;
}

impl<T> ReadSafe<T> for RwLock<T> {
    fn read_safe(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(|e| e.into_inner())
    }
}

/// `std::sync::RwLock` 的安全写入锁定扩展。
pub(crate) trait WriteSafe<T> {
    fn write_safe(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> WriteSafe<T> for RwLock<T> {
    fn write_safe(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(|e| e.into_inner())
    }
}
