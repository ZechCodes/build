use crate::lifecycle::{AdoptionPrepared, PendingRow, Performed, WorktreeChange, WorktreeMutation};
use crate::models::ModelChoice;
use crate::orchestrator::{AdoptableCheckout, AdoptionScope, Orchestrator};
use crate::worktree::ExternalWorktree;
use std::collections::HashSet;
use std::path::PathBuf;

/// Write Build's ownership into a checkout, for the run that is about to stand
/// for it: judge it, and — only if it passes every refusal — make the checkpoint
/// commit that keeps pre-Build work its own legible commit and lay down the
/// `.build/` scaffold.
///
/// Every adoption goes through here, whatever reached the checkout: `run.adopt`
/// of a card or of the primary checkout, and the dispatch that just cut one. A
/// refusal leaves the checkout exactly as it was found, and what comes back is
/// the apply half — no disk, no refusals left to make.
pub fn adopt(
    project: &Orchestrator,
    project_id: &str,
    checkout: &ExternalWorktree,
    base_branch: &str,
    scope: AdoptionScope,
    run_id: &str,
    model_choice: ModelChoice,
) -> Result<AdoptionPrepared, String> {
    let adoptable = AdoptableCheckout::judge(checkout, base_branch, scope)
        .map_err(|error| error.to_string())?;
    project
        .prepare_adoption(&adoptable, base_branch, run_id)
        .map_err(|error| error.to_string())?;
    Ok(AdoptionPrepared {
        project_id: project_id.to_string(),
        run_id: run_id.to_string(),
        base_branch: base_branch.to_string(),
        checkout: adoptable,
        scope,
        model_choice,
    })
}

/// The checkout an adoption takes ownership of, and the git that reaches it.
///
/// An adoption never acts on a cached card: what Build writes its ownership
/// into has to be what is on disk now, so each arm names the git that asks.
/// The type owns its own variation — which git, which scope, what the board's
/// checkout list should say while the run is opened — so no caller matches on
/// it.
pub enum AdoptionTarget {
    /// A checkout the board lists as its own card, named by the id that card
    /// carries.
    Card {
        worktree_id: String,
        /// Checkouts a run already owns, excluded from the scan exactly as the
        /// board excludes them.
        excluded: HashSet<PathBuf>,
    },
    /// The project's primary checkout — the repo root as a super-worktree. No
    /// card stands for it, and every browser reaches it the same way.
    Primary { repo_path: PathBuf },
}

impl AdoptionTarget {
    pub fn scope(&self) -> AdoptionScope {
        match self {
            AdoptionTarget::Card { .. } => AdoptionScope::ExternalWorktree,
            AdoptionTarget::Primary { .. } => AdoptionScope::PrimaryCheckout,
        }
    }

    /// The identity two adoptions of one checkout collide on, known before any
    /// git runs: the card's own id, or the id the repo root hashes to.
    pub fn checkout_id(&self) -> String {
        match self {
            AdoptionTarget::Card { worktree_id, .. } => worktree_id.clone(),
            AdoptionTarget::Primary { repo_path } => {
                crate::worktree::external_worktree_id(&crate::worktree::canonical_root(repo_path))
            }
        }
    }

    /// The board's row for this adoption: the run it settles as, standing on
    /// the card the browser already lists this checkout under. A card has an id
    /// of its own; the project's primary checkout has none, and is named by
    /// being the primary of its project instead.
    pub fn reserve(&self, run_id: String, project_id: &str, title: String) -> PendingRow {
        let row = PendingRow::creating(run_id, Some(project_id.to_string()), title);
        match self {
            AdoptionTarget::Card { .. } => row.on_checkout(self.checkout_id()),
            AdoptionTarget::Primary { .. } => row.on_primary_checkout(self.checkout_id()),
        }
    }

    pub(super) fn reach(
        &self,
        project: &Orchestrator,
        base_branch: &str,
    ) -> Result<ExternalWorktree, String> {
        match self {
            AdoptionTarget::Card {
                worktree_id,
                excluded,
            } => project
                .scan_checkouts(base_branch, excluded)
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|checkout| &checkout.id == worktree_id)
                .ok_or_else(|| format!("unknown worktree_id: {worktree_id}")),
            AdoptionTarget::Primary { .. } => project
                .worktrees()
                .describe_primary(base_branch)
                .map_err(|error| error.to_string()),
        }
    }

    /// What the board's checkout list is told while the run is being opened. A
    /// card goes back on the board if that record fails; the primary checkout
    /// was never a card and must not become one.
    pub(super) fn amendment(&self, checkout: &ExternalWorktree) -> WorktreeChange {
        match self {
            AdoptionTarget::Card { .. } => WorktreeChange::appeared(checkout.clone()),
            AdoptionTarget::Primary { .. } => WorktreeChange::nothing(),
        }
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
            self.target.scope(),
            &self.run_id,
            self.model_choice,
        )?;
        Ok(Performed {
            change: self.target.amendment(&checkout),
            output: adopted,
        })
    }
}
