//! Exact producer work counts used by publication regression tests.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct Counts {
    pub branches: usize,
    pub agents: usize,
    pub owner_candidates: usize,
    pub external_candidates: usize,
}
thread_local! { static COUNTS: Cell<Counts> = const { Cell::new(Counts { branches: 0, agents: 0, owner_candidates: 0, external_candidates: 0 }) }; }

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
