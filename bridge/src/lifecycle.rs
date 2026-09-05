//! Worktree lifecycle work, off the app mutex.
//!
//! A lifecycle verb — create a checkout, dispatch onto a branch, adopt one,
//! discard one — is one job in three phases, and only the middle one is slow:
//!
//! - **decide**, under the app mutex: validate, reserve the board row the verb
//!   will settle, clone the project's [`Orchestrator`], build the job.
//! - **run**, holding nothing: `git worktree add`, the checkpoint commit, the
//!   scaffold, the scan a dispatch resolves against. Seconds to minutes on a
//!   large checkout.
//! - **apply**, under the mutex again: release the row, write the result down.
//!
//! This module owns the first two thirds of that. A [`WorktreeMutation`] holds
//! every input it needs by value and cannot name `AppState`, so the rule that
//! the git runs with the lock free is structural rather than remembered. What
//! the git produced travels back as a [`LifecycleEpilogue`], which is the only
//! half that touches state — and lives beside the state it writes.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::app::AppState;
use crate::models::ModelChoice;
use crate::orchestrator::{AdoptableCheckout, AdoptionScope, Orchestrator};
use crate::worktree::{ExternalWorktree, NamedBranchCheckout};
use serde_json::Value;

/// One lifecycle verb's git work, and the reservation waiting on it.
pub struct WorktreeLifecycleJob {
    reservation: Box<dyn Reservation>,
    mutation: Box<dyn WorktreeMutation>,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}

impl WorktreeLifecycleJob {
    pub fn new(reservation: Box<dyn Reservation>, mutation: Box<dyn WorktreeMutation>) -> Self {
        WorktreeLifecycleJob {
            reservation,
            mutation,
            #[cfg(test)]
            gate: None,
        }
    }

    /// Tests only: hold this job inside its git phase. Set for every job in one
    /// place, [`AppState::defer_lifecycle`].
    #[cfg(test)]
    pub fn hold_at(&mut self, gate: Option<crate::app::OffLockGate>) {
        self.gate = gate;
    }

    /// The lock-free phase. Consumes the job so nothing can run it twice.
    pub fn run(self) -> LifecycleOutcome {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        LifecycleOutcome {
            reservation: self.reservation,
            result: self.mutation.perform(),
        }
    }
}

/// The run half of one verb. One impl per verb, chosen where that verb decides.
pub trait WorktreeMutation: Send {
    /// Lock-free and self-contained: whatever this verb needs — a cloned
    /// [`Orchestrator`], a base branch, a slug — is this impl's own field.
    /// Consumes itself into the apply half, typed.
    fn perform(self: Box<Self>) -> Result<Performed, String>;
}

/// The apply half of one verb, built by that verb's [`WorktreeMutation`].
pub trait LifecycleEpilogue: Send {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String>;
}

/// What the git left behind, and who writes it down.
pub struct Performed {
    pub change: WorktreeChange,
    pub epilogue: Box<dyn LifecycleEpilogue>,
}

/// What one mutation did to the checkouts a project's board lists, in the terms
/// the scan cache is amended with — never a rescan of the whole repository.
#[derive(Default)]
pub struct WorktreeChange {
    pub appeared: Vec<ExternalWorktree>,
    pub gone: Vec<PathBuf>,
    /// The mutation changed a checkout it could not describe, so the amendment
    /// is not enough and the project's scan is claimed instead.
    pub rescan: bool,
}

impl WorktreeChange {
    pub fn nothing() -> Self {
        WorktreeChange::default()
    }

    pub fn appeared(worktree: ExternalWorktree) -> Self {
        WorktreeChange {
            appeared: vec![worktree],
            ..WorktreeChange::default()
        }
    }

    /// A checkout is on disk that nothing could describe. The board finds it
    /// through the scan this claims rather than through an amendment.
    pub fn undescribed() -> Self {
        WorktreeChange {
            rescan: true,
            ..WorktreeChange::default()
        }
    }
}

/// One finished job, on its way back under the app mutex.
pub struct LifecycleOutcome {
    pub reservation: Box<dyn Reservation>,
    pub result: Result<Performed, String>,
}

