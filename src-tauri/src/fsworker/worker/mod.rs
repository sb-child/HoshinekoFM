//! FS Worker 子进程模块。
//!
//! ## 模块
//! - [`run`]：子进程入口 (`run_fs_worker`)
//! - [`service`]：[`FsWorkerServer`] 服务实现
//! - [`watch`]：[`WatchPool`] + 级联恢复
//! - [`ops`]：变更操作 + 冲突决策
//! - [`files`]：文件系统工具函数

pub mod files;
pub mod ops;
pub mod run;
pub mod service;
pub mod watch;

pub use run::run_fs_worker;
