//! The Git evidence shared by boot and post-archive recovery. No app handle.
use super::recovery::{classify_stage_publication, StagePublicationQuery, StagePublications};
use crate::isolation::{Isolation, ResolvedIsolation};
use crate::run::StagePublication;
use crate::store::PersistedRun;
use crate::worktree::{BranchTeardown, UnregisteredRestore, Worktree, WorktreeManager};

pub(super) struct RunRecoveryWork {
    pub record: PersistedRun,
    pub worktree: Worktree,
    pub project_id: Option<String>,
    pub repo_location: String,
    pub project_owner: bool,
    pub worktrees: Option<WorktreeManager>,
    pub isolation: Isolation,
}

pub(super) struct RunRecoveryEvidence {
    pub project_id: Option<String>,
    pub repo_location: String,
    pub publication: StagePublication,
    pub scratch: Option<Result<(), String>>,
    pub missing: Option<Result<(Worktree, Option<String>), String>>,
    pub missing_publications: StagePublications,
}

impl RunRecoveryWork {
    pub fn decide(self) -> RunRecoveryEvidence {
        let publication = self.publication();
        let merged = self
            .record
            .publication_attempt
            .as_ref()
            .is_some_and(|attempt| {
                matches!(attempt.action.as_str(), "merge" | "merge_push")
                    && publication == StagePublication::Merged
            });
        let terminal = self.record.state.is_terminal() || merged;
        let scratch = (self.project_owner && !terminal)
            .then(|| crate::app::projects::recut_project_scratch(&self.worktree.path));
        let missing = (!self.project_owner && !terminal && !self.worktree.path.exists())
            .then(|| self.restore_checkout());
        let missing_publications = if missing.as_ref().is_some_and(Result::is_err)
            && !(self.record.plan_id.is_some() && self.project_id.is_some())
        {
            self.stage_query().classify()
        } else {
            StagePublications::default()
        };
        RunRecoveryEvidence {
            project_id: self.project_id,
            repo_location: self.repo_location,
            publication,
            scratch,
            missing,
            missing_publications,
        }
    }

    fn publication(&self) -> StagePublication {
        self.record
            .publication_attempt
            .as_ref()
            .zip(self.worktrees.as_ref())
            .map_or(StagePublication::Local, |(attempt, worktrees)| {
                classify_stage_publication(
                    worktrees,
                    &self.worktree.path,
                    &self.worktree.branch(),
                    &self.worktree.base_branch,
                    &attempt.candidate_sha,
                )
            })
    }

    fn restore_checkout(&self) -> Result<(Worktree, Option<String>), String> {
        let worktrees = self
            .worktrees
            .as_ref()
            .filter(|_| !self.record.adopted)
            .ok_or_else(|| "the original project/branch is unavailable".to_string())?;
        let resolved = match worktrees.availability().lock_reason(self.isolation) {
            None => ResolvedIsolation::honoured(self.isolation),
            Some(reason) => ResolvedIsolation::downgraded(reason),
        };
        worktrees
            .restore(
                &self.worktree,
                restore_ownership(self.record.adopted),
                resolved.isolation,
            )
            .map(|worktree| (worktree, resolved.downgrade))
            .map_err(|error| error.to_string())
    }

    fn stage_query(&self) -> StagePublicationQuery {
        StagePublicationQuery {
            run_id: self.record.id.clone(),
            worktrees: self.worktrees.clone(),
            checkout: self.worktree.path.clone(),
            branch: self.worktree.branch(),
            base_branch: self.worktree.base_branch.clone(),
            completions: self
                .record
                .stages
                .iter()
                .filter_map(|stage| Some((stage.stage_id.clone(), stage.completion_sha.clone()?)))
                .collect(),
        }
    }
}

/// A native run owns its original branch; an adopted checkout cannot invent
/// a missing teardown record. Shared with the legacy ownership contract tests.
pub(super) fn restore_ownership(adopted: bool) -> UnregisteredRestore {
    if adopted {
        UnregisteredRestore::Refuse
    } else {
        UnregisteredRestore::Write(BranchTeardown::DeletesBranch)
    }
}
