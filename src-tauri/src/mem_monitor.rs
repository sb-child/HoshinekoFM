//! 进程内存监控：定期读取 `/proc/self/status` 输出 RSS / VmSize 到 tracing。
//!
//! 用法：在 main 中 spawn，传入 `CancellationToken` 和采样间隔。
//! 退出前调用 `token.cancel()` 即可优雅停止。

use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info, instrument, warn};

/// 启动内存监控后台 task。
///
/// - `token`: 外部取消信号
/// - `interval`: 采样间隔（建议 5s）
/// - `parent_span`: 可选的父 span，用于继承上下文（如 worker_span）
#[instrument(name = "mem_monitor", skip_all)]
pub fn spawn_mem_monitor(
    token: CancellationToken,
    interval: Duration,
    parent_span: Option<tracing::Span>,
) {
    let span = parent_span.unwrap_or_else(|| tracing::info_span!("mem_monitor_loop"));
    tokio::spawn(
        async move {
            let mut timer = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        info!("mem_monitor cancelled, exiting");
                        break;
                    }
                    _ = timer.tick() => {
                        if let Err(e) = report_memory() {
                            warn!("failed to read /proc/self/status: {e}");
                        }
                    }
                }
            }
        }
        .instrument(span),
    );
}

/// 读取 `/proc/self/status` 并输出 VmRSS / VmSize（单位 KB）。
fn report_memory() -> std::io::Result<()> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let mut vm_rss_kb: u64 = 0;
    let mut vm_size_kb: u64 = 0;

    for line in status.lines() {
        if let Some(val) = line.strip_prefix("VmRSS:") {
            vm_rss_kb = parse_kb(val);
        } else if let Some(val) = line.strip_prefix("VmSize:") {
            vm_size_kb = parse_kb(val);
        }
    }

    info!(vm_rss_kb, vm_size_kb, "memory status");
    Ok(())
}

/// 从 "  12345 kB" 格式中提取数字。
fn parse_kb(s: &str) -> u64 {
    s.trim()
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}
