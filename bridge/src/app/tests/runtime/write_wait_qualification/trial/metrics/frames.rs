//! Exact per-observation Spent values, with no invented acquisition stamps.
use super::*;
pub(super) fn append(row: &Row, samples: &mut MetricSamples, seen: &mut BTreeSet<u64>) {
    for frame in &row.frames {
        if !seen.insert(frame.observer_id) {
            continue;
        }
        append_frame(row, frame, samples);
    }
    for service in &row.services {
        push(samples, row, "finite_service_actual", service.actual_us);
        push(
            samples,
            row,
            "finite_registry_interval",
            service
                .finite_service
                .end_us
                .saturating_sub(service.finite_service.begin_us),
        );
        if let Some(operation) = service.operation_before_padding_us {
            push(samples, row, "operation_before_padding", operation);
        }
    }
}
fn append_frame(row: &Row, frame: &FrameTrace, samples: &mut MetricSamples) {
    for (label, value) in [
        ("frame_queued", frame.queued_us),
        ("frame_lock_wait", frame.lock_wait_us),
        ("frame_held", frame.held_us),
        ("frame_total", frame.total_us),
    ] {
        push(samples, row, label, value);
    }
    if let Some(value) = frame.permit_wait_us {
        push(samples, row, "frame_permit", value);
    }
    if let Some(value) = frame.pool_wait_us {
        push(samples, row, "frame_pool_wait", value);
    }
    // Same observation only; never combine independent extrema.
    if let Some(permit) = frame.permit_wait_us {
        push(
            samples,
            row,
            "frame_queue_permit_lock_wait",
            frame.queued_us + permit + frame.lock_wait_us,
        );
    }
}
