use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, instrument, warn};

use super::config::WatchConfig;
use crate::channel::{self, RxAsync, Tx};
use crate::fsworker::protocol::{File, WatchDelta};

use super::files::{augment_mime, augment_stat, build_file, build_file_skeleton, needs_mime};
use super::scheduler::SchedulerEvent;

pub enum BuilderCmd {
    Reset {
        path: PathBuf,
    },
    CancelPath {
        path: PathBuf,
    },
    Shutdown,
}

struct StatJob {
    file: File,
    watch_root: PathBuf,
    delta_tx: Tx<(PathBuf, WatchDelta)>,
    mime_tx: mpsc::Sender<MimeJob>,
}

struct MimeJob {
    file: File,
    watch_root: PathBuf,
    delta_tx: Tx<(PathBuf, WatchDelta)>,
}

struct Pipeline {
    stat_txs: Vec<mpsc::Sender<StatJob>>,
    mime_txs: Vec<mpsc::Sender<MimeJob>>,
    _stat_threads: Vec<std::thread::JoinHandle<()>>,
    _mime_threads: Vec<std::thread::JoinHandle<()>>,
}

const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
const BATCH_SIZE: usize = 64;

impl Pipeline {
    fn start(cancel: CancellationToken, config: &WatchConfig) -> Self {
        let mut stat_txs = Vec::with_capacity(config.stat_pool_size);
        let mut stat_threads = Vec::with_capacity(config.stat_pool_size);

        for i in 0..config.stat_pool_size {
            let (tx, rx) = mpsc::channel::<StatJob>();
            stat_txs.push(tx);
            let cancel = cancel.clone();
            stat_threads.push(
                std::thread::Builder::new()
                    .name(format!("stat-{i}"))
                    .spawn(move || stat_worker(i, rx, cancel))
                    .unwrap(),
            );
        }

        let mut mime_txs = Vec::with_capacity(config.mime_pool_size);
        let mut mime_threads = Vec::with_capacity(config.mime_pool_size);

        for i in 0..config.mime_pool_size {
            let (tx, rx) = mpsc::channel::<MimeJob>();
            mime_txs.push(tx);
            let cancel = cancel.clone();
            mime_threads.push(
                std::thread::Builder::new()
                    .name(format!("mime-{i}"))
                    .spawn(move || mime_worker(i, rx, cancel))
                    .unwrap(),
            );
        }

        Self {
            stat_txs,
            mime_txs,
            _stat_threads: stat_threads,
            _mime_threads: mime_threads,
        }
    }

    fn shutdown(self) {
        for tx in self.stat_txs {
            drop(tx);
        }
        for tx in self.mime_txs {
            drop(tx);
        }
        for h in self._stat_threads {
            let _ = h.join();
        }
        for h in self._mime_threads {
            let _ = h.join();
        }
    }
}

fn stat_worker(i: usize, rx: mpsc::Receiver<StatJob>, cancel: CancellationToken) {
    let _span = tracing::info_span!("stat_pool_worker", thread = i).entered();
    let mut batch: Vec<(PathBuf, Tx<(PathBuf, WatchDelta)>, File)> =
        Vec::with_capacity(BATCH_SIZE);

    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(job) => {
                if cancel.is_cancelled() {
                    break;
                }
                let root = job.watch_root.clone();
                let dt = job.delta_tx.clone();
                let mime_tx = job.mime_tx.clone();
                let path = job.file.path.clone();
                match augment_stat(job.file) {
                    Ok(file) => {
                        batch.push((root.clone(), dt.clone(), file.clone()));
                        if batch.len() >= BATCH_SIZE {
                            flush_stat_batch(&mut batch);
                        }
                        if needs_mime(&file) {
                            let _ = mime_tx.send(MimeJob {
                                file,
                                watch_root: root,
                                delta_tx: dt,
                            });
                        }
                    }
                    Err(e) => {
                        warn!("augment_stat {}: {e}", path.display());
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    break;
                }
                flush_stat_batch(&mut batch);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush_stat_batch(&mut batch);
                break;
            }
        }
    }
}

fn flush_stat_batch(
    batch: &mut Vec<(PathBuf, Tx<(PathBuf, WatchDelta)>, File)>,
) {
    if batch.is_empty() {
        return;
    }
    let items = std::mem::replace(batch, Vec::with_capacity(BATCH_SIZE));
    let mut groups: HashMap<PathBuf, (Tx<(PathBuf, WatchDelta)>, Vec<File>)> = HashMap::new();

    for (root, tx, file) in items {
        let entry = groups.entry(root).or_insert_with(|| (tx, Vec::new()));
        entry.1.push(file);
    }

    for (root, (tx, files)) in groups {
        let _ = tx.send((root, WatchDelta::UpsertBatch(files)));
    }
}

