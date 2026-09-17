use super::{
    AdoptImplementationSettlement, AdoptionSettlement, CreateWorktreeSettlement,
    DispatchSettlement, ImplementationCaller, LifecycleSettlement, OpenImplementationSettlement,
    PlanSessionOpening, PlanWorkspaceSettlement, ProjectRegistrationSettlement,
    RestoreImplementationSettlement, SetRemoteSettlement,
};
use crate::app::{AppState, RoutedCapture};
use crate::lifecycle::{
    AdoptedImplementation, AdoptionPrepared, CreatedCheckout, DispatchReached, DispatchedCheckout,
    JoinedCheckout, OpenedRepository, RemoteChanged,
};
use crate::models::ModelChoice;
use crate::orchestrator::{ActiveRun, AdoptableCheckout, PlanWorkspace, PreparedImplementation};
use crate::thread::ThreadDetail;
use crate::worktree::Worktree;
use serde_json::Value;
use std::path::PathBuf;

pub struct WorktreeCreated {
    pub project_id: String,
    pub placeholder_id: String,
    pub worktree_id: String,
    pub branch: String,
    pub name: String,
    pub path: PathBuf,
    pub branch_was_cut: bool,
    pub checkouts: crate::lifecycle::holders::ProjectCheckouts,
    pub isolation: Option<crate::isolation::Isolation>,
    pub downgrade: Option<String>,
}
impl WorktreeCreated {
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self {
            project_id,
            placeholder_id,
            worktree_id,
            branch,
            name,
            path,
            branch_was_cut,
            checkouts,
            isolation,
            downgrade,
        } = *self;
        CreateWorktreeSettlement {
            project_id,
            placeholder_id,
            checkouts,
        }
        .settle(
            state,
            Ok(CreatedCheckout {
                worktree_id,
                branch,
                name,
                path,
                branch_was_cut,
                isolation,
                downgrade,
            }),
        )
    }
}

pub struct ImplementationOpened {
    pub project_id: String,
    pub issue_id: String,
    pub run_id: String,
    pub prepared: PreparedImplementation,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn ImplementationCaller>,
    pub downgrade: Option<String>,
}
impl ImplementationOpened {
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self {
            project_id,
            issue_id,
            run_id,
            prepared,
            model_choice,
            caller,
            downgrade,
        } = *self;
        OpenImplementationSettlement {
            project_id,
            issue_id,
            run_id,
            model_choice,
            caller,
        }
        .settle(
            state,
            Ok(crate::lifecycle::ImplementationPrepared {
                prepared,
                downgrade,
            }),
        )
    }
}

pub struct RunAdopted {
    pub project_id: String,
    pub run_id: String,
    pub base_branch: String,
    pub checkout: AdoptableCheckout,
    pub model_choice: ModelChoice,
}
impl RunAdopted {
    fn into_prepared(self) -> AdoptionPrepared {
        AdoptionPrepared {
            project_id: self.project_id,
            run_id: self.run_id,
            base_branch: self.base_branch,
            checkout: self.checkout,
            model_choice: self.model_choice,
        }
    }
    fn prepared_snapshot(&self) -> AdoptionPrepared {
        AdoptionPrepared {
            project_id: self.project_id.clone(),
            run_id: self.run_id.clone(),
            base_branch: self.base_branch.clone(),
            checkout: self.checkout.clone(),
            model_choice: self.model_choice.clone(),
        }
    }
    pub fn open_run(&self, state: &mut AppState) -> Result<ActiveRun, String> {
        state.open_adoption(&self.prepared_snapshot())
    }
}

pub struct ImplementationAdopted {
    pub project_id: String,
    pub issue_id: String,
    pub run_id: String,
    pub base_sha: String,
    pub adopted: Option<RunAdopted>,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn ImplementationCaller>,
}
impl ImplementationAdopted {
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self {
            project_id,
            issue_id,
            run_id,
            base_sha,
            adopted,
            model_choice,
            caller,
        } = *self;
        AdoptImplementationSettlement {
            project_id,
            issue_id,
            run_id,
            model_choice,
            caller,
        }
        .settle(
            state,
            Ok(AdoptedImplementation {
                base_sha,
                adopted: adopted.map(RunAdopted::into_prepared),
            }),
        )
    }
}

pub struct ImplementationRefused {
    pub error: String,
    pub caller: Box<dyn ImplementationCaller>,
}
impl ImplementationRefused {
    #[allow(clippy::boxed_local)] // preserves the former object-safe epilogue call shape
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        self.caller.settle(state, Err(self.error))
    }
}

