//! Actual index loops and active-owner source discovery, independent of helpers.
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Output {
    pub dirty: BTreeSet<String>,
    pub active: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub(in crate::app) struct Counts {
    pub index_builds: usize,
    pub index_owners: usize,
    pub discoveries: usize,
    pub discovery_owners: usize,
    pub registry_tabs: usize,
    pub indexed_by_owner: BTreeMap<String, usize>,
    pub discovered_by_owner: BTreeMap<String, usize>,
    pub outputs: Vec<Output>,
}

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static DISCOVERING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: RefCell<Counts> = const { RefCell::new(Counts {
        index_builds: 0, index_owners: 0, discoveries: 0,
        discovery_owners: 0, registry_tabs: 0, outputs: Vec::new(),
        indexed_by_owner: BTreeMap::new(), discovered_by_owner: BTreeMap::new(),
    }) };
}

fn count(update: impl FnOnce(&mut Counts)) {
    if ENABLED.get() {
        COUNTS.with_borrow_mut(update);
    }
}

pub(in crate::app) fn index_build() {
    count(|work| work.index_builds += 1);
}

pub(in crate::app) fn index_owner(owner: &str) {
    count(|work| {
        work.index_owners += 1;
        *work.indexed_by_owner.entry(owner.into()).or_default() += 1;
    });
}

pub(in crate::app) fn discovery_owner(owner: &str) {
    count(|work| {
        work.discovery_owners += 1;
        *work.discovered_by_owner.entry(owner.into()).or_default() += 1;
    });
}

pub(in crate::app) fn registry_tab() {
    if DISCOVERING.get() {
        count(|work| work.registry_tabs += 1);
    }
}

pub(in crate::app) fn outputs(dirty: &BTreeSet<String>, active: &BTreeSet<String>) {
    count(|work| {
        work.outputs.push(Output {
            dirty: dirty.clone(),
            active: active.clone(),
        });
    });
}

pub(in crate::app) struct Discovery {
    previous: bool,
}

impl Drop for Discovery {
    fn drop(&mut self) {
        DISCOVERING.set(self.previous);
    }
}

pub(in crate::app) fn discover() -> Discovery {
    count(|work| work.discoveries += 1);
    Discovery {
        previous: DISCOVERING.replace(ENABLED.get()),
    }
}

struct Measurement {
    previous: Counts,
    enabled: bool,
    discovering: bool,
}

impl Drop for Measurement {
    fn drop(&mut self) {
        COUNTS.replace(std::mem::take(&mut self.previous));
        ENABLED.set(self.enabled);
        DISCOVERING.set(self.discovering);
    }
}

pub(in crate::app) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let restore = Measurement {
        previous: COUNTS.take(),
        enabled: ENABLED.replace(true),
        discovering: DISCOVERING.replace(false),
    };
    let result = work();
    let counts = COUNTS.take();
    drop(restore);
    (result, counts)
}
