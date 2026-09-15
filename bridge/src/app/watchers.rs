//! Which worktrees have a filesystem watcher on them, and why (wire spec Part
//! 1, step 1.2).
//!
//! One [`WorktreeWatcher`] per worktree that at least one subscription covers
//! with `git` or `files`. The set is reconciled — never pushed one at a time —
//! against two inputs: what the bus says is covered
//! ([`ChangeBus::covered_worktrees`]) and what the board currently lists as a
//! worktree entity (the roots snapshot the app refreshes on every board
//! change). A watcher starts when both say yes and drops when either says no,
//! so a subscription going away and an entity leaving the board are the same
//! code path.
//!
//! Reconciling starts watchers, and starting one walks the tree to register
//! inotify watches, so it never runs under the app mutex: the subscribe
//! verbs hand it to the off-lock drain ([`crate::app::runtime::deferred`]) so
//! the reply can say `live` or `polled` truthfully, and every other trigger
//! wakes the reconciler task spawned by [`WorktreeWatchers::spawn_reconciler`].

use crate::changes::ChangeBus;
use crate::watch::{WatchError, WorktreeWatcher};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Entity id → worktree root, for every board entity that has a checkout: a
/// run's worktree, a project's primary checkout, an external worktree.
pub(in crate::app) type WorktreeRoots = BTreeMap<String, PathBuf>;

/// How often the reconciler sweeps unprompted. A session whose push failed
/// loses its subscriptions inside the bus with nobody to wake us, so the
/// sweep is what eventually drops the watchers it covered.
pub(in crate::app) const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(in crate::app) struct WorktreeWatchers {
    /// The board's worktree entities, as of the last board change.
    roots: Mutex<WorktreeRoots>,
    /// The watchers alive right now, by entity id. Held only by
    /// [`reconcile`](Self::reconcile), which is the one writer.
    running: Mutex<BTreeMap<String, WorktreeWatcher>>,
    /// Worktrees whose start failed and was logged. Once per worktree, for
    /// as long as it is on the board.
    warned: Mutex<BTreeSet<String>>,
    /// The reconciler task's doorbell.
    wake: tokio::sync::Notify,
}

impl WorktreeWatchers {
    pub(in crate::app) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Every entity the board lists with a checkout — what `{"kind":"all"}`
    /// resolves to. The bus's [`crate::changes::BoardEntities`] source.
    pub(in crate::app) fn entity_ids(&self) -> Vec<String> {
        self.roots.lock().unwrap().keys().cloned().collect()
    }

    /// Replace the roots snapshot. Under the app mutex this is a map swap and
    /// nothing else — the reconcile it implies runs off the lock.
    pub(in crate::app) fn set_roots(&self, roots: WorktreeRoots) {
        *self.roots.lock().unwrap() = roots;
    }

    /// The board moved: remember what it lists now and reconcile soon.
    pub(in crate::app) fn board_moved(&self, roots: WorktreeRoots) {
        self.set_roots(roots);
        self.wake.notify_one();
    }

    /// Something else changed the coverage (a session closed): reconcile soon.
    pub(in crate::app) fn wake(&self) {
        self.wake.notify_one();
    }

    /// Whether a watcher is on this entity's worktree right now.
    #[cfg(test)]
    pub(in crate::app) fn is_watching(&self, entity_id: &str) -> bool {
        self.running.lock().unwrap().contains_key(entity_id)
    }

    /// The worktrees whose watcher could not start, each logged once.
    #[cfg(test)]
    pub(in crate::app) fn warned(&self) -> Vec<String> {
        self.warned.lock().unwrap().iter().cloned().collect()
    }

    /// Bring the running set in line with what is covered and on the board.
    /// BLOCKING: starting a watcher registers a watch per directory. Runs on
    /// the off-lock drain or on `spawn_blocking`, never under the app mutex.
    pub(in crate::app) fn reconcile(&self, bus: &Arc<ChangeBus>) {
        let roots = self.roots.lock().unwrap().clone();
        let covered = bus.covered_worktrees();
        let wanted: BTreeMap<&str, &PathBuf> = roots
            .iter()
            .filter(|(id, _)| covered.contains(*id))
            .map(|(id, root)| (id.as_str(), root))
            .collect();
        let mut running = self.running.lock().unwrap();
        running.retain(|id, _| wanted.contains_key(id.as_str()));
        self.forget_uncovered(bus, &roots, &wanted);
        for (id, root) in wanted {
            if running.contains_key(id) {
                continue;
            }
            match WorktreeWatcher::start(root, id, Arc::clone(bus)) {
                Ok(watcher) => {
                    running.insert(id.to_string(), watcher);
                    bus.clear_polled(id);
                }
                Err(error) => {
                    self.warn_once(id, &error);
                    bus.mark_polled(id);
                }
            }
        }
    }

    /// A worktree nobody covers any more, or that left the board, is neither
    /// polled nor remembered as unwatchable: the next time it is covered it
    /// gets a fresh start and, if that fails again, a fresh line in the log.
    fn forget_uncovered(
        &self,
        bus: &ChangeBus,
        roots: &WorktreeRoots,
        wanted: &BTreeMap<&str, &PathBuf>,
    ) {
        let mut warned = self.warned.lock().unwrap();
        warned.retain(|id| wanted.contains_key(id.as_str()));
        for id in roots.keys().filter(|id| !wanted.contains_key(id.as_str())) {
            bus.clear_polled(id);
        }
    }

    fn warn_once(&self, entity_id: &str, error: &WatchError) {
        if self.warned.lock().unwrap().insert(entity_id.to_string()) {
            eprintln!("[watch] {error}; git and files for {entity_id} come from the TTL refresh");
        }
    }

