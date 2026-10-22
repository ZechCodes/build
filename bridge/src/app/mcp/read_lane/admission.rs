use super::service::{Inputs, ProjectScope};
use crate::app::read_context::IssuedCodingAddress;
use crate::app::runtime::sessions::{McpIssuer, RouterAuthority};
use crate::mcp::BridgeAction;
use serde_json::Value;

pub(super) struct ReadSession {
    endpoint: McpIssuer,
}

impl ReadSession {
    pub(super) fn authenticate(
        inputs: &Inputs,
        frame: &Value,
        action: &BridgeAction,
    ) -> Result<Self, String> {
        let endpoint = authenticate_token(inputs, frame)?;
        if let McpIssuer::Coding(issued) = &endpoint {
            // The token's stable installation address must have committed.
            let current = inputs
                .addresses
                .issued_address(&issued.agent)
                .map_err(|_| unauthorized())?;
            if current != *issued {
                return Err(unauthorized());
            }
            guard_source(inputs, issued, &current)?;
        }
        check_surface(inputs, &endpoint, action)?;
        Ok(Self { endpoint })
    }

    pub(super) fn coding(&self) -> Result<&IssuedCodingAddress, String> {
        match &self.endpoint {
            McpIssuer::Coding(issued) => Ok(issued),
            McpIssuer::Router(_) => Err(unauthorized()),
        }
    }

    pub(super) fn project(
        &self,
        inputs: &Inputs,
        on_rebind: String,
    ) -> Result<ProjectScope, String> {
        ProjectScope::capture(inputs, &self.coding()?.owner, on_rebind)
    }

    pub(super) fn validate(
        &self,
        inputs: &Inputs,
        frame: &Value,
        action: &BridgeAction,
    ) -> Result<(), String> {
        // Authentication still runs even when query work returned an error.
        let current = authenticate_token(inputs, frame)?;
        match (&self.endpoint, &current) {
            (McpIssuer::Coding(before), McpIssuer::Coding(after))
                if before.owner == after.owner && before.agent == after.agent =>
            {
                // Preserve reset/binding refusals before generic issuer mismatch.
                let saved = inputs
                    .addresses
                    .issued_address(&after.agent)
                    .map_err(|_| unauthorized())?;
                if before.owner != saved.owner || before.agent != saved.agent {
                    return Err(unauthorized());
                }
                guard_source(inputs, before, &saved)?;
                if saved != *after {
                    return Err(unauthorized());
                }
            }
            (McpIssuer::Router(before), McpIssuer::Router(after)) if before == after => {
                validate_router(inputs, before)?;
            }
            _ => return Err(unauthorized()),
        }
        if self.endpoint != current {
            return Err(unauthorized());
        }
        check_surface(inputs, &current, action)
    }
}

fn authenticate_token(inputs: &Inputs, frame: &Value) -> Result<McpIssuer, String> {
    let agent = frame
        .get("task_id")
        .and_then(Value::as_str)
        .ok_or_else(unauthorized)?;
    let supplied = frame
        .get("session_token")
        .and_then(Value::as_str)
        .ok_or_else(unauthorized)?;
    inputs
        .authority
        .authenticate_token(agent, supplied)
        .ok_or_else(unauthorized)
}

fn validate_router(inputs: &Inputs, router: &RouterAuthority) -> Result<(), String> {
    if inputs.authority.router_is_current(router) {
        Ok(())
    } else {
        Err(unauthorized())
    }
}

fn unauthorized() -> String {
    "unauthorized MCP session".into()
}

fn check_surface(
    inputs: &Inputs,
    endpoint: &McpIssuer,
    action: &BridgeAction,
) -> Result<(), String> {
    match endpoint {
        McpIssuer::Router(_) if !action.allowed_on(crate::mcp::McpSurface::Router) => Err(format!(
            "{} is a {} tool; this session routes captures",
            action.tool_name(),
            action.surface_name()
        )),
        McpIssuer::Coding(issued) => {
            let surface = inputs.addresses.surface(&issued.owner, &issued.agent)?;
            if action.allowed_on(surface) {
                Ok(())
            } else {
                Err(format!(
                    "{} is a {} tool; this session is on the {} surface",
                    action.tool_name(),
                    action.surface_name(),
                    surface.as_str()
                ))
            }
        }
        _ => Ok(()),
    }
}

/// Legacy coding admission resolves the CURRENT binding, then checks reservation
/// and the captured thread. It never reports old conversation_id first.
fn guard_source(
    inputs: &Inputs,
    captured: &IssuedCodingAddress,
    current: &IssuedCodingAddress,
) -> Result<(), String> {
    inputs.guard_reservation(&current.conversation)?;
    if current.thread_id != captured.thread_id || current.generation != captured.generation {
        return Err(format!(
            "stale thread_id {}; current thread is {}",
            captured.thread_id, current.thread_id
        ));
    }
    Ok(())
}
