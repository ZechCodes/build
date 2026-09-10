use crate::lifecycle::{Performed, WorktreeChange, WorktreeMutation};
use crate::orchestrator::{Orchestrator, PlanWorkspace};
pub struct OpenPlanWorkspace {
    pub project: Orchestrator,
    pub plan_id: String,
    pub store: crate::store::Store,
}
impl WorktreeMutation for OpenPlanWorkspace {
    type Output = PlanWorkspace;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let workspace = self
            .project
            .prepare_plan_workspace(&self.plan_id, &self.store)
            .map_err(|error| error.to_string())?;
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: workspace,
        })
    }
}
