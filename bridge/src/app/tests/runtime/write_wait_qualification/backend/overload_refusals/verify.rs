use super::*;
use crate::timing::write_wait::IntervalCategory;
use std::collections::BTreeSet;
use std::time::Duration;

pub(super) fn all(
    expected: &BTreeMap<String, Expected>,
    state: &State,
    rows: &[WriteSnapshot],
    intervals: &[CompletedInterval],
    observe: &Observation,
    accepted: Option<&AcceptedRefusals>,
) -> Result<VerifiedRefusals, String> {
    let mut errors = state.errors.clone();
    let mut verified = BTreeMap::new();
    let candidates = state
        .candidates
        .keys()
        .chain(state.quarantined.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let accepted = accepted.ok_or("final persisted native-quiet acceptance audit missing")?;
    for original in accepted.originals() {
        if !candidates.contains(original) {
            errors.push(format!(
                "empty acceptance proof has no refusal candidate: {original}"
            ));
        }
    }
    for original in candidates {
        match one(
            &original,
            (expected, state),
            rows,
            intervals,
            observe,
            accepted,
        ) {
            Ok(row) => {
                verified.insert(original, row);
            }
            Err(error) => errors.push(format!("{original}: {error}")),
        }
    }
    if errors.is_empty() {
        // TESTFIRST: validate actual prerequisites but do not issue the closed
        // capability until the positive native target is observed RED.
        Err("verified_refusal_native_partition_TESTFIRST_pending".into())
    } else {
        Err(errors.join("; "))
    }
}
fn one(
    original: &str,
    scope: (&BTreeMap<String, Expected>, &State),
    rows: &[WriteSnapshot],
    intervals: &[CompletedInterval],
    observe: &Observation,
    accepted: &AcceptedRefusals,
) -> Result<VerifiedRefusal, String> {
    let (expected, state) = scope;
    let expected = expected
        .get(original)
        .ok_or("unknown refusal eligibility")?;
    let candidates = state
        .candidates
        .get(original)
        .ok_or("native cancellation lacks actual busy reply")?;
    let [candidate] = candidates.as_slice() else {
        return Err("duplicate refusal candidates".into());
    };
    let quarantined = state
        .quarantined
        .get(original)
        .ok_or("refusal lacks actual terminal cursor cancellation")?;
    let [quarantined] = quarantined.as_slice() else {
        return Err("duplicate cancelled cursor rows".into());
    };
    let native = rows
        .iter()
        .filter(|row| row.original_id == original)
        .collect::<Vec<_>>();
    let [native] = native.as_slice() else {
        return Err("missing or duplicate final native original".into());
    };
    if native.id != quarantined.id
        || native.method != expected.method
        || native.method != quarantined.method
        || native.intake != quarantined.intake
        || native.admitted_at != quarantined.admitted_at
        || native.acknowledged_at != quarantined.acknowledged_at
        || native.completed_at != quarantined.completed_at
        || !accepted.contains(original)
    {
        return Err("native identity/timestamps or final zero-acceptance receipt changed".into());
    }
    let classified = native
        .admitted_at
        .ok_or("actual classified intake marker missing")?;
    if candidate.intended != observe.original_due(&expected.slot)
        || candidate.sent < candidate.intended
        || native.intake < candidate.sent
        || classified < native.intake
        || candidate.decoded < classified
        || candidate.decoded < candidate.sent
        || candidate.decoded.duration_since(candidate.intended) > Duration::from_millis(100)
    {
        return Err("actual refusal chronology or original 100ms deadline invalid".into());
    }
    if intervals.iter().any(|span| {
        span.origins.contains(&native.id)
            && matches!(
                span.category,
                IntervalCategory::FiniteService
                    | IntervalCategory::SqlService
                    | IntervalCategory::OwnExternal
            )
    }) {
        return Err("refused original executed finite/SQLite/external service".into());
    }
    let row = observe.archived_row(original)?;
    if serde_json::to_value(&row.slot).map_err(|e| e.to_string())?
        != serde_json::to_value(&expected.slot).map_err(|e| e.to_string())?
    {
        return Err("final observation differs from immutable planned slot".into());
    }
    if candidate.refusal.decoded_response != candidate.raw || candidate.raw["id"] != original {
        return Err("actual raw busy identity or retained value changed".into());
    }
    super::super::faults::validate_candidate(&row, &candidate.refusal)?;
    let mut candidate_final = candidate.refusal.clone();
    candidate_final.native_terminal_rows = vec![super::super::native_archive::native_row(
        native,
        Instant::now(),
    )];
    super::super::faults::validate_candidate(&row, &candidate_final)?;
    Ok(VerifiedRefusal {
        original: original.into(),
        method: expected.method.clone(),
        write_id: native.id,
        intended: candidate.intended,
        sent: candidate.sent,
        decoded: candidate.decoded,
        intake: native.intake,
        classified,
        raw: candidate.raw.clone(),
        raw_artifact: candidate.refusal.raw_response_artifact.clone(),
    })
}
