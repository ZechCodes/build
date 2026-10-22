//! A file mutation reopens a finished workspace without holding app during I/O.
use super::*;

pub(crate) struct FileReopen {
    before: Workspace,
    after: Workspace,
    legacy_marker: PathBuf,
}

impl FileReopen {
    pub(crate) fn persist(&self) -> Result<(), String> {
        if self.after.managed {
            persist(&self.after)
        } else {
            match fs::remove_file(&self.legacy_marker) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "reopen legacy workspace {}: {error}",
                    self.after.id
                )),
            }
        }
    }
}

impl WorkspaceRegistry {
    pub(crate) fn prepare_file_reopen(&self, id: &str) -> Result<Option<FileReopen>, String> {
        let before = self
            .get(id)
            .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
        if before.status != WorkspaceStatus::Finished {
            return Ok(None);
        }
        let mut after = before.clone();
        after.status = WorkspaceStatus::Ready;
        after.archived_at = None;
        Ok(Some(FileReopen {
            before: before.clone(),
            after,
            legacy_marker: self.root.join(".legacy").join(id),
        }))
    }

    pub(crate) fn apply_file_reopen(&mut self, reopened: FileReopen) -> Result<(), String> {
        let current = self
            .get(&reopened.before.id)
            .ok_or_else(|| format!("unknown workspace_id: {}", reopened.before.id))?;
        if current == &reopened.after {
            return Ok(());
        }
        if current != &reopened.before {
            return Err("workspace changed while reopening after a file mutation".into());
        }
        self.install_workspace(reopened.after);
        Ok(())
    }
}
