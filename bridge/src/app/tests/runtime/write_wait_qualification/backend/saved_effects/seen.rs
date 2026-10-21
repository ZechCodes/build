//! OFFSOURCE extractor: consumes actual retained source witness, no SQL under app.
use super::*;
pub(super) fn extract(
    request: &CommonRequest,
    reply: &ReplyProof,
    handles: &Handles,
    captured: &SeenRecordWitness,
) -> Result<SavedEffect, String> {
    check_wire_original(request, reply)?;
    let ExpectedEffect::Seen {
        owner,
        agent,
        thread,
    } = &request.effect
    else {
        return Err("Seen extractor called for different effect".into());
    };
    let address = handles.address(agent)?;
    handles.current_addresses().validate(&address)?;
    let crate::app::tests::runtime::write_wait_qualification::common_requests::ActualRequest::Rpc {
        method: "entity.seen",
        params,
    } = &request.request
    else {
        return Err("Seen effect lacks actual entity.seen request".into());
    };
    if params["entity_id"] != address.owner
        || params["agent_id"] != address.agent
        || params["conversation_id"] != address.conversation
        || params["thread_id"] != address.thread_id
    {
        return Err("Seen native request does not use the captured canonical address".into());
    }
    if captured.original_ids != vec![request.original_id.clone()]
        || &captured.owner != owner
        || &captured.agent != agent
        || &captured.conversation_id != thread
        || captured.owner != address.owner
        || captured.agent != address.agent
        || captured.conversation_owner != address.canonical_owner
        || captured.conversation_id != address.conversation
        || captured.thread_id != address.thread_id
        || captured.generation_revision != address.generation
        || captured.native_revision.is_some()
        || captured.saved_through_sequence < captured.actual_reached_sequence
        || captured.published_through_sequence != captured.saved_through_sequence
    {
        return Err("actual Seen accepted original/address/target/cursors differ".into());
    }
    Ok(SavedEffect::Seen {
        owner: captured.owner.clone(),
        agent: captured.agent.clone(),
        thread: captured.conversation_id.clone(),
        persisted_through: captured.saved_through_sequence,
        published_through: captured.published_through_sequence,
        actual_read_target_through: captured.actual_reached_sequence,
    })
}
