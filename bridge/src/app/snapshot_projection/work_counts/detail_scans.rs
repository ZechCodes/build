//! Count actual detail dependency enumeration and row construction.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct Counts {
    pub registry_scans: usize,
    pub registry_rows: usize,
    pub previous_bases: usize,
    pub base_renders: usize,
    pub variant_renders: usize,
    pub tombstone_keys: usize,
}

thread_local! { static COUNTS: Cell<Counts> = const { Cell::new(Counts {
    registry_scans: 0, registry_rows: 0, previous_bases: 0,
    base_renders: 0, variant_renders: 0, tombstone_keys: 0,
}) }; }

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let previous = COUNTS.replace(Counts::default());
    let value = work();
    (value, COUNTS.replace(previous))
}

fn count(update: impl FnOnce(&mut Counts)) {
    COUNTS.with(|held| {
        let mut value = held.get();
        update(&mut value);
        held.set(value);
    });
}

pub(in crate::app) fn registry_scan() {
    count(|value| value.registry_scans += 1);
}
pub(in crate::app) fn registry_row() {
    count(|value| value.registry_rows += 1);
}
pub(in crate::app) fn previous_base() {
    count(|value| value.previous_bases += 1);
}
pub(in crate::app) fn base_render() {
    count(|value| value.base_renders += 1);
}
pub(in crate::app) fn variant_render() {
    count(|value| value.variant_renders += 1);
}
pub(in crate::app) fn tombstone_key() {
    count(|value| value.tombstone_keys += 1);
}
