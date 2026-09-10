use crate::lifecycle::BeforeRemoval;
use crate::run::StagePublication;
use crate::worktree::{bounded_git_fetch, configured_remote_for_branch, WorktreeManager};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct StagePublicationQuery {
    pub run_id: String,
    pub worktrees: Option<WorktreeManager>,
    pub checkout: PathBuf,
    pub branch: String,
    pub base_branch: String,
    pub completions: Vec<(String, String)>,
}
#[derive(Default)]
pub struct StagePublications(HashMap<String, StagePublication>);
impl StagePublicationQuery {
    pub fn classify(&self) -> StagePublications {
        let Some(worktrees) = self.worktrees.as_ref() else {
            return StagePublications::default();
        };
        StagePublications(
            self.completions
                .iter()
                .map(|(stage_id, completion_sha)| {
                    (
                        stage_id.clone(),
                        classify_stage_publication(
                            worktrees,
                            &self.checkout,
                            &self.branch,
                            &self.base_branch,
                            completion_sha,
                        ),
                    )
                })
                .collect(),
        )
    }
}
impl BeforeRemoval for StagePublicationQuery {
    type Output = StagePublications;
    fn judge(self) -> Self::Output {
        self.classify()
    }
}
impl StagePublications {
    pub fn of(&self, stage_id: &str) -> StagePublication {
        self.0
            .get(stage_id)
            .copied()
            .unwrap_or(StagePublication::Local)
    }
}
pub(crate) fn classify_stage_publication(
    worktrees: &WorktreeManager,
    checkout: &std::path::Path,
    branch: &str,
    base_branch: &str,
    completion_sha: &str,
) -> StagePublication {
    if checkout.exists() {
        if let Err(error) = worktrees.publish(checkout, branch) {
            eprintln!(
                "classify_stage_publication {branch}: publishing {} failed: {error}",
                checkout.display()
            );
        }
    }
    let repo_path = worktrees.repo_path();
    let Ok(repo) = git2::Repository::open(repo_path) else {
        return StagePublication::Local;
    };
    let Ok(completion) = git2::Oid::from_str(completion_sha) else {
        return StagePublication::Local;
    };
    if let Some(remote) = configured_remote_for_branch(&repo, branch) {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
        let _ = bounded_git_fetch(repo_path, &remote, &refspec);
    }
    let reachable = |reference: &str| {
        repo.find_reference(reference)
            .ok()
            .and_then(|reference| reference.peel_to_commit().ok())
            .is_some_and(|tip| {
                tip.id() == completion
                    || repo
                        .graph_descendant_of(tip.id(), completion)
                        .unwrap_or(false)
            })
    };
    if reachable(&format!("refs/heads/{base_branch}")) {
        return StagePublication::Merged;
    }
    let upstream_ref = repo
        .find_branch(branch, git2::BranchType::Local)
        .ok()
        .and_then(|local| local.upstream().ok())
        .and_then(|upstream| upstream.get().name().map(str::to_string));
    if upstream_ref.as_deref().is_some_and(reachable) {
        StagePublication::Pushed
    } else {
        StagePublication::Local
    }
}
