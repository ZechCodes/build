use crate::lifecycle::{Performed, WorktreeChange, WorktreeMutation};
use crate::orchestrator::Orchestrator;
use crate::reaper::Retirement;
use crate::worktree::Worktree;

pub trait BeforeRemoval: Send + 'static {
    type Output: Send + 'static;
    fn judge(self) -> Self::Output;
}
impl BeforeRemoval for () {
    type Output = ();
    fn judge(self) {}
}
/// What a discard does to the run's checkout: removes it, or keeps it.
pub enum DiscardedCheckout {
    Removed(Box<RemovedCheckout>),
    Kept,
}
/// The checkout a discard removes, and the project that removes it.
pub struct RemovedCheckout {
    pub project: Orchestrator,
    pub worktree: Worktree,
}
impl DiscardedCheckout {
    fn discard(&self, writers: &[Retirement], run_id: &str) -> WorktreeChange {
        let Self::Removed(removed) = self else {
            return WorktreeChange::nothing();
        };
        let RemovedCheckout { project, worktree } = removed.as_ref();
        Retirement::wait_all(writers, crate::orchestrator::CHECKOUT_REAP_WAIT, run_id);
        project.discard_checkout(worktree, /* keep_branch */ true);
        match worktree.path.exists() {
            false => WorktreeChange {
                gone: vec![crate::worktree::canonical_root(&worktree.path)],
                ..WorktreeChange::default()
            },
            true => WorktreeChange::undescribed(),
        }
    }
}
pub struct DiscardCheckout<J> {
    pub checkout: DiscardedCheckout,
    pub retirements: Vec<Retirement>,
    pub run_id: String,
    pub before_removal: J,
}
impl<J: BeforeRemoval> WorktreeMutation for DiscardCheckout<J> {
    type Output = J::Output;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let output = self.before_removal.judge();
        let change = self.checkout.discard(&self.retirements, &self.run_id);
        Ok(Performed { change, output })
    }
}