/// The board's carrier for a verb in flight: a row that stands where the real
/// record will be, from the decide phase's acquisition until the epilogue
/// replaces it.
pub struct PendingRow {
    /// The id the settled record is expected to carry — for a create, the
    /// checkout id its path will hash to; for a dispatch, the run it opens.
    pub entity_id: String,
    pub project_id: String,
    pub title: String,
    /// The branch this verb is claiming, when it names one. A second verb
    /// claiming the same branch is refused while this row stands: the checkout
    /// it would work in does not exist yet, so the branch is the only identity
    /// the two share.
    pub branch: Option<String>,
    pub state: PendingState,
    /// The existing card this verb acts on, when there is one: the state is
    /// rendered on that card rather than as a second row.
    pub checkout_id: Option<String>,
    pub since: Instant,
}

/// What is happening to the row. Rendered, never branched on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    Creating,
    Discarding,
}

impl PendingState {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingState::Creating => "creating",
            PendingState::Discarding => "discarding",
        }
    }
}

/// What the decide phase reserved, and what undoes it.
pub trait Reservation: Send {
    fn row(&self) -> &PendingRow;
    /// Undo the rest — the registry writes, not the row, which
    /// [`AppState::apply_lifecycle`] releases either way. Called only when the
    /// mutation failed: a mutation that succeeded is settled by its epilogue,
    /// which writes the real record where the placeholder stood.
    fn roll_back(self: Box<Self>, state: &mut AppState);
}

/// A verb whose whole claim is its row: the name a create took, or the checkout
/// a dispatch is acting on. Nothing was taken out of the registry, so there is
/// nothing to put back.
pub struct ReservedRow {
    /// The board's own row, shared rather than copied: there is one row, and
    /// what the board renders and what the job settles cannot drift.
    row: std::sync::Arc<PendingRow>,
}

impl ReservedRow {
    pub fn new(row: std::sync::Arc<PendingRow>) -> Self {
        ReservedRow { row }
    }
}

impl Reservation for ReservedRow {
    fn row(&self) -> &PendingRow {
        &self.row
    }

    fn roll_back(self: Box<Self>, _state: &mut AppState) {}
}

/// `worktree.create` — cut a branch off the project's base and add a checkout
/// for it, with nothing attached: no run, no agent, no session.
pub struct CreateWorktree {
    pub project: Orchestrator,
    pub project_id: String,
    pub base_branch: String,
    pub slug: String,
    /// The id the decide phase put on the board. The settled checkout carries
    /// it too unless the slug had to be suffixed, and the epilogue ships both.
    pub placeholder_id: String,
}

impl WorktreeMutation for CreateWorktree {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let worktree = self
            .project
            .create_bare_worktree(&self.slug, &self.base_branch)
            .map_err(|error| error.to_string())?;
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
            epilogue: Box::new(crate::app::WorktreeCreated {
                project_id: self.project_id,
                placeholder_id: self.placeholder_id,
                worktree_id,
                branch: worktree.branch(),
                name: worktree.name,
                path,
            }),
        })
    }
}

/// `branch.dispatch` — reach the checkout the instruction is meant for: the
/// bare one already on that branch, or one cut for it. Whichever it is, the
/// checkout comes back checkpointed and scaffolded, ready for the run the
/// epilogue opens around it.
pub struct DispatchCheckout {
    pub project: Orchestrator,
    pub project_id: String,
    pub base_branch: String,
    pub run_id: String,
    /// The branch the caller named, if any. With none, the instruction names
    /// the branch this cuts.
    pub branch: Option<String>,
    pub instruction: String,
    /// Checkouts a run already owns, excluded from the scan exactly as the
    /// board excludes them.
    pub excluded: HashSet<PathBuf>,
    pub model_choice: ModelChoice,
    #[cfg(test)]
    pub fault: Option<crate::app::BranchDispatchStep>,
}

impl WorktreeMutation for DispatchCheckout {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let (checkout, minted) = self.open_checkout()?;
        match self.take_ownership(&checkout) {
            Ok(performed) => Ok(performed),
            Err(error) => {
                // What this call cut, this call removes. A checkout it merely
                // found is handed back with every file intact.
                if let Some(minted) = minted {
                    match minted.branch_was_cut {
                        true => self.project.discard_worktree(&minted.worktree),
                        false => self
                            .project
                            .discard_checkout_keeping_branch(&minted.worktree),
                    }
                }
                Err(error)
            }
        }
    }
}

