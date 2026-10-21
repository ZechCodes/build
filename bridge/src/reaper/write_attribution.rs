//! Per-original causal classification under the same live joined obligation.
use crate::timing::write_wait::{
    current_context, IntervalCategory, WaitGuard, WaitKind, WriteContext,
};

pub(super) fn condition(cause: &Option<WriteContext>) -> Option<Vec<WaitGuard>> {
    let current = current_context()?;
    let (own, predecessors): (Vec<_>, Vec<_>) = current.origins().iter().copied().partition(|id| {
        cause
            .as_ref()
            .is_some_and(|cause| current.same_registry(cause) && cause.origins().contains(id))
    });
    let mut guards = Vec::with_capacity(2);
    if !own.is_empty() {
        guards.push(current.interval_for(&own, IntervalCategory::OwnExternal));
    }
    if !predecessors.is_empty() {
        guards.push(current.interval_for(
            &predecessors,
            IntervalCategory::KnownWait(WaitKind::Retirement),
        ));
    }
    Some(guards)
}
