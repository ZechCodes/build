//! TEST-FIRST seam: unit fixture data is not native qualification evidence.
//! Intentionally unimplemented until the actual controls establish RED.
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
/// The empty result is deliberate TEST-FIRST scaffolding, not a passed audit.
pub(in super::super) fn validate_frame_holds(
    _rows: &[Row],
    _other_frames: &[FrameTrace],
    _observe: &Observation,
) -> FrameHoldAudit {
    FrameHoldAudit::default()
}

#[cfg(test)]
#[path = "frames/tests.rs"]
mod tests;
