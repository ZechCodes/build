//! Actual reply/effect proof validation, split by contract boundary.
use super::*;

pub(super) fn validate_proof(row: &Row, proof: &ExecutionProof) -> Result<(), String> {
    validate_identity(row, proof)?;
    validate_endpoints(row)?;
    validate_root(row, proof)?;
    validate_effect(proof)?;
    validate_read_cohort(row, proof)?;
    validate_endpoint_order(row)
}
fn validate_identity(row: &Row, proof: &ExecutionProof) -> Result<(), String> {
    if proof.original_id != row.slot.original_id || proof.method != row.recipe.method {
        return Err("reply original/method pairing mismatch".into());
    }
    if row
        .recipe
        .durable_operation_id
        .as_ref()
        .is_some_and(|operation| proof.durable_operation_id.as_ref() != Some(operation))
    {
        return Err("actual durable mutation ID differs from planned original association".into());
    }
    Ok(())
}
fn validate_endpoints(row: &Row) -> Result<(), String> {
    let required = [
        Stage::Offered,
        Stage::DispatchStarted,
        Stage::WireSent,
        Stage::ParsedFinalReply,
        Stage::EffectVerified,
    ];
    if required
        .iter()
        .any(|stage| !row.milestones_us.contains_key(stage))
    {
        return Err("missing actual endpoint/effect observation".into());
    }
    for (needed, stage, message) in [
        (
            row.recipe.needs_admission,
            Stage::Admitted,
            "missing actual admission observation",
        ),
        (
            row.recipe.needs_durable_ack,
            Stage::DurableAck,
            "missing actual durable-ack observation",
        ),
    ] {
        if needed && !row.milestones_us.contains_key(&stage) {
            return Err(message.into());
        }
    }
    Ok(())
}
fn validate_root(row: &Row, proof: &ExecutionProof) -> Result<(), String> {
    if row.recipe.needs_registry_root
        && (proof.registry_root.is_none() || !proof.mandatory_children_complete)
    {
        return Err("logical root/mandatory children not complete".into());
    }
    if row.recipe.needs_registry_root && !row.milestones_us.contains_key(&Stage::ServerIntake) {
        return Err("logical root missing actual registry intake".into());
    }
    Ok(())
}
fn validate_effect(proof: &ExecutionProof) -> Result<(), String> {
    if proof
        .effect
        .as_ref()
        .is_none_or(|effect| effect.boundary.is_empty())
    {
        return Err("actual durable/published/read/control effect proof required".into());
    }
    validate_witness(&proof.response_evidence.oracle_witness)?;
    validate_witness(&proof.effect.as_ref().unwrap().witness)?;
    let digest = &proof.response_evidence.decoded_json_reserialized_sha256;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("decoded JSON SHA256 must be the actual64hex digest".into());
    }
    Ok(())
}
fn validate_read_cohort(row: &Row, proof: &ExecutionProof) -> Result<(), String> {
    if row.slot.traffic == Traffic::Read && proof.read_cohort.is_none() {
        return Err("read cohort missing".into());
    }
    if row.slot.traffic == Traffic::Read
        && (row.recipe.expected_read_cohort.is_none()
            || row.recipe.expected_read_cohort != proof.read_cohort)
    {
        return Err("actual read cohort differs from declared recipe; pending/legacy cannot replace ready evidence".into());
    }
    Ok(())
}
fn validate_endpoint_order(row: &Row) -> Result<(), String> {
    let order = [
        Stage::Offered,
        Stage::DispatchStarted,
        Stage::WireSent,
        Stage::ServerIntake,
        Stage::Admitted,
        Stage::DurableAck,
        Stage::EncodedFinalReply,
        Stage::ParsedFinalReply,
    ];
    let observed = order
        .into_iter()
        .filter_map(|stage| row.milestones_us.get(&stage))
        .collect::<Vec<_>>();
    if observed.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err("actual endpoint order is inconsistent".into());
    }
    Ok(())
}
