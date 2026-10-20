//! Exact capture work, including list allocation inputs and normalized paths.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counts {
    pub workspace_lists: usize,
    pub workspace_rows: usize,
    pub owner_candidates: usize,
    pub scope_candidates: usize,
    pub task_candidates: usize,
    pub observed_roots: usize,
}

thread_local! {
    static COUNTS: Cell<Counts> = const { Cell::new(Counts {
        workspace_lists: 0, workspace_rows: 0, owner_candidates: 0,
        scope_candidates: 0, task_candidates: 0, observed_roots: 0,
    }) };
}

pub(crate) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let previous = COUNTS.replace(Counts::default());
    let value = work();
    (value, COUNTS.replace(previous))
}

fn change(update: impl FnOnce(&mut Counts)) {
    COUNTS.with(|held| {
        let mut counts = held.get();
        update(&mut counts);
        held.set(counts);
    });
}

pub(crate) fn workspace_list(rows: usize) {
    change(|counts| {
        counts.workspace_lists += 1;
        counts.workspace_rows += rows;
    });
}

pub(crate) fn owner_candidate() {
    change(|counts| counts.owner_candidates += 1);
}

pub(crate) fn scope_candidate() {
    change(|counts| counts.scope_candidates += 1);
}

pub(crate) fn task_candidate() {
    change(|counts| counts.task_candidates += 1);
}

pub(crate) fn observed_root() {
    change(|counts| counts.observed_roots += 1);
}