fn mime_worker(i: usize, rx: mpsc::Receiver<MimeJob>, cancel: CancellationToken) {
    let _span = tracing::info_span!("mime_pool_worker", thread = i).entered();
    let mut batch: Vec<(PathBuf, Tx<(PathBuf, WatchDelta)>, File)> =
        Vec::with_capacity(BATCH_SIZE);

    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(job) => {
                if cancel.is_cancelled() {
                    break;
                }
                let root = job.watch_root.clone();
                let dt = job.delta_tx.clone();
                match augment_mime(job.file) {
                    Ok(file) => {
                        batch.push((root, dt, file));
                        if batch.len() >= BATCH_SIZE {
                            flush_mime_batch(&mut batch);
                        }
                    }
                    Err(e) => {
                        warn!("augment_mime failed: {e}");
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    break;
                }
                flush_mime_batch(&mut batch);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush_mime_batch(&mut batch);
                break;
            }
        }
    }
}

fn flush_mime_batch(
    batch: &mut Vec<(PathBuf, Tx<(PathBuf, WatchDelta)>, File)>,
) {
    if batch.is_empty() {
        return;
    }
    let items = std::mem::replace(batch, Vec::with_capacity(BATCH_SIZE));
    let mut groups: HashMap<PathBuf, (Tx<(PathBuf, WatchDelta)>, Vec<File>)> = HashMap::new();

    for (root, tx, file) in items {
        let entry = groups.entry(root).or_insert_with(|| (tx, Vec::new()));
        entry.1.push(file);
    }

    for (root, (tx, files)) in groups {
        let _ = tx.send((root, WatchDelta::UpsertBatch(files)));
    }
}

pub struct DeltaBuilder {
    event_rx: RxAsync<SchedulerEvent>,
    cmd_rx: RxAsync<BuilderCmd>,
    delta_tx: Tx<(PathBuf, WatchDelta)>,
    cancel: CancellationToken,
    config: WatchConfig,
    pipeline: Option<Pipeline>,
    cancelled: HashMap<PathBuf, ()>,
    /// 有活跃 Reset 的 watch_root 集合，期间 inotify 增量事件跳过 stat
    reset_active: HashSet<PathBuf>,
    /// reader 完成 Reset 后通过此 channel 通知移除 reset_active 条目
    reset_done_tx: Tx<PathBuf>,
    reset_done_rx: RxAsync<PathBuf>,
}

impl DeltaBuilder {
    pub(crate) fn spawn(
        cancel: CancellationToken,
        config: &WatchConfig,
        event_rx: RxAsync<SchedulerEvent>,
    ) -> (Tx<BuilderCmd>, RxAsync<(PathBuf, WatchDelta)>, Self) {
        let (cmd_tx, cmd_rx) = channel::unbounded();
        let (delta_tx, delta_rx) = channel::unbounded();
        let (reset_done_tx, reset_done_rx) = channel::unbounded();

        (
            cmd_tx,
            delta_rx,
            Self {
                event_rx,
                cmd_rx,
                delta_tx,
                cancel,
                config: config.clone(),
                pipeline: None,
                cancelled: HashMap::new(),
                reset_active: HashSet::new(),
                reset_done_tx,
                reset_done_rx,
            },
        )
    }

