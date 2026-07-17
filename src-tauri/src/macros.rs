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

/// 使用**已有 span**（继承父 span 上下文）的 tokio::spawn 替代。
///
/// 用法: `spawn_with_span!(span.clone(), async move { ... })`
///
/// 与 [`spawn_instrumented!`] 的区别：本宏用已有的 `tracing::Span` 对象，
/// spawn 的 task 会继承该 span 的字段和父链；`spawn_instrumented!` 则创建全新的 span。
#[macro_export]
macro_rules! spawn_with_span {
    ($span:expr, async { $($body:tt)* }) => {
        tokio::spawn(::tracing::Instrument::instrument(async { $($body)* }, $span))
    };
    ($span:expr, async move { $($body:tt)* }) => {
        tokio::spawn(::tracing::Instrument::instrument(async move { $($body)* }, $span))
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

/// 使用**已有 span** 的 spawn_blocking 替代。
///
/// 用法: `spawn_blocking_with_span!(span.clone(), move || { ... })`
#[macro_export]
macro_rules! spawn_blocking_with_span {
    ($span:expr, move || $body:block) => {
        tokio::task::spawn_blocking(move || {
            let _guard = $span.entered();
            $body
        })
    };
    ($span:expr, $body:block) => {
        tokio::task::spawn_blocking(move || {
            let _guard = $span.entered();
            $body
        })
    };
}
