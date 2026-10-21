//! Mandatory coordinator review when SQL alone prevents certification.
use super::*;
pub(in super::super) fn sql_only_budget_review_required(
    rows: &[Row],
    violations: &[Violation],
) -> bool {
    let sql_prevents_certification =
        rows.iter()
            .filter_map(|row| row.waits.as_ref())
            .any(|waits| {
                waits.known_wait_union_us <= WRITE_BOUND_US
                    && (waits.blocked_or_sql_union_us > WRITE_BOUND_US
                        || waits.conservative_upper_us > WRITE_BOUND_US)
            })
            || violations
                .iter()
                .any(|v| v.kind == "wait_plus_sql_union" || v.kind == "conservative_wait_upper");
    let independent_non_sql_budget_failure =
        rows.iter()
            .filter_map(|row| row.waits.as_ref())
            .any(|waits| {
                waits.known_wait_union_us > WRITE_BOUND_US
                    || waits.max_chargeable_interval_us > WRITE_BOUND_US
            })
            || violations.iter().any(|v| {
                matches!(
                    v.kind.as_str(),
                    "known_wait_union"
                        | "max_chargeable_interval"
                        | "unfinished_caller"
                        | "intended_to_decoded_final"
                        | "unfinished_finite_service"
                        | "finite_service_ceiling"
                        | "finite_registry_interval_ceiling"
                )
            });
    sql_prevents_certification && !independent_non_sql_budget_failure
}
