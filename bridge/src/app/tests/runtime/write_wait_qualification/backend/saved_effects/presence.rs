//! OFFSOURCE extractor: preserves best-effort native semantics; proof is stricter.
use super::*;
pub(super) fn extract(
    request: &CommonRequest,
    reply: &ReplyProof,
    window: InvocationWindow,
    record: &PresenceRecordWitness,
) -> Result<SavedEffect, String> {
    check_wire_original(request, reply)?;
    if !matches!(request.effect, ExpectedEffect::UserPresent) {
        return Err("presence extractor called for different effect".into());
    }
    if record.original_ids != vec![request.original_id.clone()] || record.local != record.published
    {
        return Err("presence actual original/local publication mismatch".into());
    }
    if matches!(
        record.persistence,
        PresencePersistence::SaveFailed | PresencePersistence::NoStore
    ) {
        return Err("best-effort Presence did not establish durable original evidence".into());
    }
    let durable = record
        .durable_at_execution
        .ok_or("no prior actual durable coverage")?;
    if durable != record.local {
        return Err("no exact durable coverage at original execution".into());
    }
    let replied_session: crate::session_summary::UserSession =
        serde_json::from_value(reply.reply["result"]["user_session"].clone())
            .map_err(|_| "native presence reply session missing or invalid")?;
    let replied = replied_session
        .last_activity_ms
        .ok_or("native presence reply timestamp missing")?;
    if record.local != replied_session || window.started_ms > replied || replied > window.decoded_ms
    {
        return Err(
            "native Presence reply does not match exact accepted session/time window".into(),
        );
    }
    Ok(SavedEffect::UserSession {
        persisted: durable,
        invocation_start_ms: window.started_ms,
        decoded_final_ms: window.decoded_ms,
    })
}
