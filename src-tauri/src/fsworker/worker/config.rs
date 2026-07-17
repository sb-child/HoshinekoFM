use std::time::Duration;

/// Watch 子系统的运行时参数。
///
/// 所有值均有合理默认值，生产环境通常不需要调整。
/// 测试时可按需覆盖任意字段：
/// ```ignore
/// WatchConfig { stat_pool_size: 1, ..Default::default() }
/// ```
#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub event_coalesce_window: Duration,
    pub cascade_backoff_base: Duration,
    pub cascade_backoff_max: Duration,
    pub backpressure_dirty_threshold: usize,
    pub backpressure_push_timeout: Duration,

    /// Skeleton 阶段每批推送文件数（内容极小，可大些）。
    pub skeleton_batch_size: usize,

    /// Stat worker pool 线程数（所有 watcher 共享）。
    pub stat_pool_size: usize,

    /// MIME worker pool 线程数（所有 watcher 共享）。
    pub mime_pool_size: usize,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            event_coalesce_window: Duration::from_millis(50),
            cascade_backoff_base: Duration::from_millis(200),
            cascade_backoff_max: Duration::from_secs(2),
            backpressure_dirty_threshold: 3,
            backpressure_push_timeout: Duration::from_millis(100),
            skeleton_batch_size: 128,
            stat_pool_size: 16,
            mime_pool_size: 16,
        }
    }
}
