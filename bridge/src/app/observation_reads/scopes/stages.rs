use super::*;
use serde_json::json;

impl ScopeReadContext {
    pub(super) fn prepare_stage_diff(&self, params: &Value) -> Result<ScopeReadJob, String> {
        let task = crate::app::require_str(params, "task_id")?;
        let stage_id = crate::app::require_str(params, "stage_id")?;
        let lineages = self
            .lineages
            .get(&task)
            .ok_or("task has no implementation lineage")?;
        let complete = |id: &&String| {
            self.runs[*id].stages.iter().any(|stage| {
                stage.stage_id == stage_id
                    && stage.start_sha.is_some()
                    && stage.completion_sha.is_some()
            })
        };
        let id = lineages
            .iter()
            .rev()
            .find(complete)
            .or_else(|| lineages.iter().rev().find(|id| !self.runs[*id].terminal))
            .or_else(|| lineages.last())
            .ok_or("task has no implementation lineage")?;
        let run = self.run(id)?;
        let stage = run
            .stages
            .iter()
            .find(|stage| stage.stage_id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))?
            .clone();
        let mut fence = ScopeReadFence::new("stage boundary changed while reading its diff");
        fence.bindings.push(self.run_binding(id)?);
        fence.bindings.push(Binding::Stage {
            run: id.clone(),
            stage: stage.clone(),
        });
        let id = id.clone();
        let checkout = run.checkout.clone();
        let project_path = run.project_path.clone();
        Ok(ScopeReadJob::new(fence, move || {
            let (Some(start_sha), Some(completion_sha)) = (&stage.start_sha, &stage.completion_sha)
            else {
                return Ok(json!({"run_id": id, "stage_id": stage_id, "task_id": task,
                    "status": "unavailable", "reason": if stage.publication == crate::run::StagePublication::LegacyUnknown { "legacy_unpinned" } else { "stage_not_complete" },
                    "start_sha": stage.start_sha, "completion_sha": stage.completion_sha}));
            };
            let read = crate::app::runtime::deferred::DeferredRead {
                subject: ReadSubject::Stage {
                    run_id: id,
                    stage_id,
                    object_database: if checkout.exists() {
                        checkout
                    } else {
                        project_path
                    },
                    start_sha: start_sha.clone(),
                    completion_sha: completion_sha.clone(),
                },
                task_id: Some(task),
                if_diff_key: None,
                with_patch: true,
                paths: None,
                range: None,
                #[cfg(test)]
                gate: None,
            };
            read.run()
        }))
    }
}
