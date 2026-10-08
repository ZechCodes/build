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
        self.turn_at(stopped, Instant::now());
    }
    fn turn_at(&mut self, stopped: &AtomicBool, now: Instant) {
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
        let Ok(Some(review)) = self.store.load_review_sync_state(task) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reviews::sync::reconcile::tests::Fixture;

    #[test]
    fn periodic_poll_recovers_latest_receiver_when_all_metadata_events_are_dropped() {
        let fixture = Fixture::new();
        let workspace = fixture.review().workspace_id;
        let (sender, invalidations) = mpsc::channel();
        let mut worker = Worker::new(
            fixture.store.clone(),
            Arc::new(move |result| {
                sender.send(result).unwrap();
            }),
        );
        // Disconnect the real hints before watcher registration. Its callbacks
        // can still fire, but every hint is discarded rather than scheduling work.
        let (discarded, empty_events) = mpsc::channel();
        drop(discarded);
        worker.events = empty_events;
        let started = Instant::now();
        let stopped = AtomicBool::new(false);
        worker.turn_at(&stopped, started);
        let startup = invalidations.try_recv().unwrap();
        assert!(startup.persisted);
        assert_eq!(startup.task_id, fixture.task_id());
        assert_eq!(startup.workspace_id, workspace);

        fixture.commit("dropped-first.txt");
        fixture.push();
        let latest = fixture.commit("dropped-latest.txt");
        fixture.push();
        worker.turn_at(&stopped, started + Duration::from_secs(29));
        assert_eq!(fixture.review().snapshots.len(), 1);
        assert!(invalidations.try_recv().is_err());

        worker.turn_at(&stopped, started + Duration::from_secs(31));
        let recovered = invalidations.try_recv().unwrap();
        assert!(recovered.persisted);
        assert_eq!(recovered.task_id, fixture.task_id());
        assert_eq!(recovered.workspace_id, workspace);
        let review = fixture.review();
        assert_eq!(review.snapshots.len(), 2);
        assert_eq!(
            review.snapshots[1].directories[0].head.as_deref(),
            Some(latest.as_str())
        );
        worker.turn_at(&stopped, started + Duration::from_secs(32));
        assert!(invalidations.try_recv().is_err());
    }
}
