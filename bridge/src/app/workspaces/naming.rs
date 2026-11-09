//! An agent may supply the missing label of its own workspace once.
use super::*;

impl AppState {
    pub(in crate::app) fn workspace_metadata_busy(&self, id: &str) -> bool {
        self.deferred_work.is_some()
            || self.active_deferred_filesystem_jobs > 0
            || self.workspace_reserved(id)
            || self.workspace_has_pending_shell(id)
    }

    pub(in crate::app) fn workspace_name_request_blocked(
        &self,
        owner: &str,
        kind: crate::agent_message::MessageKind,
    ) -> bool {
        if kind == crate::agent_message::MessageKind::ProviderCommand {
            return false;
        }
        self.workspace_for_conversation(owner, None)
            .is_some_and(|workspace| {
                workspace.waiting_for_name
                    && !workspace.name_asked
                    && self.workspace_metadata_busy(&workspace.id)
            })
    }

    pub(in crate::app) fn claim_workspace_name_request(
        &mut self,
        owner: &str,
        agent_id: &str,
    ) -> Result<Option<String>, String> {
        if self.is_project_coordinator(owner, agent_id) {
            return Ok(None);
        }
        let Some(id) = self.workspace_of_conversation(owner) else {
            return Ok(None);
        };
        Ok(self.workspaces.claim_name_request(&id)?.then_some(id))
    }

    pub(in crate::app) fn set_workspace_name(
        &mut self,
        owner: &str,
        name: &str,
    ) -> Result<Value, String> {
        let name = crate::mcp::normalized_workspace_name(name)?;
        let workspace = self
            .workspace_for_conversation(owner, None)
            .ok_or("set_workspace_name: you are not in a workspace")?;
        if !workspace.waiting_for_name {
            return Err(
                "set_workspace_name: workspace already named; keep the person's name".into(),
            );
        }
        self.workspace_rename(&json!({"workspace_id":workspace.id,"name":name}))
    }
}
