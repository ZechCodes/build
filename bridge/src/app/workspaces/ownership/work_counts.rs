//! Actual structural construction and native-input inspection work.
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct Counts {
    pub input_builds: usize,
    pub map_builds: usize,
    pub render_index_builds: usize,
    pub source_rows: usize,
}

thread_local! {
    static COUNTS: Cell<Counts> = const { Cell::new(Counts {
        input_builds: 0, map_builds: 0, render_index_builds: 0, source_rows: 0,
    }) };
}

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let previous = COUNTS.replace(Counts::default());
    let value = work();
    (value, COUNTS.replace(previous))
}

fn change(update: impl FnOnce(&mut Counts)) {
    COUNTS.with(|slot| {
        let mut value = slot.get();
        update(&mut value);
        slot.set(value);
    });
}

pub(in crate::app::workspaces) fn input_build() {
    change(|counts| counts.input_builds += 1);
}

pub(in crate::app::workspaces) fn map_build() {
    change(|counts| counts.map_builds += 1);
}

pub(in crate::app::workspaces) fn render_index_build() {
    change(|counts| counts.render_index_builds += 1);
}

pub(in crate::app::workspaces) fn source_row() {
    change(|counts| counts.source_rows += 1);
}
