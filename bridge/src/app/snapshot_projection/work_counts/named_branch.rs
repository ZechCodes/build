//! Source attribution surrounds whole producer calls, including future helpers.
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(in crate::app) enum Source {
    Winner,
    RunState,
    Reference,
}

#[derive(Clone)]
struct Tag {
    owner: String,
    source: Source,
}

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::app) struct Visits {
    pub winner: usize,
    pub state: usize,
    pub reference: usize,
}

pub(in crate::app) type Counts = BTreeMap<String, Visits>;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static ACTIVE: RefCell<Option<Tag>> = const { RefCell::new(None) };
    static COUNTS: RefCell<Counts> = const { RefCell::new(BTreeMap::new()) };
}

pub(in crate::app) struct SourceScope {
    previous: Option<Tag>,
    enabled: bool,
}

impl Drop for SourceScope {
    fn drop(&mut self) {
        if self.enabled {
            ACTIVE.replace(self.previous.take());
        }
    }
}

pub(in crate::app) fn source(owner: &str, source: Source) -> SourceScope {
    let enabled = ENABLED.get();
    let previous = enabled
        .then(|| {
            ACTIVE.replace(Some(Tag {
                owner: owner.into(),
                source,
            }))
        })
        .flatten();
    SourceScope { previous, enabled }
}

struct Measurement {
    previous_counts: Counts,
    previous_active: Option<Tag>,
    previous_enabled: bool,
}

impl Drop for Measurement {
    fn drop(&mut self) {
        COUNTS.replace(std::mem::take(&mut self.previous_counts));
        ACTIVE.replace(self.previous_active.take());
        ENABLED.set(self.previous_enabled);
    }
}

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let restore = Measurement {
        previous_counts: COUNTS.take(),
        previous_active: ACTIVE.take(),
        previous_enabled: ENABLED.replace(true),
    };
    let value = work();
    let counts = COUNTS.take();
    drop(restore);
    (value, counts)
}

#[inline]
pub(in crate::app) fn indexed_value() {
    if ENABLED.get() {
        ACTIVE.with_borrow(|tag| {
            if let Some(tag) = tag {
                COUNTS.with_borrow_mut(|counts| {
                    let value = counts.entry(tag.owner.clone()).or_default();
                    match tag.source {
                        Source::Winner => value.winner += 1,
                        Source::RunState => value.state += 1,
                        Source::Reference => value.reference += 1,
                    }
                });
            }
        });
    }
}
