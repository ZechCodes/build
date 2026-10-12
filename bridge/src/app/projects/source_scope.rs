//! One original source of a project, as a client names it: the project, the
//! source's id and — for every verb added since `fs.projectSourcePath` — the
//! folder the client opened it at.
//!
//! The Files verbs, the Git verbs and the change subscriptions resolve the
//! same membership here, so a source moved or removed in settings is refused
//! the same way by each of them instead of quietly reaching its new folder.

use std::path::PathBuf;

use crate::app::AppState;
use crate::changes::{Kind, KindSet, Scope};

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
    /// Recheck a captured original-source identity after waiting off the app mutex.
    pub(in crate::app) fn validate_source_location(
        &self,
        captured: &ResolvedProjectSource,
    ) -> Result<(), String> {
        let current = self.resolve_project_source(
            &captured.project_id,
            &captured.source_id,
            Some(&captured.source_path),
            MOVED_GIT_SOURCE,
        )?;
        if current.root != captured.root || current.is_git != captured.is_git {
            return Err(MOVED_GIT_SOURCE.to_string());
        }
        Ok(())
    }

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

    /// Whether a `project_source` subscription names a registered source
    /// where the client opened it, and asks for Git only where there is Git.
    /// Every other scope is the bus's own business.
    pub(crate) fn check_source_subscription(
        &self,
        scope: &Scope,
        kinds: &KindSet,
    ) -> Result<(), String> {
        let Scope::ProjectSource(source) = scope else {
            return Ok(());
        };
        let resolved = self.resolve_project_source(
            &source.project_id,
            &source.source_id,
            Some(&source.source_path),
            MOVED_GIT_SOURCE,
        )?;
        if kinds.contains(Kind::Git) && !resolved.is_git {
            return Err(NOT_GIT_SOURCE.to_string());
        }
        Ok(())
    }
}
