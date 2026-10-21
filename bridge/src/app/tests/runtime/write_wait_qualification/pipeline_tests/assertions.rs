//! Assertions over actual completed control evidence.
use super::*;

pub(super) fn assert_transport_and_cleanup(report: &PipelineControlReport, retained: &Path) {
    assert!(
        !report.qualification,
        "short control must remain NoQualification"
    );
    assert!(
        report.schema.contains("NoQualification"),
        "{}",
        report.schema
    );
    assert_eq!(report.offer_window_us, 2_000_000);
    assert!(
        report.invalid_infrastructure.is_none(),
        "infrastructure={:?}; artifacts={}",
        report.invalid_infrastructure,
        retained.display()
    );
    assert!(
        report.transport_schedule_completed,
        "actual transport schedule incomplete; artifacts={}",
        retained.display()
    );
    assert!(
        report.cleanup_drained && report.backend_closed,
        "actual drain/close not proven; artifacts={}",
        retained.display()
    );
    assert!(
        report.counts_at_offer_end.is_some(),
        "full two-second offer window was not observed"
    );
    for (traffic, expected) in [
        (Traffic::Write, 80),
        (Traffic::Exempt, 200),
        (Traffic::Read, 32),
    ] {
        let counts = report
            .final_counts
            .get(&traffic)
            .expect("declared traffic cohort");
        assert_eq!(
            (
                counts.intended,
                counts.offered,
                counts.sent,
                counts.completed
            ),
            (expected, expected, expected, expected),
            "{traffic:?}: {counts:?}; artifacts={}",
            retained.display()
        );
        assert_eq!(
            (
                counts.not_offered,
                counts.pending_unsubmitted,
                counts.sent_unfinished,
                counts.admitted_unfinished
            ),
            (0, 0, 0, 0),
            "{traffic:?}: {counts:?}"
        );
    }
    let probe = &report.last_probe;
    assert!(
        probe.registry.is_empty()
            && probe.deferred_active.is_empty()
            && probe.receiver_pending.is_empty()
            && probe.producer_active.is_empty(),
        "last actual probe not drained: {probe:?}"
    );
    assert!(
        probe.producer_failures.is_empty() && probe.caller_breaches.is_empty(),
        "last probe failed: {probe:?}"
    );
    assert_eq!(
        probe.producer_counts.len(),
        4,
        "all four actual producer handles must be reported"
    );
    for counts in probe.producer_counts.values() {
        assert_eq!(counts.failed, 0, "actual background producer failure");
        assert_eq!(
            counts.intended, counts.started,
            "missed actual background callback"
        );
        assert_eq!(
            counts.started, counts.completed,
            "unfinished actual background callback"
        );
    }
}

pub(super) fn assert_complete_rows(rows: &[Row], report: &PipelineControlReport, retained: &Path) {
    assert_eq!(
        rows.len(),
        312,
        "actual per-ID archive cardinality; artifacts={}",
        retained.display()
    );
    let unique = rows
        .iter()
        .map(|row| row.slot.original_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), 312, "no original may be replaced or repeated");
    let coding = rows
        .iter()
        .filter(|row| row.slot.route == Route::AgentMcp)
        .map(|row| row.slot.caller)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        coding,
        (4..12).collect(),
        "exact eight actual Coding callers, no fabricated Router"
    );
    let gaps = report
        .explicit_proof_gaps
        .iter()
        .map(|gap| (gap.original_id.as_str(), gap.detail.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for row in rows {
        assert!(
            row.slot.due_us < 2_000_000,
            "an original escaped the immutable short window"
        );
        for stage in [
            Stage::Offered,
            Stage::WireSent,
            Stage::ParsedFinalReply,
            Stage::Completed,
        ] {
            assert!(
                row.milestones_us.contains_key(&stage),
                "{} missing actual {stage:?}",
                row.slot.original_id
            );
        }
        assert_ne!(
            row.slot.caller, 12,
            "no passive Router in this explicit short control"
        );
        if row.slot.traffic == Traffic::Read {
            assert_eq!(row.slot.route, Route::SpaRpc, "zero MCP read control");
        }
        if let Some(error) = &row.error {
            let detail = gaps
                .get(row.slot.original_id.as_str())
                .expect("row failure is not an explicit retained proof gap");
            assert_eq!(error, &format!("explicit missing proof: {detail}"));
        }
        assert_finite_tail(row);
    }
    assert_eq!(
        rows.iter()
            .filter(|row| row.slot.traffic == Traffic::Write)
            .count(),
        80
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.slot.traffic == Traffic::Exempt)
            .count(),
        200
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.slot.traffic == Traffic::Read)
            .count(),
        32
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.slot.finite_target_us.is_some())
            .count(),
        16
    );
}