    #[instrument(skip(self), name = "builder")]
    pub async fn run(&mut self) {
        info!("DeltaBuilder starting");
        self.pipeline = Some(Pipeline::start(self.cancel.clone(), &self.config));

        loop {
            select! {
                biased;

                _ = self.cancel.cancelled() => {
                    info!("DeltaBuilder shutting down");
                    break;
                }

                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Ok(BuilderCmd::Reset { path }) => {
                            self.handle_reset(path);
                        }
                        Ok(BuilderCmd::CancelPath { path }) => {
                            self.reset_active.remove(&path);
                            self.cancelled.insert(path, ());
                        }
                        Ok(BuilderCmd::Shutdown) => {
                            info!("DeltaBuilder received Shutdown");
                            self.cancel.cancel();
                        }
                        Err(_) => break,
                    }
                }

                event = self.event_rx.recv() => {
                    match event {
                        Ok(evt) => self.handle_event(evt),
                        Err(_) => break,
                    }
                }

                done = self.reset_done_rx.recv() => {
                    if let Ok(path) = done {
                        self.reset_active.remove(&path);
                    }
                }
            }
        }

        if let Some(pipeline) = self.pipeline.take() {
            pipeline.shutdown();
        }
    }

    fn handle_reset(&mut self, path: PathBuf) {
        self.cancelled.remove(&path);
        self.reset_active.insert(path.clone());

        let pipeline = self.pipeline.as_ref().expect("pipeline not started");
        let stat_txs = pipeline.stat_txs.clone();
        let mime_txs = pipeline.mime_txs.clone();
        let delta_tx = self.delta_tx.clone();
        let batch_size = self.config.skeleton_batch_size;
        let done_tx = self.reset_done_tx.clone();
        let reset_path = path.clone();

        let h = tokio::task::spawn_blocking(move || {
            let _span = tracing::info_span!("builder::reset_reader").entered();
            let read = match std::fs::read_dir(&reset_path) {
                Ok(r) => r,
                Err(e) => {
                    let _ = delta_tx.send((
                        reset_path.clone(),
                        WatchDelta::Inaccessible {
                            path: reset_path.clone(),
                            ancestor: reset_path.clone(),
                            level: 0,
                            reason: e.to_string(),
                        },
                    ));
                    let _ = done_tx.send(reset_path);
                    return;
                }
            };

            let stat_cnt = AtomicUsize::new(0);
            let mime_cnt = AtomicUsize::new(0);
            let mut batch: Vec<File> = Vec::with_capacity(batch_size);

            for entry in read.flatten() {
                let Some(file) = build_file_skeleton(&entry) else {
                    continue;
                };
                batch.push(file.clone());

                if batch.len() >= batch_size {
                    let chunk =
                        std::mem::replace(&mut batch, Vec::with_capacity(batch_size));
                    let _ =
                        delta_tx.send((reset_path.clone(), WatchDelta::UpsertBatch(chunk)));
                }

                let si = stat_cnt.fetch_add(1, Ordering::Relaxed) % stat_txs.len();
                let mi = mime_cnt.fetch_add(1, Ordering::Relaxed) % mime_txs.len();
                if stat_txs[si]
                    .send(StatJob {
                        file,
                        watch_root: reset_path.clone(),
                        delta_tx: delta_tx.clone(),
                        mime_tx: mime_txs[mi].clone(),
                    })
                    .is_err()
                {
                    let _ = done_tx.send(reset_path);
                    return;
                }
            }
            if !batch.is_empty() {
                let _ =
                    delta_tx.send((reset_path.clone(), WatchDelta::UpsertBatch(batch)));
            }
            let _ = done_tx.send(reset_path);
        });

        tokio::spawn(
            async move {
                if let Err(e) = h.await {
                    tracing::error!("DeltaBuilder reset reader panicked: {e}");
                }
            }
            .instrument(tracing::info_span!("builder::reset_monitor")),
        );
    }

    fn handle_event(&mut self, event: SchedulerEvent) {
        match event {
            SchedulerEvent::FilesChanged { path, affected } => {
                self.cancelled.remove(&path);
                let skip_stat = self.reset_active.contains(&path);

                for file_path in affected {
                    if file_path == path {
                        debug!(
                            path = %path.display(),
                            "builder: skipping watch_root in affected (SelfOnly leak)"
                        );
                        continue;
                    }
                    if !file_path.exists() {
                        let _ = self
                            .delta_tx
                            .send((path.clone(), WatchDelta::Remove(file_path)));
                        continue;
                    }
                    if skip_stat {
                        continue;
                    }
                    let dt = self.delta_tx.clone();
                    let p = path.clone();
                    let h = tokio::task::spawn_blocking(move || {
                        let _span = tracing::info_span!(
                            "builder::incremental_build_file"
                        )
                        .entered();
                        let file = build_file(&file_path)?;
                        let _ = dt.send((p, WatchDelta::Upsert(file)));
                        Some(())
                    });
                    tokio::spawn(
                        async move {
                            if let Err(e) = h.await {
                                tracing::error!(
                                    "builder incremental build_file panicked: {e}"
                                );
                            }
                        }
                        .instrument(
                            tracing::info_span!("builder::incremental_monitor"),
                        ),
                    );
                }
            }
            SchedulerEvent::RecoveredAccess { path } => {
                self.cancelled.remove(&path);
                self.handle_reset(path);
            }
            SchedulerEvent::LostAccess { path, reason } => {
                let _ = self.delta_tx.send((
                    path.clone(),
                    WatchDelta::Inaccessible {
                        path: path.clone(),
                        ancestor: path.clone(),
                        level: 0,
                        reason,
                    },
                ));
            }
            SchedulerEvent::FatalError { path, reason } => {
                let _ = self
                    .delta_tx
                    .send((path.clone(), WatchDelta::FatalError { path, reason }));
            }
        }
    }
}
