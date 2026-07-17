//! 工具宏 -- 带 tracing span 的 spawn 等。
//!
//! 所有 spawn 统一走此宏，确保每个异步/阻塞任务都有 tracing span 覆盖。

/// 带 tracing span 的 tokio::spawn 替代。
///
/// 用法: `spawn!("task_name", async { ... })` 或 `spawn!("task_name", async move { ... })`
#[macro_export]
macro_rules! spawn_instrumented {
    ($span:expr, async { $($body:tt)* }) => {
        tokio::spawn(async {
            let _span = ::tracing::info_span!($span).entered();
            $($body)*
        })
    };
    ($span:expr, async move { $($body:tt)* }) => {
        tokio::spawn(async move {
            let _span = ::tracing::info_span!($span).entered();
            $($body)*
        })
    };
}

/// 带 tracing span 的 tokio::task::spawn_blocking 替代。
///
/// 用法: `spawn_blocking!("task_name", move || { ... })`
#[macro_export]
macro_rules! spawn_blocking_instrumented {
    ($span:expr, move || $body:block) => {
        tokio::task::spawn_blocking(move || {
            let _span = ::tracing::info_span!($span).entered();
            $body
        })
    };
    ($span:expr, $body:block) => {
        tokio::task::spawn_blocking(move || {
            let _span = ::tracing::info_span!($span).entered();
            $body
        })
    };
}
