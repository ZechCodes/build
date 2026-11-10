use super::*;
use crate::api::ApiError;

impl AppState {
    pub(in crate::app) fn refuse_deleting_identity(
        &self,
        id: &str,
        key: &str,
    ) -> Result<(), String> {
        if let Some(plan) = self
            .project_deletion_fences
            .iter()
            .find(|p| p.project_id == id || p.project_key == key)
        {
            return Err(format!(
                "Project {} is deletion-pending; retry project.delete with confirm:true{}",
                plan.project_id,
                if plan.delete_tasks {
                    ", tasks:delete"
                } else {
                    ""
                }
            ));
        }
        Ok(())
    }

    pub(in crate::app) fn deletion_pending_addresses(&self) -> std::collections::HashSet<String> {
        self.project_deletion_fences
            .iter()
            .flat_map(|p| {
                std::iter::once(p.project_id.clone())
                    .chain(p.run_ids.iter().cloned())
                    .chain(p.plan_ids.iter().cloned())
                    .chain(p.workspace_ids.iter().cloned())
            })
            .collect()
    }

    pub(in crate::app) fn guard_deleting_rpc(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<(), ApiError> {
        if method == "project.delete" {
            return Ok(());
        }
        for field in [
            "project_id",
            "workspace_id",
            "entity_id",
            "run_id",
            "plan_id",
            "id",
        ] {
            if let Some(id) = params[field].as_str() {
                self.refuse_deleting_address(id)
                    .map_err(ApiError::classify)?;
            }
        }
        if let Some(task) = params["task_id"].as_str() {
            if let Some(store) = &self.store {
                if let Some((key, _)) = store
                    .task_project_authority(task)
                    .map_err(|e| ApiError::unavailable(e.to_string()))?
                {
                    self.refuse_deleting_identity("", &key)
                        .map_err(ApiError::classify)?;
                }
            }
        }
        Ok(())
    }

    pub(in crate::app) fn refuse_deleting_address(&self, id: &str) -> Result<(), String> {
        for plan in &self.project_deletion_fences {
            if plan.project_id == id
                || plan.workspace_ids.iter().any(|x| x == id)
                || plan.run_ids.iter().any(|x| x == id)
                || plan.plan_ids.iter().any(|x| x == id)
            {
                return self.refuse_deleting_identity(&plan.project_id, &plan.project_key);
            }
        }
        Ok(())
    }
}
