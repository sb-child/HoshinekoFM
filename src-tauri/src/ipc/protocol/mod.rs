//! RPC 服务定义 (tarpc service traits) 和共享数据类型。
//!
//! ## 传输层
//!
//! | 场景            | transport               | 方向                          |
//! | --------------- | ----------------------- | ----------------------------- |
//! | 实例→Worker     | 匿名 socketpair + tarpc | 主进程=client, Worker=server  |
//! | Worker→实例回调 | 匿名 socketpair + tarpc | Worker=client, 主进程=server  |
//!
//! 注意：实例间通信类型已移至 `crate::mesh::types`。
//! FsWorker RPC 类型已移至 `crate::fsworker::protocol`。
//! UI 类型已移至 `crate::mesh::types::ui`。

pub mod file;
pub mod ui;

// 保留向后兼容重导出（过渡期）
pub use file::*;
pub use ui::*;
