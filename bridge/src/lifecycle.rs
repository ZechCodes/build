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
    /// The project whose board renders this row. A project verb has none: there
    /// is no project id until its git lands, and what it settles into is the
    /// project itself rather than a card, so nothing renders it.
    pub project_id: Option<String>,
    pub title: String,
    pub branch: Option<String>,
    pub state: PendingState,
    pub checkout_id: Option<String>,
    pub implements: Option<String>,
    pub since: Instant,
}

impl PendingRow {
    /// A card the board is about to have: the run or checkout `entity_id` names
    /// once the git lands.
    pub fn creating(entity_id: String, project_id: Option<String>, title: String) -> PendingRow {
        PendingRow::of(entity_id, project_id, title, PendingState::Creating)
    }

    /// A card the board is about to lose.
    pub fn discarding(entity_id: String, project_id: Option<String>, title: String) -> PendingRow {
        PendingRow::of(entity_id, project_id, title, PendingState::Discarding)
    }

    /// A row for the directory a project verb reached for, which is the only
    /// identity two of them share before either has a project id: two clones
    /// into one folder, or two remotes written into one repository, collide
    /// here. No card stands where it does, so no board renders it.
    pub fn on_directory(entity_id: String, title: String, state: PendingState) -> PendingRow {
        PendingRow::of(entity_id, None, title, state)
    }

    fn of(
        entity_id: String,
        project_id: Option<String>,
        title: String,
        state: PendingState,
    ) -> PendingRow {
        PendingRow {
            entity_id,
            project_id,
            title,
            branch: None,
            state,
            checkout_id: None,
            implements: None,
            since: Instant::now(),
        }
    }

    /// The branch this verb is claiming. A second verb claiming the same branch
    /// is refused while this row stands: the checkout it would work in does not
    /// exist yet, so the branch is the only identity the two share.
    pub fn on_branch(self, branch: String) -> PendingRow {
        PendingRow {
            branch: Some(branch),
            ..self
        }
    }

    /// The existing card this verb acts on: the state is rendered on that card
    /// rather than as a second row, and a second verb reaching for the same
    /// checkout is refused while this row stands.
    pub fn on_checkout(self, checkout_id: String) -> PendingRow {
        PendingRow {
            checkout_id: Some(checkout_id),
            ..self
        }
    }

    /// The Issue this verb is opening an implementation of. An Issue has one
    /// active writer, and the run that will be it is not in the run map until
    /// the git has landed — so the row is the gate for the length of that git,
    /// and a second implementation of the same Issue is refused while it
    /// stands.
    pub fn implementing(self, issue_id: String) -> PendingRow {
        PendingRow {
            implements: Some(issue_id),
            ..self
        }
    }
}

/// What is happening to the row. Rendered, never branched on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    Creating,
    Discarding,
    /// A record that already exists is being rewritten in place. Nothing is
    /// minted and nothing is taken away; what the row holds is the right to be
    /// the one verb rewriting it.
    Updating,
}

impl PendingState {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingState::Creating => "creating",
            PendingState::Discarding => "discarding",
            PendingState::Updating => "updating",
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
        let prepared = match prepared {
            Ok(prepared) => prepared,
            // The checkout it cut, if it got that far, is already removed —
            // what is left to settle is what the Issue says it was doing.
            Err(error) => {
                return Ok(Performed {
                    change: WorktreeChange::nothing(),
                    epilogue: Box::new(crate::app::ImplementationRefused {
                        error: error.to_string(),
                        caller: self.caller,
                    }),
                })
            }
        };
        // The run that will own this checkout takes it back off the unbound
        // list in the epilogue. Naming it here is what puts it on the board if
        // that epilogue never gets there: a checkout on disk under no run,
        // invisible until the next full rescan, is how a minted one gets lost.
        let change = match self
            .project
            .describe_checkout(&prepared.worktree.path, &self.base_branch)
        {
            Ok(checkout) => WorktreeChange::appeared(checkout),
            Err(error) => {
                eprintln!(
                    "describing the implementation checkout at {}: {error}",
                    prepared.worktree.path.display()
                );
                WorktreeChange::undescribed()
            }
        };
        Ok(Performed {
            change,
            epilogue: Box::new(crate::app::ImplementationOpened {
                project_id: self.project_id,
                issue_id: self.issue_id,
                run_id: self.run_id,
                prepared,
                model_choice: self.model_choice,
                caller: self.caller,
            }),
        })
    }
}

