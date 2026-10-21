//! Saved native values are consumed and reduced after actual full checks.
use super::*;
#[path = "verify/finite.rs"]
mod finite;
#[path = "verify/message.rs"]
mod message;
#[path = "verify/state.rs"]
mod state;
impl CommonRequest {
    pub fn method(&self) -> &'static str {
        match &self.request {
            ActualRequest::Rpc { method, .. } => *method,
            ActualRequest::Mcp { action, .. } => action.tool_name(),
        }
    }
    pub fn verify_saved(
        &self,
        decoded: &Value,
        actual: SavedEffect,
    ) -> Result<OracleWitness, String> {
        ensure(
            decoded.get("error").is_none_or(Value::is_null)
                && decoded.get("ok").and_then(Value::as_bool) != Some(false),
            "actual RPC/MCP returned an error",
        )?;
        let result = decoded
            .get("result")
            .ok_or("actual final response has no result")?;
        match actual {
            SavedEffect::File { .. } | SavedEffect::GitHead { .. } => {
                finite::verify(self, result, actual)
            }
            SavedEffect::Message { .. } => message::verify(self, result, actual),
            _ => state::verify(self, result, actual),
        }
    }
}
fn state_witness(request: &CommonRequest, fields: Vec<OracleAssertion>) -> OracleWitness {
    OracleWitness::State {
        oracle: format!("native-common-effect/{}", request.method()),
        assertions: fields,
    }
}
