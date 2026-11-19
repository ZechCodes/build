use super::{acted, argument, refused, required_argument, BridgeAction, Handled};
use serde_json::{json, Value};

pub(super) fn set_agent_model_tool() -> Value {
    json!({
        "name": "set_agent_model",
        "description": "Change an agent's model within its existing live session and project. Build saves the choice immediately between turns, or records it for the next turn if the agent is working. The call never interrupts or starts a turn. Build refuses a choice that would require a new session.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "agent_id": { "type": "string", "description": "From list_project_agents or list_workspace_agents." },
                "model": { "type": "string", "description": "Exact model ID for the agent's harness from list_harnesses." },
                "effort": { "type": "string", "description": "A reasoning effort the model offers. Omit to keep the existing effort. Claude ADK requires the session's launch effort." }
            },
            "required": ["agent_id", "model"]
        }
    })
}

pub(super) fn set_agent_model_action(id: Value, params: Option<&Value>) -> Handled {
    match parse_choice(params) {
        Ok(action) => acted(id, action),
        Err(message) => refused(id, message),
    }
}

fn parse_choice(params: Option<&Value>) -> Result<BridgeAction, String> {
    if ["harness", "provider"]
        .into_iter()
        .any(|field| argument(params, field).is_some())
    {
        return Err(
            "set_agent_model: the agent is locked to its harness. Specify model and effort only."
                .into(),
        );
    }
    let agent_id = required_argument(params, "agent_id")?;
    let model = required_argument(params, "model")?;
    let effort = match argument(params, "effort") {
        None => None,
        Some(Value::String(effort)) => Some(effort),
        _ => return Err("invalid effort: expected a string".into()),
    };
    Ok(BridgeAction::SetAgentModel {
        agent_id,
        model,
        effort,
    })
}
