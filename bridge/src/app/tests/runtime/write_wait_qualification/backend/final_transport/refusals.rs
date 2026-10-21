//! Only closed final native refusal tokens can discharge a reply-only failure.
use super::*;

#[cfg(test)]
#[path = "refusals/tests.rs"]
mod tests;

pub(super) struct Audit {
    pub errors: Vec<String>,
    pub discharged: Vec<Value>,
}

pub(super) fn partition(
    _plan: &[Slot],
    _reconcile: Option<&FinalReconciliation>,
    mut strict_errors: Vec<String>,
    _refused: &VerifiedRefusals,
    _anchor: Instant,
) -> Audit {
    strict_errors.push("verified_refusal_transport_partition_TESTFIRST_pending".into());
    Audit {
        errors: strict_errors,
        discharged: Vec::new(),
    }
}
