//! One original source of a project, as a client names it: the project, the
//! source's id and — for every verb added since `fs.projectSourcePath` — the
//! folder the client opened it at.
//!
//! The Files verbs, the Git verbs and the change subscriptions resolve the
//! same membership here, so a source moved or removed in settings is refused
//! the same way by each of them instead of quietly reaching its new folder.

use std::path::PathBuf;

use crate::app::AppState;

/// The refusal when a caller's `source_path` no longer names the folder its
/// project source resolves to, for a Git verb or a change subscription.
pub(crate) const MOVED_GIT_SOURCE: &str =
    "Build cannot use this project folder: it moved. Reopen Changes before reading or changing it.";

/// The refusal when a Git verb or a `git` subscription names a source that is
/// a plain folder.
pub(crate) const NOT_GIT_SOURCE: &str =
    "Build cannot use Git in this project folder: it is not a Git repository.";

/// One registered source, resolved from server records.
#[derive(Clone, Debug)]
pub(in crate::app) struct ResolvedProjectSource {
    pub(in crate::app) project_id: String,
    pub(in crate::app) source_id: String,
    /// The path the source row carries, which is what a client captures.
    pub(in crate::app) source_path: String,
    /// That folder, canonicalised: where the work happens.
    pub(in crate::app) root: PathBuf,
    pub(in crate::app) is_git: bool,
}

impl AppState {
    /// Resolve `source_id` within `project_id` and, when the caller says
    /// where it opened the source, refuse with `moved` unless the row still
    /// names that folder.
    pub(in crate::app) fn resolve_project_source(
        &self,
        project_id: &str,
        source_id: &str,
        opened_at: Option<&str>,
        moved: &str,
    ) -> Result<ResolvedProjectSource, String> {
        let project = self
            .projects
            .get(project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
        let source = project
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
        let source_path = source.path.display().to_string();
        if opened_at.is_some_and(|opened| opened != source_path) {
            return Err(moved.to_string());
        }
        let root = std::fs::canonicalize(&source.path)
            .map_err(|error| format!("cannot resolve scope root: {error}"))?;
        Ok(ResolvedProjectSource {
            project_id: project_id.to_string(),
            source_id: source_id.to_string(),
            source_path,
            root,
            is_git: source.is_git,
        })
    }
}