fn assert_finite_tail(row: &Row) {
    let Some(target) = row.slot.finite_target_us else {
        return;
    };
    let parsed = row.milestones_us[&Stage::ParsedFinalReply];
    assert!(
        parsed >= row.slot.due_us + target,
        "{} lost its real paired finite tail",
        row.slot.original_id
    );
    assert_eq!(
        row.services.len(),
        1,
        "{} needs its one actual finite service",
        row.slot.original_id
    );
    let service = &row.services[0];
    assert_eq!(service.target_us, target);
    assert_eq!(Some(service.ceiling_us), row.slot.finite_ceiling_us);
    assert!(
        service.succeeded
            && !service.abandoned
            && service.actual_us >= target
            && service.actual_us <= service.ceiling_us,
        "actual finite service violates its unchanged budget: {service:?}"
    );
}

pub(super) fn assert_proof_only_failure(report: &PipelineControlReport, retained: &Path) {
    for violation in &report.violations {
        let explicit_gap = violation.kind == "missing_control_proof";
        let cursor_gap = matches!(violation.kind.as_str(), "archive" | "final_archive")
            && violation
                .detail
                .contains("actual work drained but incremental completion proof RED");
        assert!(
            explicit_gap || cursor_gap,
            "unexpected non-proof violation before intended RED: {violation:?}; artifacts={}",
            retained.display()
        );
    }
    // Success is still asserted by the final test assertion. This helper only
    // separates genuine unexpected failures from the named proof implementation.
}

pub(super) fn assert_native_identity(rows: &[Row], native: &serde_json::Value) {
    let planned = native["planned_client_originals"]
        .as_array()
        .expect("actual immutable native plan export");
    assert_eq!(planned.len(), 312);
    let planned = planned
        .iter()
        .map(|id| id.as_str().expect("actual original string"))
        .collect::<BTreeSet<_>>();
    let archived = rows
        .iter()
        .map(|row| row.slot.original_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        planned, archived,
        "every immutable native-plan original must survive unchanged"
    );
    let native_rows = native["rows"]
        .as_array()
        .expect("full actual native registry archive");
    for row in native_rows {
        assert_eq!(
            row["root_finished"], true,
            "unfinished actual native row: {row}"
        );
        assert_eq!(
            row["cancelled"], false,
            "cancelled actual native row: {row}"
        );
        assert_eq!(
            row["abandoned"], false,
            "abandoned actual native row: {row}"
        );
        assert!(row["active"]
            .as_array()
            .expect("actual active intervals")
            .is_empty());
        assert!(row["outstanding"]
            .as_array()
            .expect("actual outstanding obligations")
            .is_empty());
    }
}

/// Tightens the same existing 312 smoke after its observed proof-boundary RED.
pub(super) fn assert_all_native_write_proofs(rows: &[Row], report: &PipelineControlReport) {
    assert!(
        report.explicit_proof_gaps.is_empty(),
        "native write proof gaps remain"
    );
    assert!(
        report.violations.is_empty(),
        "native violations: {:?}",
        report.violations
    );
    let writes = rows
        .iter()
        .filter(|row| row.slot.traffic == Traffic::Write)
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 80);
    for row in writes {
        assert!(
            row.error.is_none(),
            "{}: {:?}",
            row.slot.original_id,
            row.error
        );
        let proof = row
            .proof
            .as_ref()
            .expect("actual write ExecutionProof missing");
        assert_eq!(proof.original_id, row.slot.original_id);
        assert_eq!(proof.method, row.recipe.method);
        assert!(proof.registry_root.is_some() && proof.mandatory_children_complete);
        assert!(proof.effect.is_some());
        assert!(row.milestones_us.contains_key(&Stage::EffectVerified));
        assert_decoded_credit_stamps(row);
        assert!(row.waits.as_ref().is_some_and(|waits| waits.complete));
    }
}

fn assert_decoded_credit_stamps(row: &Row) {
    let parsed = row.milestones_us[&Stage::ParsedFinalReply];
    assert_eq!(
        row.milestones_us.get(&Stage::DecodedReplyObserved),
        Some(&parsed),
        "{} must signal the actual parser timestamp, not its later proof return",
        row.slot.original_id,
    );
    let released = row
        .milestones_us
        .get(&Stage::CallerCreditReleased)
        .expect("every actual semantic write must return its caller credit at decode");
    assert!(
        *released >= parsed && *released <= row.milestones_us[&Stage::Completed],
        "{} must retain ordered parser/credit-return/completion observations",
        row.slot.original_id,
    );
}
