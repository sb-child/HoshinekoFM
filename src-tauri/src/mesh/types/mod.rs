//! Mesh 端点类型：消息枚举与 Handler trait 定义。
//!
//! 每种端点类型（Instance / Window / FsService）有一个独立文件，
//! 包含其消息枚举、Handler trait 和 dispatch 函数。
//! 新增端点类型只需在此添加新文件即可，不需要修改已有端点文件。

pub mod fsservice;
pub mod instance;
pub mod ui;
pub mod window;

pub use fsservice::{FsServiceHandler, FsServiceMsg, dispatch_fs_msg};
pub use instance::{InstanceHandler, InstanceMsg, dispatch_instance_msg};
pub use ui::*;
pub use window::{WindowHandler, WindowMsg, dispatch_window_msg};
