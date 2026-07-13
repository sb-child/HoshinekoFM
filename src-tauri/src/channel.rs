//! crossfire channel 类型别名。
//!
//! 所有 channel 统一使用 crossfire 的 lockless MPSC/oneshot 实现。
//! 这里定义的别名让类型签名更简洁。

use crossfire::{AsyncRx, MTx, Rx, mpsc};

/// 无界异步通道发送端（多生产者，可在 async/blocking 两侧使用）。
pub type Tx<T> = MTx<mpsc::List<T>>;

/// 无界异步通道接收端（单消费者，仅 async 上下文）。
pub type RxAsync<T> = AsyncRx<mpsc::List<T>>;

/// 无界阻塞通道接收端（单消费者，仅 blocking 上下文）。
pub type RxBlocking<T> = Rx<mpsc::List<T>>;

/// 创建无界异步通道。返回 `(Tx<T>, RxAsync<T>)`。
pub fn unbounded<T: 'static>() -> (Tx<T>, RxAsync<T>) {
    mpsc::unbounded_async()
}

/// 创建无界阻塞通道。返回 `(Tx<T>, RxBlocking<T>)`。
pub fn unbounded_blocking<T: 'static>() -> (Tx<T>, RxBlocking<T>) {
    mpsc::unbounded_blocking()
}

/// crossfire oneshot 类型别名。
pub mod oneshot {
    pub use crossfire::oneshot::{RxOneshot, TxOneshot, oneshot};
}
