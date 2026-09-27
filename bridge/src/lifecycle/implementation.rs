use crate::isolation::ResolvedIsolation;
use crate::lifecycle::{adopt, AdoptionTarget};
use crate::lifecycle::{
    AdoptedImplementation, AdoptionPrepared, ImplementationPrepared, Performed, RestoredCheckout,
    WorktreeChange, WorktreeMutation,
};
use crate::models::ModelChoice;
use crate::orchestrator::{ImplementableTask, Orchestrator};
use std::path::PathBuf;

pub struct OpenImplementation {
    pub project: Orchestrator,
    pub task: ImplementableTask,
    pub base_branch: String,
    pub run_id: String,
    pub store: crate::store::Store,
    pub model_choice: ModelChoice,
    /// How this implementation's checkout is made. A fallback is said on the
    /// Task's conversation once the run stands.
    pub resolved: ResolvedIsolation,
}

impl WorktreeMutation for OpenImplementation {
    type Output = ImplementationPrepared;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let prepared = self.project.prepare_run_checkout(
            &self.task,
            &self.base_branch,
            &self.run_id,
            self.resolved.isolation,
            &self.store,
        );
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Err(error.to_string()),
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
            output: ImplementationPrepared {
                prepared,
                downgrade: self.resolved.downgrade,
            },
        })
    }
}

/// The checkout a Task's implementation is being handed, and what reaching it
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
    adopted: Option<AdoptionPrepared>,
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
    pub task: ImplementableTask,
    pub run_id: String,
    pub checkout: ImplementationCheckout,
    pub store: crate::store::Store,
    pub model_choice: ModelChoice,
}

impl WorktreeMutation for AdoptImplementation {
    type Output = AdoptedImplementation;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let AdoptImplementation {
            project,
            project_id,
            task,
            run_id,
            checkout,
            store,
            model_choice,
        } = self;
        let prepared = (|| -> Result<(ReachedCheckout, String), String> {
            let reached = checkout.reach(&project, &project_id, &run_id, &model_choice)?;
            let base_sha = project
                .prepare_adopted_checkout(&task, &reached.path, &store)
                .map_err(|error| error.to_string())?;
            Ok((reached, base_sha))
        })();
        let (reached, base_sha) = prepared?;
        Ok(Performed {
            change: reached.change,
            output: AdoptedImplementation {
                base_sha,
                adopted: reached.adopted,
            },
        })
    }
}

/// `task.implement_*` — put back the checkout a Task's implementation lost,
/// from the exact branch its run recorded. `git worktree add`, and a fetch when
/// the branch survives only on a remote.
pub struct RestoreImplementationCheckout {
    pub project: Orchestrator,
    pub worktree: crate::worktree::Worktree,
    pub checkout_stood: bool,
    /// How the checkout is put back, when it has to be. A fallback is said on
    /// the Task's conversation beside what the restore found.
    pub resolved: ResolvedIsolation,
}

impl WorktreeMutation for RestoreImplementationCheckout {
    type Output = RestoredCheckout;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        // A branch that is gone is a finding, not a failure of this job: the
        // apply phase refuses the caller with git's reason.
        let restored = self
            .project
            .restore_run_worktree(
                &self.worktree,
                crate::worktree::UnregisteredRestore::Write(
                    crate::worktree::BranchTeardown::DeletesBranch,
                ),
                self.resolved.isolation,
            )
            .map_err(|error| error.to_string());
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: RestoredCheckout {
                restored,
                checkout_stood: self.checkout_stood,
                downgrade: (!self.checkout_stood)
                    .then_some(self.resolved.downgrade)
                    .flatten(),
            },
        })
    }
}
