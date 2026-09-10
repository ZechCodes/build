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
pub enum DiscardedCheckout {
    Removed {
        project: Orchestrator,
        worktree: Worktree,
    },
    Pruned {
        project: Orchestrator,
        worktree: Worktree,
    },
    Kept,
}
impl DiscardedCheckout {
    fn removal(&self) -> Option<(&Orchestrator, &Worktree, bool)> {
        match self {
            Self::Kept => None,
            Self::Removed { project, worktree } => Some((project, worktree, true)),
            Self::Pruned { project, worktree } => Some((project, worktree, false)),
        }
    }
    fn discard(&self, writers: &[Retirement], run_id: &str) -> WorktreeChange {
        let Some((project, worktree, keep_branch)) = self.removal() else {
            return WorktreeChange::nothing();
        };
        Retirement::wait_all(writers, crate::orchestrator::CHECKOUT_REAP_WAIT, run_id);
        project.discard_checkout(worktree, keep_branch);
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