impl DispatchCheckout {
    /// The checkout this dispatch works in, and what it had to create to have
    /// one. A named branch a run already owns never reaches here — that
    /// dispatch joins the run and builds no job at all.
    fn open_checkout(&self) -> Result<(ExternalWorktree, Option<NamedBranchCheckout>), String> {
        // Forced rather than cached: a dispatch decides against the checkouts
        // that exist now, not against a summary from a scan interval ago.
        let found = match &self.branch {
            Some(branch) => self
                .project
                .scan_checkouts(&self.base_branch, &self.excluded)
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|checkout| checkout.branch.as_deref() == Some(branch.as_str())),
            None => None,
        };
        if let Some(checkout) = found {
            return Ok((checkout, None));
        }
        let minted = self.cut_branch()?;
        match self
            .project
            .describe_checkout(&minted.worktree.path, &self.base_branch)
        {
            Ok(checkout) => Ok((checkout, Some(minted))),
            Err(error) => {
                match minted.branch_was_cut {
                    true => self.project.discard_worktree(&minted.worktree),
                    false => self
                        .project
                        .discard_checkout_keeping_branch(&minted.worktree),
                }
                Err(error.to_string())
            }
        }
    }

    /// Cut the branch this dispatch has nowhere else to put its work.
    ///
    /// A `branch` that is already a branch name is used exactly as it stands —
    /// the caller named a ref, and re-deriving one from it is how
    /// `build/csv-export` became `build/build-csv-export`. Anything else is
    /// words about the work (the router's guess, or the instruction itself when
    /// no branch was named), and words are slugified into Build's namespace.
    fn cut_branch(&self) -> Result<NamedBranchCheckout, String> {
        let named = self
            .branch
            .as_deref()
            .filter(|name| crate::worktree::is_usable_branch_name(name));
        if let Some(name) = named {
            return self
                .project
                .create_worktree_on_named_branch(name, &self.base_branch)
                .map_err(|error| error.to_string());
        }
        let words = self.branch.as_deref().unwrap_or(&self.instruction);
        if !words.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Err(format!(
                "branch.dispatch: {words:?} has no letter or number to name a branch after"
            ));
        }
        Ok(NamedBranchCheckout {
            worktree: self
                .project
                .create_bare_worktree(&crate::worktree::slugify(words), &self.base_branch)
                .map_err(|error| error.to_string())?,
            branch_was_cut: true,
        })
    }

    /// Judge the checkout, then write Build's ownership into it: the checkpoint
    /// commit that keeps pre-Build work its own legible commit, and the
    /// `.build/` scaffold. Nothing here touches the run record.
    fn take_ownership(&self, checkout: &ExternalWorktree) -> Result<Performed, String> {
        #[cfg(test)]
        self.fail_at(crate::app::BranchDispatchStep::Adopt)?;
        let adoptable =
            AdoptableCheckout::judge(checkout, &self.base_branch, AdoptionScope::ExternalWorktree)
                .map_err(|error| error.to_string())?;
        self.project
            .prepare_adoption(&adoptable, &self.base_branch, &self.run_id)
            .map_err(|error| error.to_string())?;
        #[cfg(test)]
        self.fail_at(crate::app::BranchDispatchStep::Post)?;
        Ok(Performed {
            change: WorktreeChange::appeared(checkout.clone()),
            epilogue: Box::new(crate::app::BranchDispatched {
                project_id: self.project_id.clone(),
                run_id: self.run_id.clone(),
                base_branch: self.base_branch.clone(),
                checkout: adoptable,
                instruction: self.instruction.clone(),
                model_choice: self.model_choice.clone(),
            }),
        })
    }

    /// Fail this dispatch where a test asked it to.
    #[cfg(test)]
    fn fail_at(&self, step: crate::app::BranchDispatchStep) -> Result<(), String> {
        if self.fault == Some(step) {
            return Err(format!("branch.dispatch: injected failure at {step:?}"));
        }
        Ok(())
    }
}
