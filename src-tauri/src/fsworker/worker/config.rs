use std::time::Duration;

/// Watch 子系统的运行时参数。
///
/// 所有值均有合理默认值，生产环境通常不需要调整。
/// 测试时可按需覆盖任意字段：
/// ```ignore
/// WatchConfig { max_parallel_stat: 1, ..Default::default() }
/// ```
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// inotify 事件合并窗口。该时间窗内同一路径的多个事件合并为一个。
    pub event_coalesce_window: Duration,

    /// DeltaBuilder 最大并行 `spawn_blocking` 数。
    /// 控制同时 stat 的文件数量，避免慢文件系统（NFS）上线程爆炸。
    pub max_parallel_stat: usize,

    /// 级联退避起始间隔（200ms，桌面文件管理器无需长退避）。
    pub cascade_backoff_base: Duration,

    /// 级联退避最大间隔（2s，超出此值标记 FatalError 让用户手动刷新）。
    pub cascade_backoff_max: Duration,

    /// 背压降级：连续 N 次 push 延迟超阈值后，标记该 watch_id dirty。
    pub backpressure_dirty_threshold: usize,

    /// 背压降级：单次 push 超时阈值。
    pub backpressure_push_timeout: Duration,

    /// Reset（全量目录）每批推送文件数。大目录分批评避免单次序列化膨胀。
    pub reset_batch_size: usize,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            event_coalesce_window: Duration::from_millis(50),
            max_parallel_stat: 16,
            cascade_backoff_base: Duration::from_millis(200),
            cascade_backoff_max: Duration::from_secs(2),
            backpressure_dirty_threshold: 3,
            backpressure_push_timeout: Duration::from_millis(100),
            reset_batch_size: 64,
        }
    }
}
