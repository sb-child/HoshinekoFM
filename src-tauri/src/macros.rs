//! 工具宏 -- 带 tracing span 的 spawn 等。

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
