use crate::lifecycle::{AdoptionPrepared, PendingRow, Performed, WorktreeChange, WorktreeMutation};
use crate::models::ModelChoice;
use crate::orchestrator::{AdoptableCheckout, Orchestrator};
use crate::worktree::ExternalWorktree;
use std::collections::HashSet;
use std::path::PathBuf;

/// Write Build's ownership into a checkout, for the run that is about to stand
/// for it: judge it, and — only if it passes every refusal — make the checkpoint
/// commit that keeps pre-Build work its own legible commit and lay down the
/// `.build/` scaffold.
///
/// Every adoption goes through here, whatever reached the checkout: `run.adopt`
/// of a card, and the dispatch that just cut one. A refusal leaves the checkout
/// exactly as it was found, and what comes back is the apply half — no disk, no
/// refusals left to make.
pub fn adopt(
    project: &Orchestrator,
    project_id: &str,
    checkout: &ExternalWorktree,
    base_branch: &str,
    run_id: &str,
    model_choice: ModelChoice,
) -> Result<AdoptionPrepared, String> {
    let adoptable =
        AdoptableCheckout::judge(checkout, base_branch).map_err(|error| error.to_string())?;
    project
        .prepare_adoption(&adoptable, base_branch, run_id)
        .map_err(|error| error.to_string())?;
    Ok(AdoptionPrepared {
        project_id: project_id.to_string(),
        run_id: run_id.to_string(),
        base_branch: base_branch.to_string(),
        checkout: adoptable,
        model_choice,
    })
}

/// The checkout an adoption takes ownership of, and the git that reaches it.
///
/// An adoption never acts on a cached card: what Build writes its ownership
/// into has to be what is on disk now, so this carries the scan that asks. A
/// checkout the board lists as its own card is the only thing adoptable — the
/// project's own checkout is a source workspaces are cut from, never a place to
/// work.
pub struct AdoptionTarget {
    /// The id the card carries, which is also the identity two adoptions of one
    /// checkout collide on before any git runs.
    pub worktree_id: String,
    /// Checkouts a run already owns, excluded from the scan exactly as the
    /// board excludes them.
    pub excluded: HashSet<PathBuf>,
}

impl AdoptionTarget {
    pub fn checkout_id(&self) -> String {
        self.worktree_id.clone()
    }

    /// The board's row for this adoption: the run it settles as, standing on
    /// the card the browser already lists this checkout under.
    pub fn reserve(&self, run_id: String, project_id: &str, title: String) -> PendingRow {
        PendingRow::creating(run_id, Some(project_id.to_string()), title)
            .on_checkout(self.checkout_id())
    }

    pub(super) fn reach(
        &self,
        project: &Orchestrator,
        base_branch: &str,
    ) -> Result<ExternalWorktree, String> {
        let worktree_id = &self.worktree_id;
        project
            .scan_checkouts(base_branch, &self.excluded)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|checkout| &checkout.id == worktree_id)
            .ok_or_else(|| format!("unknown worktree_id: {worktree_id}"))
    }

    /// What the board's checkout list is told while the run is being opened. A
    /// card goes back on the board if that record fails.
    pub(super) fn amendment(&self, checkout: &ExternalWorktree) -> WorktreeChange {
        WorktreeChange::appeared(checkout.clone())
    }
}

/// `run.adopt` — take ownership of a checkout that already exists and mint the
/// run that stands for it. The scan that resolves the card, the checkpoint
/// commit and the scaffold are all git; only the run record is not.
pub struct AdoptCheckout {
    pub project: Orchestrator,
    pub project_id: String,
    pub base_branch: String,
    pub run_id: String,
    pub target: AdoptionTarget,
    pub model_choice: ModelChoice,
}

impl WorktreeMutation for AdoptCheckout {
    type Output = AdoptionPrepared;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let checkout = self.target.reach(&self.project, &self.base_branch)?;
        let adopted = adopt(
            &self.project,
            &self.project_id,
            &checkout,
            &self.base_branch,
            &self.run_id,
            self.model_choice,
        )?;
        Ok(Performed {
            change: self.target.amendment(&checkout),
            output: adopted,
        })
    }
}