    /// The task that reconciles whenever [`board_moved`](Self::board_moved)
    /// or [`wake`](Self::wake) rings, and once per [`SWEEP_INTERVAL`] anyway.
    /// A build with no runtime under it (the synchronous unit tests) gets no
    /// task, exactly as the bus gets no flusher.
    pub(in crate::app) fn spawn_reconciler(watchers: Arc<Self>, bus: Arc<ChangeBus>) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                loop {
                    let _ = tokio::time::timeout(SWEEP_INTERVAL, watchers.wake.notified()).await;
                    let (watchers, bus) = (Arc::clone(&watchers), Arc::clone(&bus));
                    let _ = tokio::task::spawn_blocking(move || watchers.reconcile(&bus)).await;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carrier::SessionSender;
    use crate::changes::{Kind, Mode, Priority, Scope, SubscriptionSpec, WatchState};

    fn bus_over(watchers: &Arc<WorktreeWatchers>) -> Arc<ChangeBus> {
        let entities = Arc::clone(watchers);
        ChangeBus::with_sources(
            Duration::from_millis(250),
            Arc::new(move || entities.entity_ids()),
            Arc::new(|_| Vec::new()),
        )
    }

    fn worktree_spec(id: &str, scope: Scope) -> SubscriptionSpec {
        SubscriptionSpec {
            id: id.into(),
            scope,
            kinds: [Kind::Git, Kind::Files].into_iter().collect(),
            mode: Mode::Realtime,
            priority: Priority::Foreground,
        }
    }

    fn roots_of(pairs: &[(&str, &std::path::Path)]) -> WorktreeRoots {
        pairs
            .iter()
            .map(|(id, root)| (id.to_string(), root.to_path_buf()))
            .collect()
    }

    /// The first subscription covering a worktree starts its watcher; the
    /// last one leaving drops it.
    #[test]
    fn a_covering_subscription_starts_the_watcher_and_the_last_unsubscribe_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let watchers = WorktreeWatchers::new();
        let bus = bus_over(&watchers);
        watchers.set_roots(roots_of(&[("run-7", dir.path())]));
        let session = SessionSender::detached("s-1");

        watchers.reconcile(&bus);
        assert!(!watchers.is_watching("run-7"), "nothing covers it yet");

        bus.subscribe(
            &session,
            worktree_spec("s-a", Scope::Entity("run-7".into())),
        );
        watchers.reconcile(&bus);
        assert!(watchers.is_watching("run-7"));

        bus.subscribe(
            &session,
            worktree_spec("s-b", Scope::Entity("run-7".into())),
        );
        bus.unsubscribe_one("s-1", "s-a");
        watchers.reconcile(&bus);
        assert!(
            watchers.is_watching("run-7"),
            "one covering subscription remains"
        );

        bus.unsubscribe_one("s-1", "s-b");
        watchers.reconcile(&bus);
        assert!(!watchers.is_watching("run-7"));
    }

    /// A state-only subscription wants no watcher, whatever its scope.
    #[test]
    fn a_state_only_subscription_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let watchers = WorktreeWatchers::new();
        let bus = bus_over(&watchers);
        watchers.set_roots(roots_of(&[("run-7", dir.path())]));
        bus.subscribe(
            &SessionSender::detached("s-1"),
            SubscriptionSpec {
                kinds: [Kind::State, Kind::Thread].into_iter().collect(),
                ..worktree_spec("s-a", Scope::All)
            },
        );
        watchers.reconcile(&bus);
        assert!(!watchers.is_watching("run-7"));
    }

    /// `{"kind":"all"}` covers every board worktree, tracked as the board
    /// changes: an entity leaving the board loses its watcher, one arriving
    /// gets one.
    #[test]
    fn scope_all_follows_the_board() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let watchers = WorktreeWatchers::new();
        let bus = bus_over(&watchers);
        watchers.set_roots(roots_of(&[("run-a", a.path())]));
        bus.subscribe(
            &SessionSender::detached("s-1"),
            worktree_spec("s-bg", Scope::All),
        );
        watchers.reconcile(&bus);
        assert!(watchers.is_watching("run-a"));
        assert!(!watchers.is_watching("run-b"));

        watchers.set_roots(roots_of(&[("run-b", b.path())]));
        watchers.reconcile(&bus);
        assert!(!watchers.is_watching("run-a"), "left the board");
        assert!(watchers.is_watching("run-b"), "arrived on it");
    }

    /// A root `notify` cannot watch is logged once, marks the worktree polled
    /// for every subscription covering it, and is forgotten again once
    /// nothing covers it.
    #[test]
    fn an_unwatchable_root_is_logged_once_and_answers_polled() {
        let dir = tempfile::tempdir().unwrap();
        let watchers = WorktreeWatchers::new();
        let bus = bus_over(&watchers);
        watchers.set_roots(roots_of(&[("run-7", &dir.path().join("missing"))]));
        let spec = worktree_spec("s-a", Scope::Entity("run-7".into()));
        let session = SessionSender::detached("s-1");
        bus.subscribe(&session, spec.clone());

        watchers.reconcile(&bus);
        watchers.reconcile(&bus);
        assert_eq!(watchers.warned(), vec!["run-7".to_string()]);
        assert!(!watchers.is_watching("run-7"));
        assert_eq!(bus.watch_state(&spec), WatchState::Polled);
        assert_eq!(
            bus.watch_state(&worktree_spec("s-all", Scope::All)),
            WatchState::Polled
        );

        bus.unsubscribe_one("s-1", "s-a");
        watchers.reconcile(&bus);
        assert!(watchers.warned().is_empty());
        assert_eq!(bus.watch_state(&spec), WatchState::Live);
    }
}
