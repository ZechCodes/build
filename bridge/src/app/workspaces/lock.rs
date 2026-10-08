//! The user's persisted workspace removal guard. No MCP action can change it.
use crate::app::{require_str, AppState};
use serde_json::Value;

impl AppState {
    pub(crate) fn workspace_set_locked(&mut self, params: &Value) -> Result<Value, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        let locked = params
            .get("locked")
            .and_then(Value::as_bool)
            .ok_or_else(|| "locked must be a boolean".to_string())?;
        // Off-lock jobs write workspace snapshots back. Wait for them, and
        // never accept a lock after a removal has already been admitted.
        if self.deferred_work.is_some()
            || self.active_deferred_filesystem_jobs > 0
            || self.workspace_reserved(&workspace_id)
        {
            return Err(crate::reclaim::BUSY.to_string());
        }
        if self.workspaces.get(&workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        let workspace = self.workspaces.set_locked(&workspace_id, locked)?;
        self.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
        Ok(super::workspace_json(&workspace))
    }
}
