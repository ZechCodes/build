//! Disabled-by-default logical witnesses on one synchronous test worker.
//! Counts real sites, not allocator, VM, time or process-wide filesystem work.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Phase {
    #[default]
    Outside,
    Construction,
    Membership,
    SummaryConstruction,
    ComputeStore,
}
#[derive(Debug, Default)]
pub(crate) struct PhaseCounts {
    pub workspace_lists: usize,
    pub workspace_rows: usize,
    pub canonicalizations: BTreeMap<PathBuf, usize>,
    pub notifications: usize,
    pub jobs: usize,
    pub passes: usize,
}
#[derive(Debug)]
pub(crate) struct RootsInvocation {
    pub phase: Phase,
    pub workspace_lists: usize,
    pub workspace_rows: usize,
    pub canonicalizations: BTreeMap<PathBuf, usize>,
    pub returned: bool,
}
#[derive(Debug, Default)]
pub(crate) struct Counts {
    pub phases: BTreeMap<Phase, PhaseCounts>,
    pub invocations: Vec<RootsInvocation>,
}
#[derive(Default)]
struct Measurement {
    counts: Counts,
    phase: Phase,
    invocation: Option<usize>,
}
thread_local! {
    static ACTIVE: RefCell<Option<Measurement>> = const { RefCell::new(None) };
}
fn change(work: impl FnOnce(&mut Measurement)) {
    ACTIVE.with(|active| {
        if let Some(held) = active.borrow_mut().as_mut() {
            work(held);
        }
    });
}
struct Restore(Option<Measurement>);
impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = self.0.take());
    }
}
pub(crate) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let previous = ACTIVE.with(|active| active.replace(Some(Measurement::default())));
    let _restore = Restore(previous);
    let value = work();
    let counts = ACTIVE.with(|active| active.borrow_mut().take().unwrap().counts);
    (value, counts)
}
pub(crate) struct PhaseGuard(Option<Phase>);
impl Drop for PhaseGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            set_phase(previous);
        }
    }
}
pub(crate) fn phase(next: Phase) -> PhaseGuard {
    let mut previous = None;
    change(|held| {
        previous = Some(held.phase);
        held.phase = next;
        if next == Phase::ComputeStore {
            held.counts.phases.entry(next).or_default().passes += 1;
        }
    });
    PhaseGuard(previous)
}
pub(crate) fn set_phase(next: Phase) {
    change(|held| held.phase = next);
}
pub(crate) struct InvocationGuard {
    previous: Option<usize>,
    index: Option<usize>,
}
impl Drop for InvocationGuard {
    fn drop(&mut self) {
        if let Some(index) = self.index {
            change(|held| {
                held.counts.invocations[index].returned = !std::thread::panicking();
                held.invocation = self.previous;
            });
        }
    }
}
pub(crate) fn roots_invocation() -> InvocationGuard {
    let mut guard = InvocationGuard {
        previous: None,
        index: None,
    };
    change(|held| {
        guard.previous = held.invocation;
        let index = held.counts.invocations.len();
        held.counts.invocations.push(RootsInvocation {
            phase: held.phase,
            workspace_lists: 0,
            workspace_rows: 0,
            canonicalizations: BTreeMap::new(),
            returned: false,
        });
        held.invocation = Some(index);
        guard.index = Some(index);
    });
    guard
}
pub(crate) fn workspace_list(rows: usize) {
    change(|held| {
        let phase = held.counts.phases.entry(held.phase).or_default();
        phase.workspace_lists += 1;
        phase.workspace_rows += rows;
        if let Some(index) = held.invocation {
            let invocation = &mut held.counts.invocations[index];
            invocation.workspace_lists += 1;
            invocation.workspace_rows += rows;
        }
    });
}
pub(crate) fn canonicalize(path: &Path) {
    change(|held| {
        *held
            .counts
            .phases
            .entry(held.phase)
            .or_default()
            .canonicalizations
            .entry(path.to_owned())
            .or_default() += 1;
        if let Some(index) = held.invocation {
            *held.counts.invocations[index]
                .canonicalizations
                .entry(path.to_owned())
                .or_default() += 1;
        }
    });
}
pub(crate) fn notification() {
    change(|held| {
        held.counts
            .phases
            .entry(held.phase)
            .or_default()
            .notifications += 1
    });
}
pub(crate) fn job() {
    change(|held| held.counts.phases.entry(held.phase).or_default().jobs += 1);
}
