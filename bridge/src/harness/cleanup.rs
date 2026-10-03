//! Exact provider artifacts are renamed out of the resume tree before the
//! store commits a clear. Dropping the guard restores them if that commit
//! fails. No provider root or checkout-scoped transcript sweep is eligible.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::models::AgentProvider;
use crate::thread::SessionLineage;

use super::{harness_for, AgentSession, AgentStatus, HarnessError};

/// An artifact whose exact provider ownership has already been established.
pub struct ConversationArtifact {
    root: PathBuf,
    path: PathBuf,
    directory: bool,
}

impl ConversationArtifact {
    pub(crate) fn file(root: &Path, relative: &Path) -> Result<Option<Self>, HarnessError> {
        Self::at(root, relative, false)
    }

    pub(crate) fn directory(root: &Path, relative: &Path) -> Result<Option<Self>, HarnessError> {
        Self::at(root, relative, true)
    }

    fn at(root: &Path, relative: &Path, directory: bool) -> Result<Option<Self>, HarnessError> {
        let Some(path) = checked_path(root, relative, directory)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            root: fs::canonicalize(root)?,
            path,
            directory,
        }))
    }
}

struct StagedArtifact {
    artifact: ConversationArtifact,
    staged: PathBuf,
}

/// Reversible removal of exact artifacts belonging to one cleared agent.
///
/// Callers must first stop and reap its processes, and must omit native
/// lineages that another surviving conversation still references.
pub struct HarnessConversationCleanup {
    staged: Vec<StagedArtifact>,
    committed: bool,
}

impl HarnessConversationCleanup {
    pub fn prepare(
        home: &Path,
        state_root: &Path,
        agent_id: &str,
        current_provider: AgentProvider,
        lineages: &[SessionLineage],
    ) -> Result<Self, HarnessError> {
        let mut artifacts = harness_for(current_provider)
            .conversation_artifacts(home, state_root, agent_id, None, None)?;
        for lineage in lineages
            .iter()
            .filter(|lineage| lineage.agent_id == agent_id)
        {
            let Some(provider) = AgentProvider::from_wire(&lineage.provider) else {
                continue;
            };
            artifacts.extend(harness_for(provider).conversation_artifacts(
                home,
                state_root,
                agent_id,
                lineage.stood_in().map(Path::new),
                lineage.resume_session_id.as_deref(),
            )?);
        }
        let mut cleanup = Self {
            staged: Vec::new(),
            committed: false,
        };
        cleanup.stage_artifacts(artifacts)?;
        Ok(cleanup)
    }

    /// Permanently remove artifacts after the store has committed. An unlink
    /// error leaves the remaining artifacts staged, never resumable again.
    pub fn commit(mut self) -> Result<(), HarnessError> {
        self.committed = true;
        for staged in &self.staged {
            let result = if staged.artifact.directory {
                fs::remove_dir_all(&staged.staged)
            } else {
                fs::remove_file(&staged.staged)
            };
            result.map_err(|error| {
                artifact_error("delete cleared artifact", &staged.staged, error)
            })?;
        }
        Ok(())
    }

    /// Include only Build-owned files carried by this exact stopped process.
    pub fn stage_session(
        &mut self,
        state_root: &Path,
        session: &dyn AgentSession,
    ) -> Result<(), HarnessError> {
        if !matches!(session.status(), AgentStatus::Ended { .. }) {
            return Err(HarnessError::Setup(
                "end the session before clearing its artifacts".into(),
            ));
        }
        for path in session.conversation_artifacts() {
            let relative = path.strip_prefix(state_root).map_err(|_| {
                HarnessError::Setup(format!(
                    "session artifact is outside Build state: {}",
                    path.display()
                ))
            })?;
            self.stage_artifacts(
                ConversationArtifact::file(state_root, relative)?
                    .into_iter()
                    .collect(),
            )?;
        }
        Ok(())
    }

    fn stage_artifacts(
        &mut self,
        artifacts: Vec<ConversationArtifact>,
    ) -> Result<(), HarnessError> {
        let mut known: HashSet<PathBuf> = self
            .staged
            .iter()
            .map(|entry| entry.artifact.path.clone())
            .collect();
        for artifact in artifacts {
            if !known.insert(artifact.path.clone()) {
                continue;
            }
            let relative = artifact
                .path
                .strip_prefix(&artifact.root)
                .expect("validated artifact root");
            if checked_path(&artifact.root, relative, artifact.directory)?.is_none() {
                continue;
            }
            let staged = artifact
                .path
                .parent()
                .expect("validated artifact parent")
                .join(format!(".build-cleared-{}", uuid::Uuid::new_v4()));
            fs::rename(&artifact.path, &staged).map_err(|error| {
                artifact_error("stage conversation artifact", &artifact.path, error)
            })?;
            self.staged.push(StagedArtifact { artifact, staged });
        }
        Ok(())
    }
}

impl Drop for HarnessConversationCleanup {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for entry in self.staged.iter().rev() {
            // A replacement process must never start until the store commits.
            // Refuse to overwrite a file if an unexpected writer appeared.
            if fs::symlink_metadata(&entry.artifact.path).is_ok() {
                continue;
            }
            let _ = fs::rename(&entry.staged, &entry.artifact.path);
        }
    }
}

pub(crate) fn validate_id(id: &str) -> Result<(), HarnessError> {
    if super::is_a_filename(id) {
        return Ok(());
    }
    Err(HarnessError::Setup(
        "conversation artifact id must be a filename".into(),
    ))
}

pub(crate) fn checked_directory(
    root: &Path,
    relative: &Path,
) -> Result<Option<PathBuf>, HarnessError> {
    checked_path(root, relative, true)
}

fn checked_path(
    root: &Path,
    relative: &Path,
    directory: bool,
) -> Result<Option<PathBuf>, HarnessError> {
    let mut path = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(artifact_error("resolve artifact root", root, error)),
    };
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        return Err(HarnessError::Setup(
            "a provider root is not a conversation artifact".into(),
        ));
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(HarnessError::Setup(
                "conversation artifact escapes its root".into(),
            ));
        };
        path.push(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(artifact_error(
                    "inspect conversation artifact",
                    &path,
                    error,
                ))
            }
        };
        let wants_directory = index + 1 < components.len() || directory;
        let valid = if wants_directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        };
        if metadata.file_type().is_symlink() || !valid {
            return Err(HarnessError::Setup(format!(
                "conversation artifact has an unsafe path: {}",
                path.display()
            )));
        }
    }
    Ok(Some(path))
}

fn artifact_error(action: &str, path: &Path, error: std::io::Error) -> HarnessError {
    HarnessError::Setup(format!("{action} {}: {error}", path.display()))
}

#[cfg(test)]
mod tests;
