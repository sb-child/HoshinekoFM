//! 面包屑管理器 — per-window 独立任务，watch 每个路径段的属性变化。
//!
//! ## 数据来源
//!
//! - **home/mount 信息**：来自 `FsService.watch_breadcrumb`（FsWorker 内部读取
//!   /etc/passwd 和 /proc/mounts，监听变化主动推送 `WatchDelta::BreadcrumbSegments`）。
//! - **文件元数据**（is_symlink、accessible）：来自 `FsService.watch_stat`
//!   （每个路径段一个 watcher，FsWorker 按 normalized path 去重）。
//!
//! 两个来源通过独立通道汇入 BreadcrumbManager，合并后 emit `hf:breadcrumbs`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::Emitter;
use tracing::{debug, warn};

use crate::app::state::AppStateManager;
use crate::channel;
use crate::fsworker::{UidToken, WorkerRequestContent};
use crate::ipc::protocol::{BreadcrumbEntry, BreadcrumbsPayload, WatchDelta};

// ---------------------------------------------------------------------------
// BreadcrumbCommand — watch 线程 → BreadcrumbManager 的命令
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(super) enum BreadcrumbCommand {
    NavUpdate {
        tab_id: u64,
        path: PathBuf,
        token: UidToken,
    },
    TabSwitch {
        tab_id: u64,
    },
    TabClosed {
        tab_id: u64,
    },
    Shutdown,
}

// ---------------------------------------------------------------------------
// 内部事件类型
// ---------------------------------------------------------------------------

enum BreadcrumbEvent {
    /// 来自 watch_breadcrumb：home/mount 上下文更新
    Context { tab_id: u64, delta: WatchDelta },
    /// 来自 watch_stat：单个段的文件元数据更新
    SegmentMeta {
        tab_id: u64,
        segment_idx: usize,
        delta: WatchDelta,
    },
}

// ---------------------------------------------------------------------------
// BreadcrumbManager
// ---------------------------------------------------------------------------

pub(super) struct BreadcrumbManager {
    cmd_tx: channel::Tx<BreadcrumbCommand>,
}

impl BreadcrumbManager {
    pub fn start(window: tauri::Window, mgr: Arc<AppStateManager>) -> Self {
        let (cmd_tx, cmd_rx) = channel::unbounded();
        tokio::spawn(async move {
            Self::run_loop(window, cmd_rx, mgr).await;
        });
        Self { cmd_tx }
    }

    pub fn send(&self, cmd: BreadcrumbCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    async fn run_loop(
        window: tauri::Window,
        cmd_rx: channel::RxAsync<BreadcrumbCommand>,
        mgr: Arc<AppStateManager>,
    ) {
        let mut tabs: HashMap<u64, BreadcrumbTabState> = HashMap::new();
        let mut active_tab: Option<u64> = None;

        let (event_tx, event_rx) = channel::unbounded::<BreadcrumbEvent>();

        debug!("BreadcrumbManager started");

        loop {
            enum Msg {
                Cmd(BreadcrumbCommand),
                Event(BreadcrumbEvent),
            }

            let msg = {
                let cmd_fut = cmd_rx.recv();
                let evt_fut = event_rx.recv();
                tokio::pin!(cmd_fut);
                tokio::pin!(evt_fut);
                tokio::select! {
                    c = cmd_fut => match c {
                        Ok(cmd) => Msg::Cmd(cmd),
                        Err(_) => break,
                    },
                    e = evt_fut => match e {
                        Ok(ev) => Msg::Event(ev),
                        Err(_) => Msg::Cmd(BreadcrumbCommand::Shutdown),
                    },
                }
            };

            match msg {
                Msg::Cmd(cmd) => match cmd {
                    BreadcrumbCommand::NavUpdate {
                        tab_id,
                        path,
                        token,
                    } => {
                        tabs.remove(&tab_id);
                        let ctx =
                            BreadcrumbTabState::build(&path, token, &mgr, event_tx.clone(), tab_id)
                                .await;
                        tabs.insert(tab_id, ctx);
                        if active_tab == Some(tab_id) {
                            if let Some(ctx) = tabs.get(&tab_id) {
                                Self::emit(&window, tab_id, &ctx.entries);
                            }
                        }
                    }
                    BreadcrumbCommand::TabSwitch { tab_id } => {
                        active_tab = Some(tab_id);
                        if let Some(ctx) = tabs.get(&tab_id) {
                            Self::emit(&window, tab_id, &ctx.entries);
                        }
                    }
                    BreadcrumbCommand::TabClosed { tab_id } => {
                        tabs.remove(&tab_id);
                        if active_tab == Some(tab_id) {
                            active_tab = None;
                        }
                    }
                    BreadcrumbCommand::Shutdown => {
                        tabs.clear();
                        debug!("BreadcrumbManager shutting down");
                        return;
                    }
                },
                Msg::Event(ev) => {
                    let tab_id = match &ev {
                        BreadcrumbEvent::Context { tab_id, .. } => *tab_id,
                        BreadcrumbEvent::SegmentMeta { tab_id, .. } => *tab_id,
                    };
                    if let Some(ctx) = tabs.get_mut(&tab_id) {
                        ctx.apply_event(&ev);
                        if active_tab == Some(tab_id) {
                            Self::emit(&window, tab_id, &ctx.entries);
                        }
                    }
                }
            }
        }

        debug!("BreadcrumbManager exited");
    }

    fn emit(window: &tauri::Window, tab_id: u64, entries: &[BreadcrumbEntry]) {
        let _ = window.emit(
            "hf:breadcrumbs",
            BreadcrumbsPayload {
                tab_id,
                entries: entries.to_vec(),
            },
        );
    }
}

// ---------------------------------------------------------------------------
// BreadcrumbTabState — per-tab 面包屑状态
// ---------------------------------------------------------------------------

struct BreadcrumbTabState {
    entries: Vec<BreadcrumbEntry>,
    /// 每个 watcher 对应的 drop guard（RAII：drop 时 unwatch）
    _watcher_guards: Vec<WatcherGuard>,
}

/// 持有 UidToken + watch_id，drop 时自动释放 watcher。
struct WatcherGuard {
    watch_id: u64,
    token: UidToken,
}

impl Drop for WatcherGuard {
    fn drop(&mut self) {
        self.token.registry.unregister_watch(self.watch_id);
        let token = self.token.clone();
        let id = self.watch_id;
        tokio::spawn(async move {
            let _ = token
                .send_request(WorkerRequestContent::Unwatch { watch_id: id })
                .await;
        });
    }
}

impl BreadcrumbTabState {
    async fn build(
        path: &Path,
        token: UidToken,
        mgr: &AppStateManager,
        event_tx: channel::Tx<BreadcrumbEvent>,
        tab_id: u64,
    ) -> Self {
        let segment_paths = split_path_segments(path);
        let seg_count = segment_paths.len();

        let entries: Vec<BreadcrumbEntry> = segment_paths
            .iter()
            .map(|seg_path| {
                let path_str = seg_path.to_string_lossy().to_string();
                let name = seg_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "/".to_string());
                BreadcrumbEntry {
                    name,
                    path: path_str,
                    is_symlink: false,
                    symlink_target: None,
                    is_mount_point: false,
                    mount_source: None,
                    is_home: false,
                    home_username: None,
                    accessible: true,
                }
            })
            .collect();

