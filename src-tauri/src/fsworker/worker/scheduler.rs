use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Instant;

use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use super::config::WatchConfig;
use crate::channel::{self, RxAsync, Tx};

use super::files::is_virtual_fs;
use super::inotify::RawEvent;

pub enum SchedulerCmd {
    Track { path: PathBuf },
    Untrack { path: PathBuf },
    RequestReset { path: PathBuf },
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum SchedulerEvent {
    FilesChanged {
        path: PathBuf,
        affected: Vec<PathBuf>,
    },
    LostAccess {
        path: PathBuf,
        reason: String,
    },
    RecoveredAccess {
        path: PathBuf,
    },
    FatalError {
        path: PathBuf,
        reason: String,
    },
}

struct PathState {
    last_flush: Instant,
    affected: Vec<PathBuf>,
    is_dir: bool,
    is_virtual: bool,
    cascade_level: Option<usize>,
    cascade_next: Option<Instant>,
    dirty: bool,
    pending_reset: bool,
}

pub struct WatchScheduler {
    event_rx: RxAsync<RawEvent>,
    cmd_rx: RxAsync<SchedulerCmd>,
    event_tx: Tx<SchedulerEvent>,
    cancel: CancellationToken,
    config: WatchConfig,
    paths: HashMap<PathBuf, PathState>,
    dirty_queue: VecDeque<PathBuf>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl WatchScheduler {
    pub fn spawn(
        cancel: CancellationToken,
        config: &WatchConfig,
        raw_event_rx: RxAsync<RawEvent>,
    ) -> (Tx<SchedulerCmd>, RxAsync<SchedulerEvent>, Self) {
        let (cmd_tx, cmd_rx) = channel::unbounded();
        let (event_tx, out_event_rx) = channel::unbounded();

        (
            cmd_tx,
            out_event_rx,
            Self {
                event_rx: raw_event_rx,
                cmd_rx,
                event_tx,
                cancel,
                config: config.clone(),
                paths: HashMap::new(),
                dirty_queue: VecDeque::new(),
                sleep: None,
            },
        )
    }

    #[instrument(skip(self), name = "scheduler")]
    pub async fn run(&mut self) {
        info!("WatchScheduler starting");

        loop {
            self.flush_dirty().await;

            let sleep_enabled = self.sleep.is_some();

            select! {
                biased;

                _ = self.cancel.cancelled() => {
                    info!("WatchScheduler shutting down");
                    return;
                }

                _ = async {
                    if let Some(sleep) = &mut self.sleep {
                        sleep.await
                    } else {
                        std::future::pending::<()>().await
                    }
                }, if sleep_enabled => {
                    self.sleep = None;
                    self.retry_cascades().await;
                }

                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Ok(SchedulerCmd::Track { path }) => self.on_track(path),
                        Ok(SchedulerCmd::Untrack { path }) => self.on_untrack(path),
                        Ok(SchedulerCmd::RequestReset { path }) => self.on_request_reset(path),
                        Ok(SchedulerCmd::Shutdown) => {
                            info!("WatchScheduler received Shutdown");
                            self.cancel.cancel();
                        }
                        Err(_) => return,
                    }
                }

                event = self.event_rx.recv() => {
                    match event {
                        Ok(raw) => self.on_raw_event(raw),
                        Err(_) => return,
                    }
                }
            }
        }
    }

    fn on_track(&mut self, path: PathBuf) {
        self.paths.entry(path.clone()).or_insert_with(|| PathState {
            last_flush: Instant::now(),
            affected: Vec::new(),
            is_dir: false,
            is_virtual: false,
            cascade_level: None,
            cascade_next: None,
            dirty: false,
            pending_reset: false,
        });
    }

    fn on_untrack(&mut self, path: PathBuf) {
        self.paths.remove(&path);
        self.dirty_queue.retain(|p| p != &path);
    }

    fn on_request_reset(&mut self, path: PathBuf) {
        if let Some(entry) = self.paths.get_mut(&path) {
            entry.pending_reset = true;
            entry.dirty = true;
            if !self.dirty_queue.contains(&path) {
                self.dirty_queue.push_back(path);
            }
        }
    }

    fn on_raw_event(&mut self, raw: RawEvent) {
        if raw.scope == super::inotify::WatchScope::SelfOnly {
            return;
        }

        let Some(entry) = self.paths.get_mut(&raw.path) else {
            return;
        };

        if entry.dirty && entry.last_flush.elapsed() < self.config.event_coalesce_window {
            entry.affected.extend(raw.affected_paths);
            entry.affected.sort();
            entry.affected.dedup();
            return;
        }

        let old_dirty = entry.dirty;
        let old_affected = std::mem::take(&mut entry.affected);
        let old_pending = entry.pending_reset;
        entry.pending_reset = false;

        // let entry borrow end before calling send_event

        if old_dirty {
            self.send_event(&raw.path, &old_affected, old_pending);
        }

        // re-acquire mutable borrow (guaranteed to exist: checked at function entry)
        let entry = self
            .paths
            .get_mut(&raw.path)
            .expect("scheduler invariant: entry must exist after initial check");
        entry.affected = raw.affected_paths;
        entry.dirty = true;
        entry.last_flush = Instant::now();
        if !self.dirty_queue.contains(&raw.path) {
            self.dirty_queue.push_back(raw.path.clone());
        }
    }

