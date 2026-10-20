//! Install observations already captured by the off-lock Git producer.
use crate::app::AppState;
use serde_json::Value;
use std::path::Path;

impl AppState {
    pub(in crate::app) fn record_git_snapshot_branches(&mut self, status: &Value) {
        let (Some(path), Some(branch)) = (status["path"].as_str(), status["branch"].as_str())
        else {
            return;
        };
        let root = self.snapshot_root(Path::new(path));
        let owners = self
            .runs
            .iter()
            .filter_map(|(id, run)| {
                if self.snapshot_root(&run.worktree.path) != root {
                    return None;
                }
                // Worktree::branch retains its recorded identity at detached HEAD.
                let branch = if branch == "HEAD" {
                    run.worktree.recorded_branch.clone()
                } else {
                    branch.to_owned()
                };
                Some((id.clone(), branch))
            })
            .collect::<Vec<_>>();
        for (owner, branch) in owners {
            self.record_snapshot_run_branch(&owner, branch);
        }
    }
}
