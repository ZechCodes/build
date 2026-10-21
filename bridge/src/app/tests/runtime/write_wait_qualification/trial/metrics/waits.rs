//! Copy registry-calculated unions without reconstructing/subtracting spans.
use super::*;
pub(super) fn append(row: &Row, samples: &mut MetricSamples) {
    let Some(waits) = &row.waits else {
        return;
    };
    for (label, value) in [
        ("known_wait_union", waits.known_wait_union_us),
        ("sql_service_union", waits.sql_service_union_us),
        ("wait_plus_sql_union", waits.blocked_or_sql_union_us),
        ("conservative_wait_upper", waits.conservative_upper_us),
        ("own_external_union", waits.own_external_union_us),
        (
            "exclusive_active_external",
            waits.exclusive_active_external_us,
        ),
        ("max_chargeable_interval", waits.max_chargeable_interval_us),
    ] {
        push(samples, row, label, value);
    }
}
