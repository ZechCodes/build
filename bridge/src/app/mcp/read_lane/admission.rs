use super::{AppState, BridgeAction};
use crate::app::mcp::{authenticated_mcp_owner, AddressedSession, McpConversationGeneration};
use serde_json::Value;
use std::path::PathBuf;

pub(super) struct ReadSession {
    pub(super) endpoint: AddressedSession,
}

impl ReadSession {
    pub(super) fn authenticate(
        app: &AppState,
        frame: &Value,
        action: &BridgeAction,
    ) -> Result<Self, String> {
        let agent =
            authenticated_mcp_owner(frame, &app.session_registry).ok_or_else(unauthorized)?;
        let endpoint = app
            .addressed_session(agent.into())
            .ok_or_else(unauthorized)?;
        if let AddressedSession::Coding(generation) = &endpoint {
            generation.guard(app)?;
        }
        check_surface(app, &endpoint, action)?;
        Ok(Self { endpoint })
    }

    pub(super) fn coding(&self) -> Result<&McpConversationGeneration, String> {
        match &self.endpoint {
            AddressedSession::Coding(generation) => Ok(generation),
            _ => Err(unauthorized()),
        }
    }

    pub(super) fn project(
        &self,
        app: &AppState,
        on_rebind: String,
    ) -> Result<ProjectScope, String> {
        let owner = &self.coding()?.entity_id;
        let id = app
            .projects
            .project_id_of(owner)
            .ok_or_else(|| format!("{owner} belongs to no project"))?;
        let project = app
            .projects
            .get(id)
            .ok_or_else(|| format!("unknown project_id: {id}"))?;
        Ok(ProjectScope {
            owner: owner.clone(),
            id: id.into(),
            path: project.repo_path.clone(),
            on_rebind,
        })
    }

    pub(super) fn validate(
        &self,
        app: &AppState,
        frame: &Value,
        action: &BridgeAction,
    ) -> Result<(), String> {
        let agent =
            authenticated_mcp_owner(frame, &app.session_registry).ok_or_else(unauthorized)?;
        let current = app
            .addressed_session(agent.into())
            .ok_or_else(unauthorized)?;
        match (&self.endpoint, &current) {
            (AddressedSession::Coding(before), AddressedSession::Coding(after))
                if before.entity_id == after.entity_id && before.agent_id == after.agent_id =>
            {
                // Keep the existing reset refusal rather than retargeting
                // the captured request to the new thread generation.
                before.guard(app)?;
            }
            _ if self.endpoint != current => return Err(unauthorized()),
            _ => {}
        }
        if self.endpoint != current {
            return Err(unauthorized());
        }
        check_surface(app, &current, action)
    }
}

pub(super) struct ProjectScope {
    owner: String,
    pub(super) id: String,
    pub(super) path: PathBuf,
    on_rebind: String,
}

impl ProjectScope {
    pub(super) fn validate(&self, app: &AppState) -> Result<(), String> {
        let current = app
            .projects
            .project_id_of(&self.owner)
            .ok_or_else(|| format!("{} belongs to no project", self.owner))?;
        if current != self.id {
            return Err(self.on_rebind.clone());
        }
        if app
            .projects
            .get(&self.id)
            .is_none_or(|project| project.repo_path != self.path)
        {
            return Err(format!("unknown project_id: {}", self.id));
        }
        if app.project_deletion_in_progress {
            return Err(crate::app::rpc::PROJECT_DELETION_IN_PROGRESS.into());
        }
        Ok(())
    }
}

fn unauthorized() -> String {
    "unauthorized MCP session".into()
}

fn check_surface(
    app: &AppState,
    endpoint: &AddressedSession,
    action: &BridgeAction,
) -> Result<(), String> {
    match endpoint {
        AddressedSession::Router { .. } if !action.allowed_on(crate::mcp::McpSurface::Router) => {
            Err(format!(
                "{} is a {} tool; this session routes captures",
                action.tool_name(),
                action.surface_name()
            ))
        }
        AddressedSession::Coding(generation) => {
            let surface = app.agent_mcp_surface(&generation.entity_id, &generation.agent_id);
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
