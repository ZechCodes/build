//! Required final archives and actual finite service evidence.
use super::*;
pub(in super::super) fn validate_final_rows(rows: &[Row], observe: &Observation) {
    // Clone only final evidence; never hold the ledger while recording failures.
    let other_frames = observe.ledger.lock().unwrap().other_frames.clone();
    super::frames::validate_frame_holds(rows, &other_frames, observe);
    for row in rows {
        validate_completed_archives(row, observe);
        if let Some(waits) = &row.waits {
            validate_wait_totals(row, waits, observe);
        }
        for service in &row.services {
            validate_service(row, service, observe);
        }
    }
}
fn validate_completed_archives(row: &Row, observe: &Observation) {
    let id = &row.slot.original_id;
    if row.recipe.needs_registry_root
        && row.milestones_us.contains_key(&Stage::Completed)
        && row.waits.as_ref().is_none_or(|waits| !waits.complete)
    {
        observe.fail(
            Some(id.clone()),
            "missing_final_wait_archive",
            "completed write lacks complete root/child union trace".into(),
        );
    }
    if row.recipe.needs_registry_root
        && row.milestones_us.contains_key(&Stage::Completed)
        && row.frames.is_empty()
    {
        observe.fail(
            Some(id.clone()),
            "missing_completed_frame",
            "complete write requires actual fullhold frame evidence".into(),
        );
    }
    if row.milestones_us.contains_key(&Stage::Completed)
        && row.slot.finite_ceiling_us.is_some()
        && !matching_declared_service(row)
    {
        observe.fail(Some(id.clone()), "planned_service_cardinality", "require exactly one actual finite sample and one matching registry FiniteService interval/original, with declared target/ceiling".into());
    }
}
fn matching_declared_service(row: &Row) -> bool {
    row.services.len() == 1
        && row.services.iter().any(|service| {
            Some(service.target_us) == row.slot.finite_target_us
                && Some(service.ceiling_us) == row.slot.finite_ceiling_us
        })
}
fn validate_wait_totals(row: &Row, waits: &UnionTrace, observe: &Observation) {
    for (kind, value) in [
        ("known_wait_union", waits.known_wait_union_us),
        ("wait_plus_sql_union", waits.blocked_or_sql_union_us),
        ("conservative_wait_upper", waits.conservative_upper_us),
        ("max_chargeable_interval", waits.max_chargeable_interval_us),
    ] {
        if value > WRITE_BOUND_US {
            observe.violation(Violation { original_id: Some(row.slot.original_id.clone()), kind: kind.into(), observed_at_us: observe.now_us(), measured_us: Some(value), limit_us: Some(WRITE_BOUND_US), detail: "actual registry aggregate; SQL-only cause requires coordinator decision, no automatic relaxation".into() });
        }
    }
}
fn validate_service(row: &Row, service: &ServiceTrace, observe: &Observation) {
    let id = &row.slot.original_id;
    if service.finite_service.end_us < service.finite_service.begin_us {
        observe.fail(
            Some(id.clone()),
            "invalid_service_interval",
            service.registry_interval_id.clone(),
        );
        return;
    }
    validate_service_ceilings(row, service, observe);
    if !service.succeeded || service.abandoned || service.actual_us < service.target_us {
        observe.fail(
            Some(id.clone()),
            "finite_service_outcome",
            "actual finite operation failed/abandoned or did not preserve declared target".into(),
        );
    }
    if service
        .operation_before_padding_us
        .is_none_or(|operation| operation > service.actual_us)
    {
        observe.fail(
            Some(id.clone()),
            "missing_operation_before_padding",
            "retain actual operation duration separately from padded finite service".into(),
        );
    }
    if !service_joined(row, service) {
        observe.fail(
            Some(id.clone()),
            "unpaired_service_original",
            service.registry_interval_id.clone(),
        );
    }
}
fn validate_service_ceilings(row: &Row, service: &ServiceTrace, observe: &Observation) {
    for (kind, actual) in [
        ("finite_service_ceiling", service.actual_us),
        (
            "finite_registry_interval_ceiling",
            service.finite_service.end_us - service.finite_service.begin_us,
        ),
    ] {
        if actual > service.ceiling_us {
            observe.violation(Violation {
                original_id: Some(row.slot.original_id.clone()),
                kind: kind.into(),
                observed_at_us: observe.now_us(),
                measured_us: Some(actual),
                limit_us: Some(service.ceiling_us),
                detail: service.registry_interval_id.clone(),
            });
        }
    }
}
fn service_joined(row: &Row, service: &ServiceTrace) -> bool {
    row.frames
        .iter()
        .any(|frame| frame.original_ids.contains(&row.slot.original_id))
        && row.waits.as_ref().is_some_and(|waits| {
            waits
                .write_ids
                .iter()
                .any(|origin| service.registry_origins.contains(origin))
        })
}
