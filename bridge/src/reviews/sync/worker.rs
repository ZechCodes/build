use super::reconcile::{reconcile, SyncResult};
use super::scheduler::Scheduler;
use crate::store::Store;
use crate::watch::metadata::{metadata_roots, MetadataCallback, MetadataWatchers};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const PAGE: usize = 64;
const WORK_PER_TURN: usize = 8;
const POLL: Duration = Duration::from_secs(30);

type MetadataTasks = BTreeMap<PathBuf, BTreeSet<String>>;
pub(super) struct Worker {
    store: Store,
    scheduler: Scheduler,
    watchers: MetadataWatchers,
    events: mpsc::Receiver<PathBuf>,
    enqueue: MetadataCallback,
    identities: MetadataTasks,
    scanning: Option<Scan>,
    poll_after: Instant,
    invalidate: Arc<dyn Fn(SyncResult) + Send + Sync>,
}
#[derive(Default)]
struct Scan {
    after: Option<String>,
    active: BTreeSet<String>,
    identities: MetadataTasks,
    repositories: BTreeSet<PathBuf>,
}
impl Worker {
    pub(super) fn new(store: Store, invalidate: Arc<dyn Fn(SyncResult) + Send + Sync>) -> Self {
        let (sender, events) = mpsc::sync_channel(128);
        Self {
            store,
            scheduler: Scheduler::default(),
            watchers: MetadataWatchers::default(),
            events,
            enqueue: Arc::new(move |path| {
                let _ = sender.try_send(path);
            }),
            identities: BTreeMap::new(),
            scanning: Some(Scan::default()),
            poll_after: Instant::now(),
            invalidate,
        }
    }
    pub(super) fn turn(&mut self, stopped: &AtomicBool) {
        let now = Instant::now();
        if self.scanning.is_none() && now >= self.poll_after {
            self.scanning = Some(Scan::default());
        }
        self.scan_page(now);
        self.drain_events(now);
        for task in self.scheduler.ready(now, WORK_PER_TURN) {
            if stopped.load(Ordering::Acquire) {
                break;
            }
            let retry = match reconcile(&self.store, &task) {
                Ok(result) => {
                    let retry = result.retry;
                    if result.persisted {
                        (self.invalidate)(result);
                    }
                    retry
                }
                Err(error) => {
                    eprintln!("review sync: task {task}: {error}");
                    true
                }
            };
            self.scheduler.settle(&task, Instant::now(), retry);
        }
    }
    fn scan_page(&mut self, now: Instant) {
        let Some(mut scan) = self.scanning.take() else {
            return;
        };
        let tasks = match self
            .store
            .list_active_review_sync_tasks(scan.after.as_deref(), PAGE)
        {
            Ok(tasks) => tasks,
            Err(error) => {
                eprintln!("review sync: discover tasks: {error}");
                self.poll_after = now + POLL;
                return;
            }
        };
        let complete = tasks.len() < PAGE;
        for task in tasks {
            scan.after = Some(task.clone());
            scan.active.insert(task.clone());
            self.scheduler.enqueue(task.clone(), now, true);
            self.scan_metadata(&task, &mut scan);
        }
        if complete {
            self.finish_scan(scan, now);
        } else {
            self.scanning = Some(scan);
        }
    }
    fn scan_metadata(&self, task: &str, scan: &mut Scan) {
        let Ok(Some(review)) = self.store.load_review(task) else {
            return;
        };
        for binding in review.bindings {
            for repository in [
                binding.working_repository,
                binding.source_repository,
                binding.receiving_repository,
            ] {
                if let Ok(roots) = metadata_roots(&repository) {
                    for root in roots {
                        scan.identities.entry(root).or_default().insert(task.into());
                    }
                }
                scan.repositories.insert(repository);
            }
        }
    }
    fn finish_scan(&mut self, scan: Scan, now: Instant) {
        self.scheduler.retain(&scan.active);
        self.identities = scan.identities;
        let repositories: Vec<_> = scan.repositories.into_iter().collect();
        for error in self.watchers.reconcile(&repositories, self.enqueue.clone()) {
            eprintln!("review sync: metadata watch: {error}");
        }
        self.poll_after = now + POLL;
    }
    fn drain_events(&mut self, now: Instant) {
        for path in self.events.try_iter().take(128) {
            if let Some(tasks) = self.identities.get(&path) {
                for task in tasks {
                    self.scheduler.enqueue(task.clone(), now, false);
                }
            }
        }
    }
}
