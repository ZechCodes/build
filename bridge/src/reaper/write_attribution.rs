//! Causal classification of the condition, separately from its exclusion lock.
use crate::timing::write_wait::{
    current_context, IntervalCategory, WaitGuard, WaitKind, WriteContext,
};

pub(super) fn condition(cause: &Option<WriteContext>) -> Option<WaitGuard> {
    let current = current_context()?;
    let own = cause.as_ref().is_some_and(|cause| {
        current.same_registry(cause)
            && current
                .origins()
                .iter()
                .any(|id| cause.origins().contains(id))
    });
    Some(current.interval(if own {
        IntervalCategory::OwnExternal
    } else {
        IntervalCategory::KnownWait(WaitKind::Retirement)
    }))
}
