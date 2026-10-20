//! Track actual registry rows without turning status publication into a scan.
use super::*;
use crate::observation_changes::ChangeKeys;
use std::ops::{Deref, DerefMut};

pub(super) struct WorkspaceMutation<'a> {
    id: String,
    before: Workspace,
    current: &'a mut Workspace,
    changes: &'a ChangeKeys,
}

impl Deref for WorkspaceMutation<'_> {
    type Target = Workspace;

    fn deref(&self) -> &Workspace {
        self.current
    }
}

impl DerefMut for WorkspaceMutation<'_> {
    fn deref_mut(&mut self) -> &mut Workspace {
        self.current
    }
}

impl Drop for WorkspaceMutation<'_> {
    fn drop(&mut self) {
        // A failed save that rolls its candidate back emits nothing. Mutators
        // that retain real checkpoint/physical changes keep their old semantics.
        if self.before != *self.current {
            self.changes.mark(self.id.clone());
        }
    }
}

impl WorkspaceRegistry {
    pub(crate) fn observation_changes(&self) -> &ChangeKeys {
        &self.observation_changes
    }

    pub(super) fn observed_workspace_mut(
        &mut self,
        id: &str,
    ) -> Result<WorkspaceMutation<'_>, String> {
        let current = self
            .workspaces
            .get_mut(id)
            .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
        Ok(WorkspaceMutation {
            id: id.into(),
            before: current.clone(),
            current,
            changes: &self.observation_changes,
        })
    }

    pub(super) fn install_workspace(&mut self, workspace: Workspace) {
        if self.workspaces.get(&workspace.id) != Some(&workspace) {
            self.observation_changes.mark(workspace.id.clone());
            self.workspaces.insert(workspace.id.clone(), workspace);
        }
    }

    pub(super) fn note_reloaded_workspaces(&self, previous: &HashMap<String, Workspace>) {
        for (id, workspace) in previous {
            if self.workspaces.get(id) != Some(workspace) {
                self.observation_changes.mark(id.clone());
            }
        }
        for id in self.workspaces.keys() {
            if !previous.contains_key(id) {
                self.observation_changes.mark(id.clone());
            }
        }
    }
}
