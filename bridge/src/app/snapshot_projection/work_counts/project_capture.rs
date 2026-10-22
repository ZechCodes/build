//! Source counts at the real project_list_rows entry and native project loop.
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(in crate::app) struct Counts {
    pub captures: usize,
    pub projects: BTreeMap<String, usize>,
}

thread_local! {
    static ACTIVE: RefCell<Option<Counts>> = const { RefCell::new(None) };
}

pub(in crate::app) fn capture() {
    ACTIVE.with_borrow_mut(|active| {
        if let Some(active) = active {
            active.captures += 1;
        }
    });
}

pub(in crate::app) fn project(id: &str) {
    ACTIVE.with_borrow_mut(|active| {
        if let Some(active) = active {
            *active.projects.entry(id.into()).or_default() += 1;
        }
    });
}

struct Measurement(Option<Counts>);

impl Drop for Measurement {
    fn drop(&mut self) {
        ACTIVE.replace(self.0.take());
    }
}

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let restore = Measurement(ACTIVE.replace(Some(Counts::default())));
    let result = work();
    let counts = ACTIVE.take().unwrap();
    drop(restore);
    (result, counts)
}
