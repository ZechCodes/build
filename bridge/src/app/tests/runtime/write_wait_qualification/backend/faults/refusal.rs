//! Actual busy is optional. A busy string alone never proves pre-durability.
use super::*;
use serde::Serialize;
#[derive(Clone, Debug, Serialize)]
pub(in super::super) enum BusyDisposition {
    VerifiedFrontdoor {
        original_id: String,
        elapsed_us: u64,
        native_write_id: String,
        classified_intake_observed: bool,
        due_to_refusal_us: u64,
        durable_acceptance: bool,
    },
    Invalid {
        original_id: String,
        reason: String,
    },
}
pub(in super::super) fn archived(
    slot: &Slot,
    refusal: &trial_complete::ControlRefusal,
    raw: &Value,
    row: Option<&Value>,
    executed_frames: usize,
) -> BusyDisposition {
    let invalid = |reason: &str| BusyDisposition::Invalid {
        original_id: slot.original_id.clone(),
        reason: reason.into(),
    };
    if !refusal.evidence_errors.is_empty() || refusal.raw_response_artifact.is_empty() {
        return invalid("refusal evidence capture or raw artifact failed");
    }
    if refusal
        .acceptance_records
        .as_ref()
        .is_none_or(|records| !records.is_empty())
    {
        return invalid("Store acceptance absence not captured or accepted records exist");
    }
    if slot.traffic != trial_complete::Traffic::Write || !refusal.pending_pre_durable_verification {
        return invalid("only pending semantic refusal candidates may be classified");
    }
    if raw["error_code"] != "busy" || raw["retryable"] != true {
        return invalid("actual raw response is not retryable busy");
    }
    if raw["ok"] != false
        || raw["id"] != slot.original_id
        || refusal.original_id != slot.original_id
    {
        return invalid("actual refusal protocol/original mismatch");
    }
    if refusal.sent_to_decoded_final_ns > 100_000_000
        || refusal.intended_to_decoded_final_ns > 100_000_000
    {
        return invalid("actual refusal exceeded100ms");
    }
    let Some(row) = row else {
        return invalid("no unique actual native root; pre-durability unproved");
    };
    if !row
        .as_object()
        .is_some_and(|row| row.contains_key("acknowledged_relative_us"))
        || row["method"] != refusal.method
        || row["original_id"] != slot.original_id
        || !row["acknowledged_relative_us"].is_null()
    {
        return invalid("native original mismatch or ACK exists");
    }
    // admitted_relative_us is classified intake, set before the Full check.
    // It is reported unchanged; only ACK/execution/effect proves later work.
    if row["cohort"] != "SemanticWrite"
        || !row["completed_relative_us"].is_number()
        || row["cancelled"] != true
        || row["abandoned"] != false
        || row["root_finished"] != true
        || executed_frames != 0
    {
        return invalid("not a proved frontdoor cancellation before execution");
    }
    if row["outstanding"]
        .as_array()
        .is_none_or(|rows| !rows.is_empty())
        || row["active"].as_array().is_none_or(|rows| !rows.is_empty())
    {
        return invalid("actual rejected root retains unfinished obligations");
    }
    let Some(write_id) = row["write_id"].as_str().filter(|value| !value.is_empty()) else {
        return invalid("actual native write_id missing");
    };
    BusyDisposition::VerifiedFrontdoor {
        original_id: slot.original_id.clone(),
        elapsed_us: refusal.sent_to_decoded_final_ns / 1000,
        native_write_id: write_id.into(),
        classified_intake_observed: row["admitted_relative_us"].is_number(),
        due_to_refusal_us: refusal.intended_to_decoded_final_ns / 1000,
        durable_acceptance: false,
    }
}

/// Exhaustive overload-only outcome validation. This cannot be used as a
/// successful-mutation proof and cannot enter the normal workload path.
pub(in crate::app::tests::runtime::write_wait_qualification) fn validate_candidate(
    row: &trial_complete::Row,
    refusal: &trial_complete::ControlRefusal,
) -> Result<(), String> {
    use trial_complete::Stage;
    if row.slot.traffic != trial_complete::Traffic::Write
        || row.recipe.method != refusal.method
        || row.slot.original_id != refusal.original_id
        || row.proof.is_some()
        || row.error.is_some()
        || !row.services.is_empty()
        || row.milestones_us.contains_key(&Stage::DurableAck)
    {
        return Err("refusal outcome overlaps accepted/error/effect state".into());
    }
    let stages = [
        Stage::Offered,
        Stage::DispatchStarted,
        Stage::WireSent,
        Stage::ServerIntake,
        Stage::ParsedFinalReply,
        Stage::Completed,
    ];
    let times = stages
        .iter()
        .map(|stage| row.milestones_us.get(stage).copied())
        .collect::<Option<Vec<_>>>()
        .ok_or("refusal missing actual offer/intake/response/completion endpoint")?;
    if times.windows(2).any(|pair| pair[0] > pair[1])
        || times[0] < row.slot.due_us
        || times[4].saturating_sub(row.slot.due_us) > 100_000
    {
        return Err("refusal original endpoint order/deadline invalid".into());
    }
    let waits = row
        .waits
        .as_ref()
        .ok_or("refusal final native wait archive absent")?;
    if waits.known_wait_union_us > 5_000_000
        || waits.blocked_or_sql_union_us > 5_000_000
        || waits.conservative_upper_us > 5_000_000
        || waits.max_chargeable_interval_us > 5_000_000
        || waits.sql_service_union_us != 0
        || waits.own_external_union_us != 0
    {
        return Err("refusal has out-of-bound waits or native executed service".into());
    }
    let native = if refusal.native_terminal_rows.len() == 1 {
        refusal.native_terminal_rows.first()
    } else {
        None
    };
    match archived(
        &row.slot,
        refusal,
        &refusal.decoded_response,
        native,
        row.frames.len(),
    ) {
        BusyDisposition::VerifiedFrontdoor {
            native_write_id, ..
        } if waits.write_ids == [native_write_id.clone()] => Ok(()),
        BusyDisposition::Invalid { reason, .. } => Err(reason),
        _ => Err("refusal native root differs from final wait archive".into()),
    }
}

#[cfg(test)]
mod tests;
