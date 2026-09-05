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
//! every input it needs by value and its `perform` takes no argument, so while
//! the git runs there is no `&mut AppState` in reach to touch — the rule that
//! the lock is free is enforced by the signature rather than remembered. What
//! the git produced travels back as a [`LifecycleEpilogue`], which is the only
//! half that touches state — and lives beside the state it writes.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::app::AppState;
use crate::models::ModelChoice;
use crate::orchestrator::{AdoptableCheckout, AdoptionScope, ImplementableIssue, Orchestrator};
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
    /// Build the job that will settle `row`. The only way to make one, and it
    /// takes the reserved row itself rather than a reservation the caller had
    /// to assemble: a row on the board with no job behind it is never released.
    pub fn reserving(row: std::sync::Arc<PendingRow>, mutation: Box<dyn WorktreeMutation>) -> Self {
        WorktreeLifecycleJob {
            reservation: Box::new(ReservedRow { row }),
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
    /// [`Orchestrator`], a base branch, a slug — is this impl's own field, and
    /// the empty argument list is what makes that a rule: there is no
    /// `&mut AppState` here to reach state through while the git runs.
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
struct ReservedRow {
    /// The board's own row, shared rather than copied: there is one row, and
    /// what the board renders and what the job settles cannot drift.
    row: std::sync::Arc<PendingRow>,
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

/// `run.create` and `issue.implement_*` — cut the checkout an Issue's
/// implementation works in and make it ready: scaffolded, with the Issue's
/// canonical docs committed as the baseline the review diff is read against.
pub struct OpenImplementation {
    pub project: Orchestrator,
    pub project_id: String,
    pub issue_id: String,
    pub issue: ImplementableIssue,
    pub base_branch: String,
    pub run_id: String,
    pub store: crate::store::Store,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn crate::app::ImplementationCaller>,
}

impl WorktreeMutation for OpenImplementation {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let prepared = self.project.prepare_run_checkout(
            &self.issue,
            &self.base_branch,
            &self.run_id,
            &self.store,
        );
        let epilogue: Box<dyn LifecycleEpilogue> = match prepared {
            Ok(prepared) => Box::new(crate::app::ImplementationOpened {
                project_id: self.project_id,
                issue_id: self.issue_id,
                run_id: self.run_id,
                prepared,
                model_choice: self.model_choice,
                caller: self.caller,
            }),
            // The checkout it cut, if it got that far, is already removed —
            // what is left to settle is what the Issue says it was doing.
            Err(error) => Box::new(crate::app::ImplementationRefused {
                error: error.to_string(),
                caller: self.caller,
            }),
        };
        Ok(Performed {
            // A checkout a run owns is on no unbound list, so there is nothing
            // for the board's scan to be told about either way.
            change: WorktreeChange::nothing(),
            epilogue,
        })
    }
}

/// The same, into a checkout that already exists: whatever the branch was
/// carrying is checkpointed under its own message first, so the baseline commit
/// is exactly what the implementation adds to it.
pub struct AdoptImplementation {
    pub project: Orchestrator,
    pub project_id: String,
    pub issue_id: String,
    pub issue: ImplementableIssue,
    pub run_id: String,
    pub checkout: PathBuf,
    pub store: crate::store::Store,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn crate::app::ImplementationCaller>,
}

impl WorktreeMutation for AdoptImplementation {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let prepared =
            self.project
                .prepare_adopted_checkout(&self.issue, &self.checkout, &self.store);
        let epilogue: Box<dyn LifecycleEpilogue> = match prepared {
            Ok(base_sha) => Box::new(crate::app::ImplementationAdopted {
                project_id: self.project_id,
                issue_id: self.issue_id,
                run_id: self.run_id,
                base_sha,
                model_choice: self.model_choice,
                caller: self.caller,
            }),
            Err(error) => Box::new(crate::app::ImplementationRefused {
                error: error.to_string(),
                caller: self.caller,
            }),
        };
        Ok(Performed {
            change: WorktreeChange::nothing(),
            epilogue,
        })
    }
}

/// `issue.implement_*` — put back the checkout an Issue's implementation lost,
/// from the exact branch its run recorded. `git worktree add`, and a fetch when
/// the branch survives only on a remote.
pub struct RestoreImplementationCheckout {
    pub project: Orchestrator,
    pub issue_id: String,
    pub run_id: String,
    pub worktree: crate::worktree::Worktree,
    pub checkout_stood: bool,
    pub caller: Box<dyn crate::app::ImplementationCaller>,
}

impl WorktreeMutation for RestoreImplementationCheckout {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        // A branch that is gone is a finding, not a failure of this job: the
        // apply phase hands the run to the recovery agent over it.
        let restored = self
            .project
            .restore_run_worktree(&self.worktree)
            .map_err(|error| error.to_string());
        Ok(Performed {
            change: WorktreeChange::nothing(),
            epilogue: Box::new(crate::app::RestoredCheckout {
                issue_id: self.issue_id,
                run_id: self.run_id,
                checkout_stood: self.checkout_stood,
                restored,
                caller: self.caller,
            }),
        })
    }
}

