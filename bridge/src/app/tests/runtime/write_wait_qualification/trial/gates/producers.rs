//! Final joined producer accounting; no synthetic admission/refusal counters.
use super::*;

pub(in super::super) fn validate_final_producers(
    _snapshot: &ProbeSnapshot,
    _offer_window: Duration,
    _observe: &Observation,
) {
    // TESTFIRST: the existing normal finish does not validate this final report.
}

#[path = "producers/tests.rs"]
mod tests;
