//! Opt-in thread-local structural diagnostics for producer input traversal.
use std::cell::RefCell;
use std::collections::BTreeMap;

type Visits = BTreeMap<(&'static str, String), usize>;
thread_local! { static VISITS: RefCell<Option<Visits>> = const { RefCell::new(None) }; }

pub(in crate::app) fn record(family: &'static str, key: &str) {
    VISITS.with_borrow_mut(|active| {
        if let Some(visits) = active {
            *visits.entry((family, key.into())).or_default() += 1;
        }
    });
}

pub(in crate::app) fn start() {
    VISITS.with_borrow_mut(|active| {
        assert!(
            active.is_none(),
            "only one capture recorder per test thread"
        );
        *active = Some(BTreeMap::new());
    });
}

pub(in crate::app) fn take() -> Visits {
    VISITS.with_borrow_mut(|active| active.take().unwrap_or_default())
}
