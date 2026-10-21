//! OFFSOURCE integration contract. Not registered in the compiled source graph.
use crate::app::read_context::{IssuedCodingAddress, McpAddresses, ReadService};
use crate::app::tests::runtime::write_wait_qualification::backend::completed_origins::NativeCompletion;
use crate::app::tests::runtime::write_wait_qualification::common_requests::{
    CommonRequest, ExpectedEffect, SavedEffect,
};
use crate::app::tests::runtime::write_wait_qualification::rpc::ReplyProof;
use crate::app::tests::SessionLog;
use crate::store::{ReadPool, Store};
use std::collections::BTreeMap;
use std::sync::Arc;
mod messages;
mod presence;
mod seen;
use crate::app::accepted_records::{
    AcceptedRecord, PresencePersistence, PresenceRecordWitness, SeenRecordWitness,
};

pub(super) struct Handles {
    pub store: Store,
    pub pool: ReadPool,
    pub service: ReadService,
    pub addresses: BTreeMap<String, IssuedCodingAddress>,
    pub logs: BTreeMap<String, SessionLog>,
}
impl Handles {
    fn address(&self, agent: &str) -> Result<IssuedCodingAddress, String> {
        self.addresses
            .get(agent)
            .cloned()
            .ok_or_else(|| "setup accepted address missing".into())
    }
    fn current_addresses(&self) -> Arc<McpAddresses> {
        self.service.mcp_addresses()
    }
}
#[derive(Clone, Copy)]
pub(super) struct InvocationWindow {
    pub started_ms: i64,
    pub decoded_ms: i64,
}
fn check_wire_original(request: &CommonRequest, reply: &ReplyProof) -> Result<(), String> {
    if request.original_id != reply.original_id
        || reply.method != native_method(request)
        || reply.reply["ok"] != true
        || !reply.reply["error"].is_null()
    {
        return Err("original-specific actual transport reply failed".into());
    }
    Ok(())
}
fn native_method(request: &CommonRequest) -> String {
    use crate::app::tests::runtime::write_wait_qualification::common_requests::ActualRequest;
    match &request.request {
        ActualRequest::Rpc { method, .. } => (*method).to_owned(),
        ActualRequest::Mcp { action, .. } => format!("mcp.control:{}", action.tool_name()),
    }
}
pub(super) fn check_original_complete(
    request: &CommonRequest,
    reply: &ReplyProof,
    row: &NativeCompletion,
) -> Result<bool, String> {
    check_wire_original(request, reply)?;
    if row.original_id() != request.original_id || row.method() != native_method(request) {
        return Err("actual completed original or native method differs".into());
    }
    // This private-field type can only come from the bounded cursor consumer's
    // terminal native snapshot check. It has no public success constructor.
    // Exact original/method association is checked again here; no registry poll.
    Ok(true)
}

/// Borrow all retained records: selection does not consume or discard sibling
/// Agent/Task/Seen/Presence evidence. The backend archives the complete vector.
pub(super) async fn verify(
    request: &CommonRequest,
    reply: &ReplyProof,
    handles: &Handles,
    window: InvocationWindow,
    completion: &NativeCompletion,
    records: &[AcceptedRecord],
) -> Result<
    crate::app::tests::runtime::write_wait_qualification::trial_complete::OracleWitness,
    String,
> {
    check_original_complete(request, reply, completion)?;
    for record in records {
        let originals = match record {
            AcceptedRecord::Agent(record) => &record.original_ids,
            AcceptedRecord::Task(record) => &record.original_ids,
            AcceptedRecord::Seen(record) => &record.original_ids,
            AcceptedRecord::Presence(record) => &record.original_ids,
        };
        if originals.len() != 1 || originals[0] != request.original_id {
            return Err("retained native record does not name the exact singleton original".into());
        }
    }
    let saved = match &request.effect {
        ExpectedEffect::Posted { .. } | ExpectedEffect::Handoff { .. } => {
            messages::extract(request, reply, handles, completion).await?
        }
        ExpectedEffect::Seen { .. } => {
            let mut matching = records.iter().filter_map(|record| match record {
                AcceptedRecord::Seen(record) => Some(record),
                _ => None,
            });
            let record = matching.next().ok_or("native Seen record missing")?;
            if matching.next().is_some() {
                return Err("duplicate native Seen records for original".into());
            }
            seen::extract(request, reply, handles, record)?
        }
        ExpectedEffect::UserPresent => {
            let mut matching = records.iter().filter_map(|record| match record {
                AcceptedRecord::Presence(record) => Some(record),
                _ => None,
            });
            let record = matching.next().ok_or("native Presence record missing")?;
            if matching.next().is_some() {
                return Err("duplicate native Presence records for original".into());
            }
            presence::extract(request, reply, window, record)?
        }
        _ => return Err("effect belongs to a different native extractor".into()),
    };
    request.verify_saved(&reply.reply, saved)
}
