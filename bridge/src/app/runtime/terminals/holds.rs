//! Pending shells retain only their own checkout identity across process open.
use super::create_scope::Selector;
use crate::app::AppState;

impl AppState {
    pub(in crate::app) fn workspace_has_pending_shell(&self, id: &str) -> bool {
        self.workspace_shell_hold(id, false)
    }

    pub(in crate::app) fn workspace_contains_pending_shell(&self, id: &str) -> bool {
        self.workspace_shell_hold(id, true)
    }

    fn workspace_shell_hold(&self, id: &str, contains: bool) -> bool {
        if self.session_registry.pending_shell_for_workspace(id) {
            return true;
        }
        let Ok(scope) = Selector::Workspace(id.to_owned()).capture(self) else {
            return false;
        };
        let Some(raw) = scope.known_root() else {
            return false;
        };
        let canonical = self.cached_snapshot_root(raw);
        if contains {
            self.session_registry.pending_shell_within(raw)
                || self.session_registry.pending_shell_within(&canonical)
        } else {
            self.session_registry.pending_shell_at(raw)
                || self.session_registry.pending_shell_at(&canonical)
        }
    }
}
