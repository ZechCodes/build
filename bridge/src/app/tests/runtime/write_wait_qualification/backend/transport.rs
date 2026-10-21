//! Actual codec/endpoint dispatch. This module returns native decoded evidence;
//! it never upgrades a reply into an accepted-version or registry-complete proof.
use super::super::common_requests::ActualRequest;
use super::super::rpc::ReplyProof;
use super::super::trial_complete::{Observation, OracleWitness, Route, Stage};
use super::*;
use crate::app::tests::runtime::write_wait_qualification::read_recipes::{
    assert_mcp_reply, mcp_cases, mcp_read_method, FullReadOutcome, McpCaller,
};

pub(super) enum NativeReadOutcome {
    Rpc(FullReadOutcome),
    McpVerified { actual_tool: String },
}
pub(super) struct NativeReadReply {
    pub reply: ReplyProof,
    pub native_outcome: NativeReadOutcome,
}
impl PreparedBackend {
    pub async fn dispatch_write(&self, slot: &Slot) -> Result<ReplyProof, String> {
        let request = self
            .common
            .get(&slot.original_id)
            .ok_or("no immutable actual write request")?;
        if request.caller != slot.caller || request.owner != slot.owner {
            return Err("immutable write identity changed".into());
        }
        let reply = match &request.request {
            ActualRequest::Rpc { method, params } => {
                self.fixture
                    .as_ref()
                    .ok_or("fixture ownership missing")?
                    .rpc
                    .request(slot.caller, &slot.original_id, method, params.clone())
                    .await
            }
            ActualRequest::Mcp { action, .. } => {
                self.mcp
                    .as_ref()
                    .ok_or("MCP transport missing")?
                    .execute(
                        McpHarness::endpoint_for_caller(slot.caller)?,
                        &slot.original_id,
                        action.clone(),
                        false,
                    )
                    .await?
            }
        };
        // The write proof owner records endpoint stages while retaining this
        // decoded value even when an observation fails, so it can archive it.
        Ok(reply)
    }
    pub async fn dispatch_read(
        &self,
        slot: &Slot,
        observe: &Observation,
    ) -> Result<NativeReadReply, String> {
        match slot.route {
            Route::SpaRpc => {
                if self.mode == PipelineMode::TwoSecondControl {
                    self.bounded_rpc_read(slot, observe).await
                } else {
                    self.rpc_read(slot, observe).await
                }
            }
            Route::AgentMcp => self.mcp_read(slot, observe).await,
        }
    }
    async fn bounded_rpc_read(
        &self,
        slot: &Slot,
        observe: &Observation,
    ) -> Result<NativeReadReply, String> {
        let method = slot.method.as_deref().ok_or("read method missing")?;
        let case = self
            .bounded_rpc_cases
            .iter()
            .find(|case| case.method == method)
            .ok_or("actual bounded RPC case missing")?;
        let reply = self
            .fixture
            .as_ref()
            .ok_or("fixture ownership missing")?
            .rpc
            .request(
                slot.caller,
                &slot.original_id,
                case.method,
                case.params.clone(),
            )
            .await;
        record_reply(slot, &reply, observe)?;
        let outcome =
            crate::app::tests::runtime::write_wait_qualification::read_recipes::assert_rpc_reply(
                case,
                &reply.reply,
                self.bounded_reads
                    .as_ref()
                    .ok_or("actual bounded seed missing")?,
            );
        Ok(NativeReadReply {
            reply,
            native_outcome: NativeReadOutcome::Rpc(outcome),
        })
    }
    async fn rpc_read(
        &self,
        slot: &Slot,
        observe: &Observation,
    ) -> Result<NativeReadReply, String> {
        let method = slot.method.as_deref().ok_or("read method missing")?;
        let case = self
            .rpc_cases
            .iter()
            .find(|case| case.method == method)
            .ok_or("actual Global RPC case missing")?;
        let reply = self
            .fixture
            .as_ref()
            .ok_or("fixture ownership missing")?
            .rpc
            .request(
                slot.caller,
                &slot.original_id,
                case.method,
                case.params.clone(),
            )
            .await;
        record_reply(slot, &reply, observe)?;
        let outcome = self
            .reads
            .as_deref()
            .ok_or("native read fixture missing")?
            .assert_rpc_reply(case, &reply.reply);
        Ok(NativeReadReply {
            reply,
            native_outcome: NativeReadOutcome::Rpc(outcome),
        })
    }
    async fn mcp_read(
        &self,
        slot: &Slot,
        observe: &Observation,
    ) -> Result<NativeReadReply, String> {
        let method = slot.method.as_deref().ok_or("read method missing")?;
        let (inputs, expected) = self
            .mcp_inputs
            .get(slot.owner)
            .ok_or("actual owner read recipe missing")?;
        let (caller, action) = mcp_cases(inputs)
            .into_iter()
            .find(|(_, action)| mcp_read_method(action) == method)
            .ok_or("actual Global MCP action missing")?;
        let endpoint = McpHarness::endpoint_for_caller(slot.caller)?;
        if (caller == McpCaller::Router) != (endpoint == 8) {
            return Err("actual MCP recipe role differs from immutable caller".into());
        }
        let actual_tool = action.tool_name().to_owned();
        let reply = self
            .mcp
            .as_ref()
            .ok_or("MCP transport missing")?
            .execute(endpoint, &slot.original_id, action.clone(), true)
            .await?;
        record_reply(slot, &reply, observe)?;
        assert_mcp_reply(&action, &reply.reply, expected);
        Ok(NativeReadReply {
            reply,
            native_outcome: NativeReadOutcome::McpVerified { actual_tool },
        })
    }
    pub async fn dispatch_exempt(
        &self,
        slot: &Slot,
        observe: &Observation,
    ) -> Result<(ReplyProof, OracleWitness), String> {
        let params = self
            .terminal
            .as_ref()
            .ok_or("owned exempt terminal missing")?
            .params(slot)?;
        let reply = self
            .fixture
            .as_ref()
            .ok_or("fixture ownership missing")?
            .rpc
            .request(
                slot.caller,
                &slot.original_id,
                slot.method.as_deref().ok_or("exempt method missing")?,
                params,
            )
            .await;
        record_reply(slot, &reply, observe)?;
        let witness = exempt::assert_reply(slot, &reply.reply)?;
        Ok((reply, witness))
    }
}
pub(super) fn record_reply(
    slot: &Slot,
    reply: &ReplyProof,
    observe: &Observation,
) -> Result<(), String> {
    // MCP original_id is dedicated one-request socket metadata. Native MCP has
    // no wire ID: this equality never claims an echoed response field.
    if reply.original_id != slot.original_id {
        return Err("actual reply associated with a different original".into());
    }
    observe.stage(&slot.original_id, Stage::WireSent, reply.sent_at)?;
    observe.stage(
        &slot.original_id,
        Stage::ParsedFinalReply,
        reply.final_reply_at,
    )?;
    // ClientWatch sent/complete are owned by the actual adapters, not here.
    Ok(())
}