/// `plan.create` with a session — the workspace its planning agent works in:
/// the scratch docs dir this Issue alone writes into, and the `.build/` config
/// in the primary checkout that routes the agent's `done` reports back to it.
///
/// Directories and files, on a checkout that may be huge and on a disk that may
/// be busy: off the app mutex like every other verb's disk.
pub struct OpenPlanWorkspace {
    pub project: Orchestrator,
    pub project_id: String,
    pub plan_id: String,
    pub goal: String,
    pub base_branch: String,
    pub model_choice: ModelChoice,
    pub detail: crate::thread::ThreadDetail,
}

impl WorktreeMutation for OpenPlanWorkspace {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let workspace = self
            .project
            .prepare_plan_workspace(&self.plan_id)
            .map_err(|error| error.to_string())?;
        Ok(Performed {
            // No checkout was cut: a plan is written against the primary one.
            change: WorktreeChange::nothing(),
            epilogue: Box::new(crate::app::PlanWorkspaceOpened {
                project_id: self.project_id,
                plan_id: self.plan_id,
                goal: self.goal,
                base_branch: self.base_branch,
                model_choice: self.model_choice,
                workspace,
                detail: self.detail,
            }),
        })
    }
}

/// Tests only: where to fail a `branch.dispatch`, so the cleanup that has to
/// undo what the call created can be exercised at each seam it opens. One
/// variant is one seam, and every seam is checked through [`fail_dispatch_at`].
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchDispatchStep {
    /// The git phase, after the checkout is resolved or cut, before Build's
    /// ownership is written into it.
    Adopt,
    /// The git phase, after the checkpoint and the scaffold, before the run
    /// record exists.
    Own,
    /// The apply phase, under the mutex again, before the run this dispatch's
    /// checkout is opened around.
    Open,
    /// The apply phase, after the agent has been handed the instruction, before
    /// the write that makes its run real — the window where a turn is queued
    /// for a run the store never got.
    Settle,
    /// A branch Build already runs: before its new agent and first message land.
    Post,
}

/// Fail a dispatch where a test asked it to. One carrier for the injected
/// fault, so a seam cannot be checked in one arm and forgotten in another.
#[cfg(test)]
pub fn fail_dispatch_at(
    fault: Option<BranchDispatchStep>,
    step: BranchDispatchStep,
) -> Result<(), String> {
    if fault == Some(step) {
        return Err(format!("branch.dispatch: injected failure at {step:?}"));
    }
    Ok(())
}

/// Where one dispatch's work goes, named before any git runs.
///
/// A branch the caller spelled out is used exactly as it stands — re-deriving a
/// name from a name is how `build/csv-export` became `build/build-csv-export`.
/// Anything else is words about the work (the router's guess, or the
/// instruction itself when no branch was named), and words are slugified into
/// Build's namespace.
///
/// It is settled under the app mutex because it is what one dispatch reserves
/// against another: two calls that name the same ref must collide on the board
/// rather than in `git worktree add`.
pub enum DispatchTarget {
    /// The caller named a ref. The checkout already on it is where the work
    /// goes; with none, the ref is cut exactly as given.
    Named(String),
    /// Nobody named a ref, so the words did. The branch is this call's own, and
    /// nothing already on disk is taken over into it.
    Minted { branch: String, slug: String },
}

