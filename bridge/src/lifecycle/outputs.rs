use crate::isolation::Isolation;
use crate::orchestrator::{
    AdoptableCheckout, AdoptionScope, PlanWorkspace, PreparedImplementation,
};
use crate::worktree::Worktree;
use std::path::PathBuf;

pub struct CreatedCheckout {
    pub worktree_id: String,
    pub branch: String,
    pub name: String,
    pub path: PathBuf,
    pub branch_was_cut: bool,
    pub isolation: Option<Isolation>,
    pub downgrade: Option<String>,
}
pub struct ImplementationPrepared {
    pub prepared: PreparedImplementation,
    pub downgrade: Option<String>,
}
pub struct AdoptionPrepared {
    pub project_id: String,
    pub run_id: String,
    pub base_branch: String,
    pub checkout: AdoptableCheckout,
    pub scope: AdoptionScope,
    pub model_choice: crate::models::ModelChoice,
}
pub struct AdoptedImplementation {
    pub base_sha: String,
    pub adopted: Option<AdoptionPrepared>,
}
pub struct RestoredCheckout {
    pub restored: Result<Worktree, String>,
    pub checkout_stood: bool,
    pub downgrade: Option<String>,
}
pub enum DispatchReached {
    Joined(JoinedCheckout),
    Adopted(DispatchedCheckout),
}
pub struct JoinedCheckout {
    pub run_id: String,
    pub branch: String,
    pub root: PathBuf,
}
pub struct DispatchedCheckout {
    pub adoption: AdoptionPrepared,
    pub downgrade: Option<String>,
}
pub struct OpenedRepository {
    pub path: PathBuf,
    pub base: String,
    pub remote: Option<String>,
    pub created_checkout: Option<PathBuf>,
}
pub struct RemoteChanged {
    pub remote: Option<String>,
}
pub type OpenedPlanWorkspace = PlanWorkspace;
