mod app_api;
mod compat;
mod continuations;
mod runner;
mod settlements;

pub use compat::{
    BranchDispatched, BranchJoined, DiscardSettlement, ImplementationAdopted, ImplementationOpened,
    ImplementationRefused, PlanWorkspaceOpened, PlanWorkspaceRefused, ProjectAdded,
    ProjectRemoteSet, RestoredCheckout, RunAdopted, RunAdoptionSettled, WorktreeCreated,
};
pub use continuations::{ImplementationCaller, PlanSessionOpening};
pub(in crate::app) use runner::WorktreeLifecycleJob;
pub(in crate::app) use runner::{LifecycleOutcome, LifecycleSettlement};
pub(in crate::app) use settlements::{
    AbandonSettlement, AdoptImplementationSettlement, AdoptionSettlement, CreateWorktreeSettlement,
    DeleteSettlement, DispatchSettlement, InitializeRepositorySettlement,
    OpenImplementationSettlement, PlanWorkspaceSettlement, ProjectRegistrationSettlement,
    RestoreImplementationSettlement, SetRemoteSettlement,
};
