mod adoption;
mod discard;
mod dispatch;
pub mod holders;
mod outputs;
mod planning;
mod projects;
mod publication;
mod rows;
mod source_update;
mod task;

pub use adoption::{adopt, AdoptCheckout, AdoptionTarget};
pub use discard::{BeforeRemoval, DiscardCheckout, DiscardedCheckout, RemovedCheckout};
#[cfg(test)]
pub use dispatch::{fail_dispatch_at, BranchDispatchStep};
pub use dispatch::{DispatchCheckout, DispatchTarget};
pub use outputs::{
    AdoptedImplementation, AdoptionPrepared, CreatedCheckout, DispatchReached, DispatchedCheckout,
    ImplementationPrepared, InitializedRepository, JoinedCheckout, OpenedPlanWorkspace,
    OpenedRepository, RemoteChanged, RestoredCheckout,
};
pub use planning::OpenPlanWorkspace;
pub use projects::{CloneRepo, CreateRepo, InitializeRepo, OpenRepo, SetRemote};
pub(crate) use publication::classify_stage_publication;
pub use publication::{StagePublicationQuery, StagePublications};
pub use rows::{PendingRow, PendingState, WorktreeChange};
pub use source_update::{CheckoutFailed, SourceUpdated, UpdateSource, WorkspaceCheckout};
pub use task::{Performed, WorktreeMutation};