pub struct RunAdoptionSettled {
    pub adopted: RunAdopted,
    pub detail: ThreadDetail,
}
impl RunAdoptionSettled {
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        AdoptionSettlement {
            detail: self.detail,
        }
        .settle(state, Ok(self.adopted.into_prepared()))
    }
}

pub struct RestoredCheckout {
    pub issue_id: String,
    pub run_id: String,
    pub checkout_stood: bool,
    pub restored: Result<Worktree, String>,
    pub caller: Box<dyn ImplementationCaller>,
    pub downgrade: Option<String>,
}
impl RestoredCheckout {
    #[allow(clippy::boxed_local)] // preserves the former object-safe epilogue call shape
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self {
            issue_id,
            run_id,
            checkout_stood,
            restored,
            caller,
            downgrade,
        } = *self;
        RestoreImplementationSettlement {
            issue_id,
            run_id,
            caller,
        }
        .settle(
            state,
            Ok(crate::lifecycle::RestoredCheckout {
                restored,
                checkout_stood,
                downgrade,
            }),
        )
    }
}

pub struct BranchDispatched {
    pub adopted: RunAdopted,
    pub instruction: String,
    pub routed: Option<RoutedCapture>,
    pub checkouts: crate::lifecycle::holders::ProjectCheckouts,
    pub downgrade: Option<String>,
}
impl BranchDispatched {
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self {
            adopted,
            instruction,
            routed,
            checkouts,
            downgrade,
        } = *self;
        state.open_dispatched_run(
            DispatchedCheckout {
                adoption: adopted.into_prepared(),
                downgrade,
            },
            instruction,
            routed,
            checkouts,
        )
    }
}

pub struct BranchJoined {
    pub project_id: String,
    pub run_id: String,
    pub branch: String,
    pub root: PathBuf,
    pub instruction: String,
    pub model_choice: ModelChoice,
    pub explicit_choice: bool,
    pub routed: Option<RoutedCapture>,
    pub checkouts: crate::lifecycle::holders::ProjectCheckouts,
}
impl BranchJoined {
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self {
            project_id,
            run_id,
            branch,
            root,
            instruction,
            model_choice,
            explicit_choice,
            routed,
            checkouts,
        } = *self;
        DispatchSettlement {
            project_id,
            instruction,
            model_choice,
            explicit_choice,
            routed,
            checkouts,
        }
        .settle(
            state,
            Ok(DispatchReached::Joined(JoinedCheckout {
                run_id,
                branch,
                root,
            })),
        )
    }
}

pub struct PlanWorkspaceOpened {
    pub workspace: PlanWorkspace,
    pub opening: Box<dyn PlanSessionOpening>,
}
impl PlanWorkspaceOpened {
    #[allow(clippy::boxed_local)] // preserves the former object-safe epilogue call shape
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        PlanWorkspaceSettlement {
            opening: self.opening,
        }
        .settle(state, Ok(self.workspace))
    }
}
pub struct PlanWorkspaceRefused {
    pub error: String,
    pub opening: Box<dyn PlanSessionOpening>,
}
impl PlanWorkspaceRefused {
    #[allow(clippy::boxed_local)] // preserves the former object-safe epilogue call shape
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        PlanWorkspaceSettlement {
            opening: self.opening,
        }
        .settle(state, Err(self.error))
    }
}

pub struct ProjectAdded {
    pub path: PathBuf,
    pub base: String,
    pub remote: Option<String>,
    pub created_checkout: Option<PathBuf>,
}
impl ProjectAdded {
    #[allow(clippy::boxed_local)] // preserves the former object-safe epilogue call shape
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self {
            path,
            base,
            remote,
            created_checkout,
        } = *self;
        ProjectRegistrationSettlement.settle(
            state,
            Ok(OpenedRepository {
                path,
                base,
                remote,
                created_checkout,
                is_git: true,
            }),
        )
    }
}
pub struct ProjectRemoteSet {
    pub project_id: String,
    pub remote: Option<String>,
}
impl ProjectRemoteSet {
    #[allow(clippy::boxed_local)] // preserves the former object-safe epilogue call shape
    pub fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let Self { project_id, remote } = *self;
        SetRemoteSettlement { project_id }.settle(state, Ok(RemoteChanged { remote }))
    }
}

pub trait DiscardSettlement: Send {
    fn judge_before_removal(&mut self) {}
    fn settle(self: Box<Self>, state: &mut AppState, active: ActiveRun) -> Result<Value, String>;
}