impl DispatchTarget {
    /// Read the ref out of what the caller said. Words with nothing to name a
    /// branch after are refused here, before anything is reserved or cut.
    pub fn of(branch: Option<&str>, instruction: &str) -> Result<DispatchTarget, String> {
        if let Some(name) = branch.filter(|name| crate::worktree::is_usable_branch_name(name)) {
            return Ok(DispatchTarget::Named(name.to_string()));
        }
        let words = branch.unwrap_or(instruction);
        if !words.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Err(format!(
                "branch.dispatch: {words:?} has no letter or number to name a branch after"
            ));
        }
        let slug = crate::worktree::slugify(words);
        Ok(DispatchTarget::Minted {
            branch: crate::worktree::branch_name_for(&slug),
            slug,
        })
    }

    /// The ref this dispatch claims: what the board's row stands for, and what
    /// a second claim on it collides with.
    pub fn branch(&self) -> &str {
        match self {
            DispatchTarget::Named(branch) => branch,
            DispatchTarget::Minted { branch, .. } => branch,
        }
    }

    /// The branch whose existing checkout this dispatch takes over, when there
    /// is one to take: a ref the caller named may already have work in it, while
    /// a ref minted from words is this call's alone.
    pub fn adopted_branch(&self) -> Option<&str> {
        match self {
            DispatchTarget::Named(branch) => Some(branch),
            DispatchTarget::Minted { .. } => None,
        }
    }

    /// Add the checkout this dispatch's work goes in. A named ref is cut under
    /// that exact name (or checked out, when it is already a branch); a minted
    /// one goes through the slug namespace, which suffixes rather than collides.
    fn cut(
        &self,
        project: &Orchestrator,
        base_branch: &str,
    ) -> Result<NamedBranchCheckout, String> {
        match self {
            DispatchTarget::Named(branch) => project
                .create_worktree_on_named_branch(branch, base_branch)
                .map_err(|error| error.to_string()),
            DispatchTarget::Minted { slug, .. } => Ok(NamedBranchCheckout {
                worktree: project
                    .create_bare_worktree(slug, base_branch)
                    .map_err(|error| error.to_string())?,
                branch_was_cut: true,
            }),
        }
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
    /// The ref this dispatch claims, settled before any git ran.
    pub target: DispatchTarget,
    pub instruction: String,
    /// Checkouts a run already owns, excluded from the scan exactly as the
    /// board excludes them.
    pub excluded: HashSet<PathBuf>,
    pub model_choice: ModelChoice,
    /// The capture this dispatch is the destination of, when a route is what
    /// asked for it. Written down by the apply phase, against the branch that
    /// is real by then.
    pub routed: Option<crate::app::RoutedCapture>,
    #[cfg(test)]
    pub fault: Option<BranchDispatchStep>,
}

impl WorktreeMutation for DispatchCheckout {
    fn perform(mut self: Box<Self>) -> Result<Performed, String> {
        // A checkout that was already there is never this call's to remove:
        // taking ownership of one touches nothing that has to be put back.
        if let Some(found) = self.find_checkout()? {
            return self.take_ownership(&found);
        }
        let minted = self.cut_branch()?;
        let described = self
            .project
            .describe_checkout(&minted.worktree.path, &self.base_branch)
            .map_err(|error| error.to_string());
        let dispatched = match described {
            Ok(checkout) => self.take_ownership(&checkout),
            Err(error) => Err(error),
        };
        if dispatched.is_err() {
            // What this call cut, this call removes — and the branch under it
            // only if this call cut that too.
            match minted.branch_was_cut {
                true => self.project.discard_worktree(&minted.worktree),
                false => self
                    .project
                    .discard_checkout_keeping_branch(&minted.worktree),
            }
        }
        dispatched
    }
}

impl DispatchCheckout {
    /// The bare checkout of the branch this dispatch names, if this project has
    /// one. A dispatch that named no branch has nothing to look for: it always
    /// cuts a new branch rather than adopting whatever is lying around. A named
    /// branch a run already owns never reaches here — that dispatch joins the
    /// run and builds no job at all.
    fn find_checkout(&self) -> Result<Option<ExternalWorktree>, String> {
        let Some(branch) = self.target.adopted_branch() else {
            return Ok(None);
        };
        // Forced rather than cached: a dispatch decides against the checkouts
        // that exist now, not against a summary from a scan interval ago.
        Ok(self
            .project
            .scan_checkouts(&self.base_branch, &self.excluded)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|checkout| checkout.branch.as_deref() == Some(branch)))
    }

    /// Cut the branch this dispatch has nowhere else to put its work.
    fn cut_branch(&self) -> Result<NamedBranchCheckout, String> {
        self.target.cut(&self.project, &self.base_branch)
    }

    /// Take Build's ownership of the checkout this dispatch reached, and owe the
    /// apply phase the instruction on top of it.
    fn take_ownership(&mut self, checkout: &ExternalWorktree) -> Result<Performed, String> {
        #[cfg(test)]
        fail_dispatch_at(self.fault, BranchDispatchStep::Adopt)?;
        let adopted = adopt(
            &self.project,
            &self.project_id,
            checkout,
            &self.base_branch,
            AdoptionScope::ExternalWorktree,
            &self.run_id,
            self.model_choice.clone(),
        )?;
        #[cfg(test)]
        fail_dispatch_at(self.fault, BranchDispatchStep::Own)?;
        Ok(Performed {
            change: WorktreeChange::appeared(checkout.clone()),
            epilogue: Box::new(crate::app::BranchDispatched {
                adopted,
                instruction: self.instruction.clone(),
                routed: self.routed.take(),
            }),
        })
    }
}

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
) -> Result<crate::app::RunAdopted, String> {
    let adoptable = AdoptableCheckout::judge(checkout, base_branch, scope)
        .map_err(|error| error.to_string())?;
    project
        .prepare_adoption(&adoptable, base_branch, run_id)
        .map_err(|error| error.to_string())?;
    Ok(crate::app::RunAdopted {
        project_id: project_id.to_string(),
        run_id: run_id.to_string(),
        base_branch: base_branch.to_string(),
        checkout: adoptable,
        scope,
        model_choice,
    })
}
