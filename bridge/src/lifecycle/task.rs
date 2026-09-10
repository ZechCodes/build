use crate::lifecycle::WorktreeChange;

pub trait WorktreeMutation: Send + 'static {
    type Output: Send + 'static;
    fn perform(self) -> Result<Performed<Self::Output>, String>;
}

pub struct Performed<O> {
    pub change: WorktreeChange,
    pub output: O,
}
