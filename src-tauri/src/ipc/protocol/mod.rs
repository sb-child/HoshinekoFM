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
//!
//! ## 子模块
//!
//! - `file` — 文件系统类型 + FsWorker RPC 接口
//! - `mesh` — 实例间/窗口间通信类型 + InstanceService RPC 接口
//! - `ui` — UI 层类型（Tab、导航、剪贴板、仪表盘、设备）

pub mod file;
pub mod mesh;
pub mod ui;

// 重导出所有类型，保持向后兼容
pub use file::*;
pub use mesh::*;
pub use ui::*;