    async fn flush_dirty(&mut self) {
        let mut i = 0;
        while i < self.dirty_queue.len() {
            let path = self.dirty_queue[i].clone();
            let expired = self.paths.get(&path).map_or(true, |e| {
                !e.dirty || e.last_flush.elapsed() >= self.config.event_coalesce_window
            });

            if !expired {
                i += 1;
                continue;
            }

            self.dirty_queue.remove(i);
            // extract data before doing anything that borrows self
            let (cascade_free, was_dirty, is_dir) = match self.paths.get_mut(&path) {
                Some(e) => (e.cascade_level.is_none(), e.dirty, e.is_dir),
                None => continue,
            };

            if !was_dirty {
                continue;
            }

            // verify_type outside of get_mut borrow
            let mut is_dir_mut = is_dir;
            let is_virtual = crate::fsworker::worker::files::is_virtual_fs(&path);
            {
                let p = path.clone();
                match tokio::task::spawn_blocking(move || {
                    let _span = tracing::info_span!("scheduler::flush_metadata").entered();
                    std::fs::metadata(&p).map(|m| (m.is_dir(), m.modified()))
                })
                .await
                {
                    Ok(Ok((dir_now, _))) => {
                        if dir_now != is_dir_mut {
                            is_dir_mut = dir_now;
                        }
                    }
                    _ => {}
                }
            }

            // now get_mut again for the actual push
            let (affected, pr_to_send) = if let Some(e) = self.paths.get_mut(&path) {
                if is_dir_mut != e.is_dir {
                    e.is_dir = is_dir_mut;
                }
                if is_virtual != e.is_virtual {
                    e.is_virtual = is_virtual;
                }
                let affected = std::mem::take(&mut e.affected);
                let pr = e.pending_reset;
                if cascade_free {
                    e.dirty = false;
                    e.pending_reset = false;
                }
                (affected, pr)
            } else {
                continue;
            };

            if cascade_free {
                self.send_event(&path, &affected, pr_to_send);
            }
        }
    }

    fn send_event(&mut self, path: &PathBuf, affected: &[PathBuf], pending_reset: bool) {
        if affected.is_empty() && !pending_reset {
            return;
        }
        let event = if pending_reset {
            SchedulerEvent::RecoveredAccess { path: path.clone() }
        } else {
            SchedulerEvent::FilesChanged {
                path: path.clone(),
                affected: affected.to_vec(),
            }
        };
        if self.event_tx.send(event).is_err() {
            warn!("WatchScheduler: event_tx closed for {:?}", path);
        }
    }

    fn enter_cascade(&mut self, path: PathBuf, level: usize, reason: String) {
        let delay = (self.config.cascade_backoff_base * (1u32 << level as u32))
            .min(self.config.cascade_backoff_max);
        if let Some(e) = self.paths.get_mut(&path) {
            e.cascade_level = Some(level);
            e.cascade_next = Some(Instant::now() + delay);
        }
        let _ = self
            .event_tx
            .send(SchedulerEvent::LostAccess { path, reason });
        self.update_sleep();
    }

    async fn retry_cascades(&mut self) {
        let now = Instant::now();
        let pairs: Vec<(PathBuf, usize)> = self
            .paths
            .iter()
            .filter_map(|(p, e)| {
                e.cascade_level
                    .zip(e.cascade_next)
                    .and_then(|(l, t)| (now >= t).then_some((p.clone(), l)))
            })
            .collect();
        for (path, level) in pairs {
            self.retry_access(path, level).await;
        }
        self.update_sleep();
    }

    async fn retry_access(&mut self, path: PathBuf, level: usize) {
        let p = path.clone();
        let ok = tokio::task::spawn_blocking(move || {
            let _span = tracing::info_span!("scheduler::retry_access").entered();
            p.metadata().is_ok()
        })
        .await
        .unwrap_or(false);

        if ok {
            if let Some(e) = self.paths.get_mut(&path) {
                e.cascade_level = None;
                e.cascade_next = None;
            }
            let _ = self.event_tx.send(SchedulerEvent::RecoveredAccess { path });
        } else if level < 10 {
            self.enter_cascade(path, level + 1, "still inaccessible".into());
        } else {
            if let Some(e) = self.paths.get_mut(&path) {
                e.cascade_level = None;
                e.cascade_next = None;
            }
            let _ = self.event_tx.send(SchedulerEvent::FatalError {
                path,
                reason: "max cascade retries".into(),
            });
        }
    }

    fn update_sleep(&mut self) {
        let now = Instant::now();
        let next = self.paths.values().filter_map(|e| e.cascade_next).min();
        self.sleep = next.map(|t| {
            let dur = t.saturating_duration_since(now);
            Box::pin(tokio::time::sleep(dur))
        });
    }
}
