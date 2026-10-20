//! Thread-local structural diagnostics for producer input traversal.
use std::cell::RefCell;
use std::collections::BTreeMap;

type Visits = BTreeMap<(&'static str, String), usize>;
thread_local! { static VISITS: RefCell<Visits> = const { RefCell::new(BTreeMap::new()) }; }

pub(in crate::app) fn record(family: &'static str, key: &str) {
    VISITS.with_borrow_mut(|visits| *visits.entry((family, key.into())).or_default() += 1);
}

pub(in crate::app) fn take() -> Visits {
    VISITS.with_borrow_mut(std::mem::take)
}
