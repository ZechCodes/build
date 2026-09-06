//! Owned branch-holder inputs and Git readings. Every read runs off the app mutex.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::branch::BranchSource;
use crate::orchestrator::Orchestrator;
use crate::worktree::{ExternalWorktree, Worktree};
use serde_json::{json, Value};

/// A snapshot of the records whose checkouts Git must inspect.
#[derive(Clone)]
pub struct ProjectCheckouts {
    pub project: Orchestrator,
    pub primary_repo_path: PathBuf,
    pub base_branch: String,
    pub excluded: HashSet<PathBuf>,
    pub run_checkouts: Vec<(String, Worktree)>,
}

impl ProjectCheckouts {
    pub fn holders(&self) -> Result<BranchOwnershipIndex, String> {
        let primary = crate::worktree::primary_checkout_holder(&self.primary_repo_path)
            .map_err(|error| error.to_string())?;
        let external = self
            .project
            .scan_checkouts(&self.base_branch, &self.excluded)
            .map_err(|error| error.to_string())?;
        let run_branches = self
            .run_checkouts
            .iter()
            .map(|(id, checkout)| (checkout.branch(), id.clone()))
            .collect();
        Ok(BranchOwnershipIndex {
            external,
            run_branches,
            primary,
        })
    }

    /// Make every held branch's tip the project's before the project's refs
    /// are read (Work Isolation spec §0.4). A linked worktree's already is; a
    /// clone's is not until published, and a listing that skipped this would
    /// weigh a clone's branch by what the project last saw of it. The
    /// project's own checkout holds nothing to publish: its refs are the
    /// project's.
    pub fn publish_held_branches(&self, ownership: &BranchOwnershipIndex) -> Result<(), String> {
        let worktrees = self.project.worktrees();
        let external = ownership.external.iter().filter_map(|checkout| {
            checkout
                .branch
                .clone()
                .map(|branch| (checkout.path.clone(), branch))
        });
        let runs = self
            .run_checkouts
            .iter()
            .filter(|(_, checkout)| checkout.path != self.primary_repo_path)
            .map(|(_, checkout)| (checkout.path.clone(), checkout.branch()));
        for (path, branch) in external.chain(runs) {
            worktrees
                .publish(&path, &branch)
                .map_err(|error| format!("publishing {branch} from {}: {error}", path.display()))?;
        }
        Ok(())
    }
}

pub struct BranchOwnershipIndex {
    pub external: Vec<ExternalWorktree>,
    run_branches: HashMap<String, String>,
    primary: Option<(String, String)>,
}

/// Exactly one holder, selected by the wire protocol's precedence.
pub struct BranchHolder {
    holder: Option<(BranchSource, String)>,
}

impl BranchHolder {
    pub fn of(ownership: &BranchOwnershipIndex, branch: &str) -> Self {
        let holders = [
            (
                BranchSource::Run,
                ownership.run_branches.get(branch).cloned(),
            ),
            (
                BranchSource::PrimaryCheckout,
                ownership
                    .primary
                    .as_ref()
                    .filter(|(_, held)| held == branch)
                    .map(|(id, _)| id.clone()),
            ),
            (
                BranchSource::ExternalWorktree,
                ownership
                    .external
                    .iter()
                    .find(|checkout| checkout.branch.as_deref() == Some(branch))
                    .map(|checkout| checkout.id.clone()),
            ),
        ];
        Self {
            holder: holders
                .into_iter()
                .filter_map(|(source, id)| Some((source, id?)))
                .min_by_key(|(source, _)| *source),
        }
    }

    pub fn held_by(&self, source: BranchSource) -> Option<&str> {
        self.holder
            .as_ref()
            .filter(|(holder, _)| *holder == source)
            .map(|(_, id)| id.as_str())
    }

    pub fn refusal(&self, branch: &str) -> Option<String> {
        self.holder.as_ref().map(|(source, id)| {
            format!(
                "branch {branch:?} is already checked out by {} {id}",
                source.holder_noun()
            )
        })
    }

    pub fn into_json(self) -> Value {
        json!(self
            .holder
            .map(|(source, id)| json!({ "kind": source.as_str(), "id": id })))
    }
}