        let mut guards = Vec::with_capacity(1 + seg_count);

        // 1. watch_breadcrumb：home/mount 上下文（FsWorker 监听 /proc/mounts 变化）
        match mgr.fs_service.watch_breadcrumb(&token, path).await {
            Ok(w) => {
                let (events, watch_id, _token) = w.into_parts();
                let tx = event_tx.clone();
                tokio::spawn(async move {
                    while let Ok(delta) = events.recv().await {
                        if tx.send(BreadcrumbEvent::Context { tab_id, delta }).is_err() {
                            break;
                        }
                    }
                });
                guards.push(WatcherGuard {
                    watch_id,
                    token: _token,
                });
            }
            Err(e) => {
                warn!("watch_breadcrumb failed for {path:?}: {e}");
            }
        };

        // 2. watch_stat：每个段的文件元数据（is_symlink、accessible）
        for (idx, seg_path) in segment_paths.iter().enumerate() {
            match mgr.fs_service.watch_stat(&token, seg_path).await {
                Ok(w) => {
                    let (events, watch_id, _token) = w.into_parts();
                    let tx = event_tx.clone();
                    tokio::spawn(async move {
                        while let Ok(delta) = events.recv().await {
                            if tx
                                .send(BreadcrumbEvent::SegmentMeta {
                                    tab_id,
                                    segment_idx: idx,
                                    delta,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    });
                    guards.push(WatcherGuard {
                        watch_id,
                        token: _token,
                    });
                }
                Err(e) => {
                    warn!("watch_stat failed for {seg_path:?}: {e}");
                }
            }
        }

        Self {
            entries,
            _watcher_guards: guards,
        }
    }

    fn apply_event(&mut self, ev: &BreadcrumbEvent) {
        match ev {
            BreadcrumbEvent::Context { delta, .. } => {
                if let WatchDelta::BreadcrumbSegments(segments) = delta {
                    for (i, seg) in segments.iter().enumerate() {
                        if let Some(entry) = self.entries.get_mut(i) {
                            entry.is_home = seg.is_home;
                            entry.home_username = seg.home_username.clone();
                            entry.is_mount_point = seg.is_mount_point;
                            entry.mount_source = seg.mount_source.clone();
                        }
                    }
                }
            }
            BreadcrumbEvent::SegmentMeta {
                segment_idx, delta, ..
            } => {
                if let Some(entry) = self.entries.get_mut(*segment_idx) {
                    apply_file_delta(entry, delta);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 文件元数据 delta → BreadcrumbEntry 字段更新
// ---------------------------------------------------------------------------

fn apply_file_delta(entry: &mut BreadcrumbEntry, delta: &WatchDelta) {
    match delta {
        WatchDelta::Reset(files) => {
            if let Some(file) = files.first() {
                entry.accessible = true;
                entry.is_symlink = file.is_symlink;
                entry.symlink_target = None;
            }
        }
        WatchDelta::Upsert(file) => {
            entry.accessible = true;
            entry.is_symlink = file.is_symlink;
            entry.symlink_target = None;
        }
        WatchDelta::Inaccessible { .. }
        | WatchDelta::Recovering { .. }
        | WatchDelta::FatalError { .. } => {
            entry.accessible = false;
        }
        WatchDelta::Remove(_) => {
            entry.accessible = false;
        }
        WatchDelta::Rename { to, .. } => {
            entry.name = to
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            entry.path = to.to_string_lossy().to_string();
        }
        WatchDelta::ConnectionLost { .. } => {}
        WatchDelta::BreadcrumbSegments { .. } => {}
    }
}

// ---------------------------------------------------------------------------
// 路径工具
// ---------------------------------------------------------------------------

fn split_path_segments(path: &Path) -> Vec<PathBuf> {
    let mut segments = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if current == Path::new("/") {
            continue;
        }
        segments.push(current.clone());
    }
    segments
}
