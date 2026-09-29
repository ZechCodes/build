use super::continuations::{ImplementationCaller, PlanSessionOpening};
use super::runner::LifecycleSettlement;
use crate::app::AppState;
use crate::lifecycle::{
    AdoptedImplementation, AdoptionPrepared, CreatedCheckout, DispatchReached,
    ImplementationPrepared, InitializedRepository, OpenedRepository, RemoteChanged,
    RestoredCheckout, StagePublications,
};
use crate::models::ModelChoice;
use crate::orchestrator::ActiveRun;
use crate::thread::ThreadDetail;
use serde_json::{json, Value};

pub struct CreateWorktreeSettlement {
    pub project_id: String,
    pub placeholder_id: String,
    pub checkouts: crate::lifecycle::holders::ProjectCheckouts,
}
impl LifecycleSettlement<CreatedCheckout> for CreateWorktreeSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<CreatedCheckout, String>,
    ) -> Result<Value, String> {
        let made = result?;
        let note = made
            .downgrade
            .as_deref()
            .map(crate::app::announce_isolation_downgrade);
        state.validate_checkout_snapshot(&self.project_id, &self.checkouts)?;
        Ok(
            json!({"project_id":self.project_id,"worktree_id":made.worktree_id,"pending_worktree_id":self.placeholder_id,"branch_was_cut":made.branch_was_cut,"branch":made.branch,"name":made.name,"path":made.path.display().to_string(),"isolation":made.isolation,"isolation_note":note}),
        )
    }
}

pub struct OpenImplementationSettlement {
    pub project_id: String,
    pub task_id: String,
    pub run_id: String,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn ImplementationCaller>,
}
impl LifecycleSettlement<ImplementationPrepared> for OpenImplementationSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<ImplementationPrepared, String>,
    ) -> Result<Value, String> {
        let prepared = match result {
            Ok(value) => value,
            Err(error) => return self.caller.settle(state, Err(error)),
        };
        let run_id = self.run_id.clone();
        match state.open_prepared_implementation(
            self.project_id,
            self.task_id,
            self.run_id,
            prepared,
            self.model_choice,
        ) {
            Ok(()) => self.caller.settle(state, Ok(&run_id)),
            Err(error) => self.caller.settle(state, Err(error)),
        }
    }
}

pub struct AdoptImplementationSettlement {
    pub project_id: String,
    pub task_id: String,
    pub run_id: String,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn ImplementationCaller>,
}
impl LifecycleSettlement<AdoptedImplementation> for AdoptImplementationSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<AdoptedImplementation, String>,
    ) -> Result<Value, String> {
        let prepared = match result {
            Ok(value) => value,
            Err(error) => return self.caller.settle(state, Err(error)),
        };
        let run_id = self.run_id.clone();
        match state.open_adopted_implementation(
            self.project_id,
            self.task_id,
            self.run_id,
            prepared,
            self.model_choice,
        ) {
            Ok(()) => self.caller.settle(state, Ok(&run_id)),
            Err(error) => self.caller.settle(state, Err(error)),
        }
    }
}

pub struct RestoreImplementationSettlement {
    pub task_id: String,
    pub run_id: String,
    pub caller: Box<dyn ImplementationCaller>,
}
impl LifecycleSettlement<RestoredCheckout> for RestoreImplementationSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<RestoredCheckout, String>,
    ) -> Result<Value, String> {
        match result {
            Ok(restored) => {
                state.settle_restored_checkout(self.task_id, self.run_id, restored, self.caller)
            }
            Err(error) => self.caller.settle(state, Err(error)),
        }
    }
}

pub struct PlanWorkspaceSettlement {
    pub opening: Box<dyn PlanSessionOpening>,
}
impl LifecycleSettlement<crate::orchestrator::PlanWorkspace> for PlanWorkspaceSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<crate::orchestrator::PlanWorkspace, String>,
    ) -> Result<Value, String> {
        match result {
            Ok(workspace) => self.opening.open(state, workspace),
            Err(error) => self.opening.refused(state, error),
        }
    }
}

pub struct AdoptionSettlement {
    pub detail: ThreadDetail,
}
impl LifecycleSettlement<AdoptionPrepared> for AdoptionSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<AdoptionPrepared, String>,
    ) -> Result<Value, String> {
        let adopted = result?;
        let run_id = adopted.run_id.clone();
        let active = state.open_adoption(&adopted)?;
        let (view, persisted) = state.answer_run_mutation(run_id, active, self.detail);
        persisted?;
        Ok(view)
    }
}

pub struct DispatchSettlement {
    pub project_id: String,
    pub instruction: String,
    pub model_choice: ModelChoice,
    pub explicit_choice: bool,
    pub routed: Option<crate::app::RoutedCapture>,
    pub checkouts: crate::lifecycle::holders::ProjectCheckouts,
}
impl LifecycleSettlement<DispatchReached> for DispatchSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<DispatchReached, String>,
    ) -> Result<Value, String> {
        let reached = result?;
        match reached {
            DispatchReached::Joined(joined) => {
                state.validate_checkout_snapshot(&self.project_id, &self.checkouts)?;
                #[cfg(test)]
                crate::lifecycle::fail_dispatch_at(
                    state.dispatch_fault,
                    crate::lifecycle::BranchDispatchStep::Post,
                )?;
                let choice = if self.explicit_choice {
                    self.model_choice
                } else {
                    state.entity_model_choice(&joined.run_id)?
                };
                state.join_dispatched_run(
                    self.project_id,
                    joined,
                    self.instruction,
                    choice,
                    self.routed,
                )
            }
            DispatchReached::Adopted(dispatched) => {
                state.open_dispatched_run(dispatched, self.instruction, self.routed, self.checkouts)
            }
        }
    }
}

pub struct AbandonSettlement {
    pub active: Box<ActiveRun>,
    pub run_id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub detail: ThreadDetail,
}
impl LifecycleSettlement<StagePublications> for AbandonSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<StagePublications, String>,
    ) -> Result<Value, String> {
        match result {
            Ok(published) => state.settle_abandoned_run(
                self.run_id,
                self.project_id,
                self.task_id,
                self.detail,
                published,
                *self.active,
            ),
            Err(error) => {
                state.runs.insert(self.run_id, *self.active);
                Err(error)
            }
        }
    }
}

pub struct ProjectRegistrationSettlement;
impl LifecycleSettlement<OpenedRepository> for ProjectRegistrationSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<OpenedRepository, String>,
    ) -> Result<Value, String> {
        state.register_opened_repository(result?)
    }
}
pub struct InitializeRepositorySettlement;
impl LifecycleSettlement<InitializedRepository> for InitializeRepositorySettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<InitializedRepository, String>,
    ) -> Result<Value, String> {
        let initialized = result?;
        if !state.projects.mark_git(&initialized.project_id) {
            return Err(format!("unknown project: {}", initialized.project_id));
        }
        state
            .board
            .diff_mut()
            .register_project(initialized.project_id.clone());
        state.persist();
        let project = state
            .projects
            .get(&initialized.project_id)
            .expect("project was just marked");
        Ok(state.project_json(project))
    }
}
pub struct SetRemoteSettlement {
    pub project_id: String,
}
impl LifecycleSettlement<RemoteChanged> for SetRemoteSettlement {
    fn settle(
        self,
        state: &mut AppState,
        result: Result<RemoteChanged, String>,
    ) -> Result<Value, String> {
        // The row reads the remote back from the checkout it was written to.
        result?;
        state.settle_remote_change(&self.project_id)
    }
}
