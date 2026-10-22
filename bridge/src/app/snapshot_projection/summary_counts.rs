//! Count repeated observations and summary searches, independent of timing noise.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct Counts {
    pub statuses: usize,
    pub joins: usize,
}

thread_local! { static COUNTS: Cell<Counts> = const { Cell::new(Counts { statuses: 0, joins: 0 }) }; }

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let previous = COUNTS.replace(Counts::default());
    let value = work();
    (value, COUNTS.replace(previous))
}

pub(in crate::app) fn status_visit() {
    COUNTS.with(|held| {
        let mut count = held.get();
        count.statuses += 1;
        held.set(count);
    });
}

pub(in crate::app) fn join_visit() {
    COUNTS.with(|held| {
        let mut count = held.get();
        count.joins += 1;
        held.set(count);
    });
}
