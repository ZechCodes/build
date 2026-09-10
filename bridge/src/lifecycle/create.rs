use crate::isolation::{Isolation, ResolvedIsolation};
use crate::lifecycle::holders::{BranchHolder, ProjectCheckouts};
use crate::lifecycle::{CreatedCheckout, Performed, WorktreeChange, WorktreeMutation};
use crate::orchestrator::Orchestrator;

pub struct CreateWorktree {
    pub project: Orchestrator,
    pub base_branch: String,
    pub slug: String,
    pub existing_branch: Option<String>,
    pub checkouts: ProjectCheckouts,
    /// How the checkout is made, resolved by the app before anything was
    /// reserved. A bare worktree has no conversation, so the answer to the ask
    /// is where a fallback is said.
    pub resolved: ResolvedIsolation,
}

impl WorktreeMutation for CreateWorktree {
    type Output = CreatedCheckout;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let minted = match &self.existing_branch {
            Some(branch) => {
                let ownership = self.checkouts.holders()?;
                if let Some(refusal) = BranchHolder::of(&ownership, branch).refusal(branch) {
                    return Err(refusal);
                }
                self.project.create_worktree_on_existing_branch(
                    branch,
                    &self.base_branch,
                    self.resolved.isolation,
                )
            }
            None => self.project.create_bare_worktree(
                &self.slug,
                &self.base_branch,
                self.resolved.isolation,
            ),
        }
        .map_err(|error| error.to_string())?;
        let worktree = minted.worktree;
        let described = self
            .project
            .describe_checkout(&worktree.path, &self.base_branch);
        let (worktree_id, path, change) = match described {
            Ok(checkout) => (
                checkout.id.clone(),
                checkout.path.clone(),
                WorktreeChange::appeared(checkout),
            ),
            Err(error) => {
                eprintln!(
                    "describing the new checkout at {}: {error}",
                    worktree.path.display()
                );
                let canonical = crate::worktree::canonical_root(&worktree.path);
                (
                    crate::worktree::external_worktree_id(&canonical),
                    canonical,
                    WorktreeChange::undescribed(),
                )
            }
        };
        Ok(Performed {
            change,
            output: CreatedCheckout {
                worktree_id,
                branch: worktree.branch(),
                name: worktree.name,
                isolation: Isolation::of(&path),
                downgrade: self.resolved.downgrade,
                path,
                branch_was_cut: minted.teardown.deletes_branch(),
            },
        })
    }
}
