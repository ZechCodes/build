//! Shared fixtures for real deep waits; requires the core ledger to be GREEN.
use super::*;

pub(crate) struct TrackedWrite {
    pub(crate) registry: Arc<WriteWaitRegistry>,
    root: LogicalWrite,
}

impl TrackedWrite {
    pub(crate) fn new(name: &str) -> Self {
        let registry = WriteWaitRegistry::new();
        let root = registry.begin(name, "deep-wait-test", Instant::now());
        root.admitted();
        let tracked = Self { registry, root };
        let positive = tracked.context().interval(IntervalCategory::OwnExternal);
        assert!(tracked
            .row()
            .active
            .iter()
            .any(|entry| entry.id == positive.id()));
        drop(positive);
        assert!(tracked.row().active.is_empty());
        tracked
    }

    pub(crate) fn context(&self) -> WriteContext {
        self.root.context()
    }

    pub(crate) fn row(&self) -> WriteSnapshot {
        self.registry
            .snapshot()
            .into_iter()
            .find(|row| row.id == self.root.id())
            .unwrap()
    }

    pub(crate) fn wait_for(&self, category: IntervalCategory) -> bool {
        let deadline = Instant::now() + Duration::from_millis(750);
        while Instant::now() < deadline {
            if self
                .row()
                .active
                .iter()
                .any(|entry| entry.category == category)
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    pub(crate) fn complete(self) {
        self.root.complete();
    }
}