/// The checkout an Issue's implementation is being handed, and what reaching it
/// costs. The type owns its own variation — which git runs, which run the
/// implementation is written onto — so no caller matches on it.
pub enum ImplementationCheckout {
    /// A checkout a run already owns. Its path is known, nothing has to be
    /// taken over, and the run is the one already on the board.
    Owned(PathBuf),
    /// A checkout no run owns yet. Build takes ownership of it first — the same
    /// adoption `run.adopt` runs — and the run this implementation is written
    /// onto is the one that adoption mints.
    Unowned {
        target: AdoptionTarget,
        base_branch: String,
    },
}

/// The checkout an implementation will be written into, as the git left it.
struct ReachedCheckout {
    path: PathBuf,
    change: WorktreeChange,
    /// The run the adoption on the way here minted, when the checkout had no
    /// owner. The epilogue opens it instead of taking one off the board.
    adopted: Option<crate::app::RunAdopted>,
}

impl ImplementationCheckout {
    fn reach(
        self,
        project: &Orchestrator,
        project_id: &str,
        run_id: &str,
        model_choice: &ModelChoice,
    ) -> Result<ReachedCheckout, String> {
        match self {
            ImplementationCheckout::Owned(path) => Ok(ReachedCheckout {
                path,
                change: WorktreeChange::nothing(),
                adopted: None,
            }),
            ImplementationCheckout::Unowned {
                target,
                base_branch,
            } => {
                let checkout = target.reach(project, &base_branch)?;
                let adopted = adopt(
                    project,
                    project_id,
                    &checkout,
                    &base_branch,
                    target.scope(),
                    run_id,
                    model_choice.clone(),
                )?;
                Ok(ReachedCheckout {
                    path: checkout.path.clone(),
                    change: target.amendment(&checkout),
                    adopted: Some(adopted),
                })
            }
        }
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
    pub checkout: ImplementationCheckout,
    pub store: crate::store::Store,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn crate::app::ImplementationCaller>,
}

impl WorktreeMutation for AdoptImplementation {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let AdoptImplementation {
            project,
            project_id,
            issue_id,
            issue,
            run_id,
            checkout,
            store,
            model_choice,
            caller,
        } = *self;
        let prepared = (|| -> Result<(ReachedCheckout, String), String> {
            let reached = checkout.reach(&project, &project_id, &run_id, &model_choice)?;
            let base_sha = project
                .prepare_adopted_checkout(&issue, &reached.path, &store)
                .map_err(|error| error.to_string())?;
            Ok((reached, base_sha))
        })();
        Ok(match prepared {
            Ok((reached, base_sha)) => Performed {
                change: reached.change,
                epilogue: Box::new(crate::app::ImplementationAdopted {
                    project_id,
                    issue_id,
                    run_id,
                    base_sha,
                    adopted: reached.adopted,
                    model_choice,
                    caller,
                }),
            },
            // A checkout Build took over on the way here keeps what the
            // adoption wrote into it — a checkpoint commit and a scaffold —
            // and no run is opened around it: the next scan lists it as the
            // card it was.
            Err(error) => Performed {
                change: WorktreeChange::nothing(),
                epilogue: Box::new(crate::app::ImplementationRefused { error, caller }),
            },
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

/// The workspace an Issue's planning agent works in: the scratch docs dir this
/// Issue alone writes into, holding the docs as they stand, and the `.build/`
/// config in the primary checkout that routes the agent's `done` reports back
/// to it.
///
/// Directories and files, on a checkout that may be huge and on a disk that may
/// be busy: off the app mutex like every other verb's disk. Every door to a
/// planning agent — the first dispatch, a batch of notes, one stage's comments,
/// a freeform message, a resume — is this one mutation and its own
/// [`PlanSessionOpening`](crate::app::PlanSessionOpening), which is what keeps
/// the workspace off the lock at all of them rather than at one.
pub struct OpenPlanWorkspace {
    pub project: Orchestrator,
    pub plan_id: String,
    pub store: crate::store::Store,
    /// What this door does with the workspace once it is real.
    pub opening: Box<dyn crate::app::PlanSessionOpening>,
}

impl WorktreeMutation for OpenPlanWorkspace {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let prepared = self
            .project
            .prepare_plan_workspace(&self.plan_id, &self.store);
        let epilogue: Box<dyn LifecycleEpilogue> = match prepared {
            Ok(workspace) => Box::new(crate::app::PlanWorkspaceOpened {
                workspace,
                opening: self.opening,
            }),
            // A door decides for itself what an unwritable workspace means —
            // an error to the caller, or a routed capture that says no agent
            // is reading it — so the refusal travels as an epilogue.
            Err(error) => Box::new(crate::app::PlanWorkspaceRefused {
                error: error.to_string(),
                opening: self.opening,
            }),
        };
        Ok(Performed {
            // No checkout was cut: a plan is written against the primary one.
            change: WorktreeChange::nothing(),
            epilogue,
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
            self.project
                .discard_checkout(&minted.worktree, !minted.branch_was_cut);
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

    fn reach(&self, project: &Orchestrator, base_branch: &str) -> Result<ExternalWorktree, String> {
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
            AdoptionTarget::Primary { repo_path } => {
                crate::worktree::describe_primary_checkout(repo_path, base_branch)
                    .map_err(|error| error.to_string())
            }
        }
    }

    /// What the board's checkout list is told while the run is being opened. A
    /// card goes back on the board if that record fails; the primary checkout
    /// was never a card and must not become one.
    fn amendment(&self, checkout: &ExternalWorktree) -> WorktreeChange {
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
    /// How much of the run's conversation the caller asked to be answered with.
    pub detail: crate::thread::ThreadDetail,
}

impl WorktreeMutation for AdoptCheckout {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
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
            epilogue: Box::new(crate::app::RunAdoptionSettled {
                adopted,
                detail: self.detail,
            }),
        })
    }
}

/// What a run being taken off the board does with the directory it was working
/// in.
///
/// The type owns its own variation — which git runs, and what the board's
/// checkout list is told afterwards — so no caller matches on it.
pub enum DiscardedCheckout {
    /// A worktree Build minted or adopted, whose run is being abandoned: the
    /// directory goes and the branch stays, because a run's work outlives the
    /// run so it can be re-attempted.
    Removed {
        project: Orchestrator,
        worktree: crate::worktree::Worktree,
    },
    /// A worktree whose card is being cleared off the board altogether: the
    /// directory and the branch under it both go.
    Pruned {
        project: Orchestrator,
        worktree: crate::worktree::Worktree,
    },
    /// Nothing on disk is touched: the project's primary checkout, which IS the
    /// repository, a checkout adopted from the user, whose files are theirs, or
    /// a run whose project is no longer registered, which leaves no
    /// [`Orchestrator`] to prune with. The project rides in the two arms that
    /// need one so that the arm that touches no disk cannot be asked for one.
    Kept,
}

impl DiscardedCheckout {
    /// The directory this takes away, whose project takes it, and whether the
    /// branch under it stays.
    fn removal(&self) -> Option<(&Orchestrator, &crate::worktree::Worktree, bool)> {
        match self {
            DiscardedCheckout::Kept => None,
            DiscardedCheckout::Removed { project, worktree } => Some((project, worktree, true)),
            DiscardedCheckout::Pruned { project, worktree } => Some((project, worktree, false)),
        }
    }

    /// Let go of the directory, waiting out the agents that were writing into
    /// it first — only when there is a walk for a live child to trip.
    fn discard(&self, writers: &[crate::reaper::Retirement], run_id: &str) -> WorktreeChange {
        let Some((project, worktree, keep_branch)) = self.removal() else {
            return WorktreeChange::nothing();
        };
        for writer in writers {
            if !writer.wait(crate::orchestrator::CHECKOUT_REAP_WAIT) {
                eprintln!(
                    "{run_id}: an agent did not die within {:?}; removing its checkout anyway",
                    crate::orchestrator::CHECKOUT_REAP_WAIT
                );
            }
        }
        project.discard_checkout(worktree, keep_branch);
        match worktree.path.exists() {
            false => WorktreeChange {
                gone: vec![crate::worktree::canonical_root(&worktree.path)],
                ..WorktreeChange::default()
            },
            // The removal is best-effort and it did not get there. What stands
            // is a checkout no run owns any more, which is a card — and only a
            // scan can describe it.
            true => WorktreeChange::undescribed(),
        }
    }
}

/// `run.abandon` and `run.delete` — take a run off the board and let go of the
/// checkout it was working in.
///
/// Kill, reap, THEN remove: `remove_dir_all` walking a directory a child is
/// still creating files in fails the walk, so the agents the decide phase
/// retired are waited out here, where a harness wedged in uninterruptible I/O
/// parks this job and nothing else.
///
/// Nothing here can fail. Every step is best-effort by contract — git's verdict
/// on the stages, the reap, the removal — and the run itself is riding along,
/// so a `perform` that could return `Err` would be a run stranded off the
/// board. What is written down is the apply half's, under the mutex.
pub struct DiscardCheckout {
    pub checkout: DiscardedCheckout,
    /// The agents the decide phase killed, waited out here before the removal.
    pub retirements: Vec<crate::reaper::Retirement>,
    /// The run itself, off the board for the length of the removal so nothing
    /// answers verbs against a checkout that is being deleted.
    pub active: Box<crate::orchestrator::ActiveRun>,
    pub run_id: String,
    /// What the verb that asked for this still owes the records.
    pub settlement: Box<dyn crate::app::DiscardSettlement>,
}

impl WorktreeMutation for DiscardCheckout {
    fn perform(mut self: Box<Self>) -> Result<Performed, String> {
        // Whatever the verb has to ask git is asked here, before the removal:
        // the refs its answer depends on are readable only until then. A verb
        // with nothing to ask pays nothing for the question.
        self.settlement.judge_before_removal();
        let change = self.checkout.discard(&self.retirements, &self.run_id);
        Ok(Performed {
            change,
            epilogue: Box::new(CheckoutDiscarded {
                active: self.active,
                settlement: self.settlement,
            }),
        })
    }
}

/// The checkout is let go of and the run is on its way back under the mutex.
/// What is written down there is the verb's own.
struct CheckoutDiscarded {
    active: Box<crate::orchestrator::ActiveRun>,
    settlement: Box<dyn crate::app::DiscardSettlement>,
}

impl LifecycleEpilogue for CheckoutDiscarded {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        self.settlement.settle(state, *self.active)
    }
}

/// Read a repository off disk and settle everything registering it needs: the
/// canonical path, the base branch — the one named, or the one the checkout is
/// standing on — proved to resolve, and the `origin` it is wired to.
///
/// The one place a project's facts are read, whichever door reached the
/// directory: opened where it stands, cloned into the projects folder, or
/// created from nothing.
fn open_repo(
    path: PathBuf,
    requested_base: Option<String>,
) -> Result<crate::app::ProjectAdded, String> {
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let repo =
        git2::Repository::open(&path).map_err(|error| format!("not a git repository: {error}"))?;
    let base = requested_base
        .or_else(|| crate::app::git_default_branch(&path))
        .unwrap_or_else(|| "main".to_string());
    repo.revparse_single(&base)
        .map_err(|_| format!("base branch '{base}' not found in repo"))?;
    Ok(crate::app::ProjectAdded {
        remote: crate::app::git_remote_origin(&path),
        path,
        base,
    })
}

/// One repository's registration, once its directory is on disk. Every project
/// door ends here, so what a project knows about itself is read in one place.
fn opened(path: PathBuf, requested_base: Option<String>) -> Result<Performed, String> {
    Ok(Performed {
        change: WorktreeChange::nothing(),
        epilogue: Box::new(open_repo(path, requested_base)?),
    })
}

/// `project.add` — register a repository where the user already keeps it.
pub struct OpenRepo {
    pub path: PathBuf,
    pub requested_base: Option<String>,
}

impl WorktreeMutation for OpenRepo {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        opened(self.path, self.requested_base)
    }
}

/// `project.clone` — put a repository in the projects folder and register it,
/// or register the one already standing there when it is the same repository.
pub struct CloneRepo {
    pub url: String,
    pub name: String,
    pub dest: PathBuf,
    pub projects_dir: PathBuf,
    pub requested_base: Option<String>,
}

impl WorktreeMutation for CloneRepo {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        if self.dest.exists() {
            if !self.dest.join(".git").exists() {
                return Err(format!(
                    "'{}' already exists in the projects folder and is not a git repo",
                    self.name
                ));
            }
            if let Some(origin) = crate::app::git_remote_origin(&self.dest) {
                if !crate::app::remotes_match(&origin, &self.url) {
                    return Err(format!(
                        "'{}' already exists with a different remote ({origin})",
                        self.name
                    ));
                }
            }
            return opened(self.dest, self.requested_base);
        }
        std::fs::create_dir_all(&self.projects_dir)
            .map_err(|error| format!("cannot create projects folder: {error}"))?;
        let cloned = std::process::Command::new("git")
            .arg("clone")
            .arg(&self.url)
            .arg(&self.dest)
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if !cloned.status.success() {
            // A clone that got far enough to make the directory leaves nothing
            // behind: a retry has to find the same empty folder this one did.
            let _ = std::fs::remove_dir_all(&self.dest);
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&cloned.stderr).trim()
            ));
        }
        opened(self.dest, self.requested_base)
    }
}

