//! Full completed-frame holds, including unmatched and background observations.
use super::*;

const FULL_FRAME_HOLD_MAX_US: u64 = 200_000;

#[derive(Debug, Default, PartialEq, Eq)]
pub(in super::super) struct FrameHoldAudit {
    pub unique_frames: usize,
    pub over_limit_frames: usize,
    pub max_held_us: u64,
}

/// Gate each complete collector observation once by observer_id, including
/// unmatched/background records. Require full held_us strictly below
/// 200ms; finite-service durations never reduce this measurement.
pub(in super::super) fn validate_frame_holds(
    rows: &[Row],
    other_frames: &[FrameTrace],
    observe: &Observation,
) -> FrameHoldAudit {
    let mut seen = BTreeSet::new();
    let mut audit = FrameHoldAudit::default();
    for frame in rows.iter().flat_map(|row| &row.frames).chain(other_frames) {
        if !seen.insert(frame.observer_id) {
            continue;
        }
        audit.unique_frames += 1;
        audit.max_held_us = audit.max_held_us.max(frame.held_us);
        if frame.held_us >= FULL_FRAME_HOLD_MAX_US {
            audit.over_limit_frames += 1;
            record_hold_violation(frame, observe);
        }
    }
    audit
}

fn record_hold_violation(frame: &FrameTrace, observe: &Observation) {
    let violation = Violation {
        original_id: frame.original_ids.first().cloned(),
        kind: "app_lock_full_frame_hold_max".into(),
        observed_at_us: frame.sampled_at_us,
        measured_us: Some(frame.held_us),
        limit_us: Some(FULL_FRAME_HOLD_MAX_US),
        detail: format!(
            "observer_id={}; protocol={}; method={}; original_ids={:?}; full held_us must be strictly below {FULL_FRAME_HOLD_MAX_US}; finite service is not deducted",
            frame.observer_id, frame.protocol, frame.method, frame.original_ids
        ),
    };
    // The ordinary original/kind latch would collapse distinct frames for one
    // original or multiple background frames. Latch this collector observation
    // instead, while preserving the common violation kind in the raw report.
    let key = (
        violation.original_id.clone(),
        format!("{}:observer_id={}", violation.kind, frame.observer_id),
    );
    let mut ledger = observe.ledger.lock().unwrap();
    if ledger.latched.insert(key) {
        ledger.violations.push(violation.clone());
        let _ = observe.wake.send(violation);
    }
}

#[cfg(test)]
#[path = "frames/tests.rs"]
mod tests;
