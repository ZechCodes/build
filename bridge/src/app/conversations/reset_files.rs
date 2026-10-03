use std::path::{Path, PathBuf};

/// Reversible filesystem half of reset. The original files return if the
/// durable transaction refuses; staging never follows attachment symlinks.
pub(super) struct ResetFiles {
    staged: Vec<(PathBuf, PathBuf)>,
    committed: bool,
}

impl ResetFiles {
    pub(super) fn prepare(
        homes: &[PathBuf],
        leaves: &std::collections::HashSet<String>,
    ) -> Result<Self, String> {
        let mut files = Self {
            staged: Vec::new(),
            committed: false,
        };
        for home in homes {
            if !home.exists() {
                continue;
            }
            if std::fs::canonicalize(home).map_err(|error| error.to_string())? != *home {
                return Err("conversation.reset: attachment folder contains a symlink".into());
            }
            for name in leaves {
                files.stage(&home.join(name))?;
            }
        }
        Ok(files)
    }

    fn stage(&mut self, path: &Path) -> Result<(), String> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_file() {
            return Err("conversation.reset: attachment is not a regular file".into());
        }
        let staged = path.with_file_name(format!(".reset-{}", uuid::Uuid::new_v4()));
        std::fs::rename(path, &staged).map_err(|error| error.to_string())?;
        self.staged.push((path.to_path_buf(), staged));
        Ok(())
    }

    pub(super) fn commit(mut self) -> Result<(), String> {
        self.committed = true;
        for (_, path) in &self.staged {
            std::fs::remove_file(path).map_err(|error| {
                format!("conversation.reset: remove staged attachment: {error}")
            })?;
        }
        Ok(())
    }

    /// A sibling may reference a draft while staging runs off-lock. Restore
    /// those bytes before the durable reset commits its current reference set.
    pub(super) fn retain_only(
        &mut self,
        exclusive: &std::collections::HashSet<String>,
    ) -> Result<(), String> {
        let mut retained = Vec::new();
        for (original, staged) in &self.staged {
            let name = original
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if exclusive.contains(name) || original.exists() {
                retained.push((original.clone(), staged.clone()));
            } else {
                std::fs::rename(staged, original).map_err(|error| {
                    format!("conversation.reset: restore shared attachment: {error}")
                })?;
            }
        }
        self.staged = retained;
        Ok(())
    }
}

impl Drop for ResetFiles {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for (original, staged) in self.staged.iter().rev() {
            if let Err(error) = std::fs::rename(staged, original) {
                eprintln!(
                    "conversation.reset: restore attachment {}: {error}",
                    original.display()
                );
            }
        }
    }
}
