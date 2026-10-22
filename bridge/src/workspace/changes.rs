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

    pub(super) fn install_reloaded_workspaces(
        &mut self,
        loaded: Option<HashMap<String, Workspace>>,
    ) {
        let refresh_legacy = loaded.is_some();
        let legacy = self.root.join(".legacy");
        let mut previous = std::mem::take(&mut self.workspaces);
        let mut loaded = loaded.unwrap_or_default();
        for (id, workspace) in &mut loaded {
            if !workspace.managed {
                refresh_adopted_record(workspace, &legacy);
            }
            if previous.remove(id).as_ref() != Some(workspace) {
                self.observation_changes.mark(id.clone());
            }
        }
        for (id, mut workspace) in previous {
            if workspace.managed {
                self.observation_changes.mark(id);
            } else {
                if refresh_legacy && refresh_adopted_record(&mut workspace, &legacy) {
                    self.observation_changes.mark(id.clone());
                }
                loaded.insert(id, workspace);
            }
        }
        self.workspaces = loaded;
    }
}

fn refresh_adopted_record(workspace: &mut Workspace, legacy: &Path) -> bool {
    let proof = fs::read(legacy.join(&workspace.id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Workspace>(&bytes).ok());
    let Some(proof) = proof else {
        let changed = workspace.status != WorkspaceStatus::Ready || workspace.archived_at.is_some();
        workspace.status = WorkspaceStatus::Ready;
        workspace.archived_at = None;
        return changed;
    };
    let mut changed =
        workspace.status != WorkspaceStatus::Finished || workspace.archived_at != proof.archived_at;
    workspace.status = WorkspaceStatus::Finished;
    workspace.archived_at = proof.archived_at;
    for directory in &mut workspace.directories {
        let head = proof
            .directories
            .iter()
            .find(|candidate| candidate.source_id == directory.source_id)
            .and_then(|candidate| candidate.finished_head.clone());
        changed |= directory.finished_head != head;
        directory.finished_head = head;
    }
    changed
}