/// `project.create` — make a repository from nothing and register it, with an
/// initial commit so its base branch resolves and work can dispatch into it.
pub struct CreateRepo {
    pub name: String,
    pub parent: PathBuf,
    pub base_branch: String,
    pub remote: Option<String>,
}

impl CreateRepo {
    fn write(&self, dest: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(&self.parent)
            .map_err(|error| format!("cannot create {}: {error}", self.parent.display()))?;
        if dest.exists() {
            return Err(format!(
                "'{}' already exists in {}",
                self.name,
                self.parent.display()
            ));
        }
        std::fs::create_dir_all(dest)
            .map_err(|error| format!("cannot create {}: {error}", self.name))?;
        crate::app::git_in(dest, &["init", "-b", &self.base_branch])?;
        std::fs::write(dest.join("README.md"), format!("# {}\n", self.name))
            .map_err(|error| format!("cannot write README: {error}"))?;
        crate::app::git_in(dest, &["add", "."])?;
        // Commit with an explicit identity so it never depends on host git config.
        crate::app::git_in(
            dest,
            &[
                "-c",
                "user.email=build@build.ing",
                "-c",
                "user.name=Build",
                "commit",
                "-m",
                "Initial commit",
            ],
        )?;
        if let Some(remote) = &self.remote {
            crate::app::git_in(dest, &["remote", "add", "origin", remote])?;
        }
        Ok(())
    }
}

