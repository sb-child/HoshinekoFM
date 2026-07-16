//! FS Worker 子进程模块。
//!
//! ## 模块
//! - [`run`]：子进程入口 (`run_fs_worker`)
//! - [`service`]：[`FsWorkerServer`] 服务实现
//! - [`pipeline`]：Watch 流水线装配
//! - [`registry`]：订阅生命周期管理
//! - [`inotify`]：单一 inotify 实例管理
//! - [`scheduler`]：事件合并 + 级联退避
//! - [`builder`]：限流 stat + 增量构建
//! - [`router`]：tarpc RPC 分发 + 背压降级
//! - [`config`]：WatchConfig 运行时参数
//! - [`ops`]：变更操作 + 冲突决策
//! - [`files`]：文件系统工具函数

pub mod builder;
pub mod config;
pub mod files;
pub mod inotify;
pub mod ops;
pub mod pipeline;
pub mod registry;
pub mod router;
pub mod run;
pub mod scheduler;
pub mod service;

#[cfg(test)]
mod tests;

pub use config::WatchConfig;
pub use run::run_fs_worker;
