//! Count actual workspace policy JSON rendering, wherever publication invokes it.
use std::cell::Cell;

thread_local! { static WORKSPACE_RENDERS: Cell<usize> = const { Cell::new(0) }; }

pub(in crate::app) fn workspace_render() {
    WORKSPACE_RENDERS.with(|count| count.set(count.get() + 1));
}

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let previous = WORKSPACE_RENDERS.replace(0);
    let value = work();
    (value, WORKSPACE_RENDERS.replace(previous))
}