impl WorktreeMutation for CreateRepo {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let dest = self.parent.join(&self.name);
        let existed = dest.exists();
        if let Err(error) = self.write(&dest) {
            // Half a repository is worse than none: the retry has to start
            // where this one did. A directory that was already there is not
            // this call's to remove.
            if !existed {
                let _ = std::fs::remove_dir_all(&dest);
            }
            return Err(error);
        }
        opened(dest, Some(self.base_branch))
    }
}

/// `project.set_remote` — point a project's `origin` somewhere, or unwire it.
pub struct SetRemote {
    pub project_id: String,
    pub repo_path: PathBuf,
    /// Empty clears the remote; removing one that is not there is not an error.
    pub url: String,
}

impl WorktreeMutation for SetRemote {
    fn perform(self: Box<Self>) -> Result<Performed, String> {
        let remote = match (
            self.url.is_empty(),
            crate::app::git_remote_origin(&self.repo_path).is_some(),
        ) {
            (true, _) => {
                let _ = std::process::Command::new("git")
                    .arg("-C")
                    .arg(&self.repo_path)
                    .args(["remote", "remove", "origin"])
                    .output();
                None
            }
            (false, true) => {
                crate::app::git_in(&self.repo_path, &["remote", "set-url", "origin", &self.url])?;
                Some(self.url)
            }
            (false, false) => {
                crate::app::git_in(&self.repo_path, &["remote", "add", "origin", &self.url])?;
                Some(self.url)
            }
        };
        Ok(Performed {
            change: WorktreeChange::nothing(),
            epilogue: Box::new(crate::app::ProjectRemoteSet {
                project_id: self.project_id,
                remote,
            }),
        })
    }
}
