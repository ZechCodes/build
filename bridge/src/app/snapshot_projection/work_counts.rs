//! Exact producer work counts used by publication regression tests.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct Counts {
    pub branches: usize,
    pub agents: usize,
    pub owner_candidates: usize,
    pub external_candidates: usize,
    pub history_updates: usize,
    pub workspace_details_full: usize,
    pub workspace_details_scoped: usize,
}
thread_local! { static COUNTS: Cell<Counts> = const { Cell::new(Counts { branches: 0, agents: 0, owner_candidates: 0, external_candidates: 0, history_updates: 0, workspace_details_full: 0, workspace_details_scoped: 0 }) }; }

pub(in crate::app) fn workspace_details(full: bool) {
    COUNTS.with(|held| {
        let mut count = held.get();
        if full {
            count.workspace_details_full += 1;
        } else {
            count.workspace_details_scoped += 1;
        }
        held.set(count);
    });
}

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let previous = COUNTS.replace(Counts::default());
    let value = work();
    (value, COUNTS.replace(previous))
}
pub(in crate::app) fn branch() {
    COUNTS.with(|held| {
        let mut count = held.get();
        count.branches += 1;
        held.set(count)
    });
}
pub(in crate::app) fn agent() {
    COUNTS.with(|held| {
        let mut count = held.get();
        count.agents += 1;
        held.set(count)
    });
}
pub(in crate::app) fn owner_candidate() {
    COUNTS.with(|held| {
        let mut count = held.get();
        count.owner_candidates += 1;
        held.set(count)
    });
}

pub(in crate::app) fn external_candidate() {
    COUNTS.with(|held| {
        let mut count = held.get();
        count.external_candidates += 1;
        held.set(count)
    });
}

pub(in crate::app) fn history_update() {
    COUNTS.with(|held| {
        let mut count = held.get();
        count.history_updates += 1;
        held.set(count)
    });
}

// Actual dependency-scan and row construction work for detail batching.
pub(in crate::app) mod detail_scans;

// Actual compaction wire rendering used to bound stable-layout capture work.
pub(in crate::app) mod policy_observations;
