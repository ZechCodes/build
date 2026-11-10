//! Inventory and record admission on the deferred blocking lane.
use super::admission::CapturedDeletion;
use super::*;
use crate::app::git::deferred::DeferredGitWork;
use std::sync::Mutex;

enum Preparation {
    New(CapturedDeletion),
    Resume(ProjectDeletion),
}
pub(super) struct PrepareDeletion {
    preparation: Preparation,
    store: Store,
    operation: DeletionOperation,
    sources: Vec<PathBuf>,
    records: Mutex<Option<Vec<ProjectDeletion>>>,
    prior: Vec<ProjectDeletion>,
    failure: DeletionFailure,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}
impl PrepareDeletion {
    pub(super) fn new(
        app: &AppState,
        captured: CapturedDeletion,
        operation: DeletionOperation,
    ) -> Result<Self, String> {
        let sources = captured.sources.clone();
        Self::capture(app, Preparation::New(captured), operation, sources)
    }
    pub(super) fn resume(
        app: &AppState,
        deletion: ProjectDeletion,
        operation: DeletionOperation,
    ) -> Result<Self, String> {
        Self::capture(
            app,
            Preparation::Resume(deletion),
            operation,
            app.deletion_sources(),
        )
    }
    fn capture(
        app: &AppState,
        preparation: Preparation,
        operation: DeletionOperation,
        sources: Vec<PathBuf>,
    ) -> Result<Self, String> {
        Ok(Self {
            preparation,
            store: app.store.clone().ok_or("missing deletion store")?,
            operation,
            sources,
            records: Mutex::new(None),
            prior: app.project_deletion_cache.clone(),
            failure: DeletionFailure::capture(app),
            #[cfg(test)]
            gate: app.off_lock_gate.clone(),
        })
    }
    fn prepare(&self) -> Result<ProjectDeletion, String> {
        match &self.preparation {
            Preparation::New(captured) => {
                #[cfg(test)]
                if let Some(gate) = &self.gate {
                    gate.arrive();
                }
                if self
                    .store
                    .writer()
                    .pending_creations()
                    .map_err(|e| e.to_string())?
                    .iter()
                    .any(|record| {
                        record.execution["project_key"].as_str() == Some(&captured.plan.project_key)
                    })
                {
                    return Err(crate::reclaim::BUSY.into());
                }
                self.failure.check("record")?;
                let plan = complete_capture(captured)?;
                self.store
                    .writer()
                    .begin_project_deletion(plan)
                    .map_err(|e| e.to_string())
            }
            Preparation::Resume(deletion) => {
                validate_plan(&self.store, deletion, &self.sources)?;
                Ok(deletion.clone())
            }
        }
    }
}
impl DeferredGitWork for PrepareDeletion {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let result = self.prepare();
        let mut records = self
            .store
            .writer()
            .project_deletions()
            .unwrap_or_else(|_| self.prior.clone());
        if let Ok(deletion) = &result {
            records.retain(|record| record.command_id != deletion.command_id);
            records.push(deletion.clone());
        }
        *self.records.lock().unwrap() = Some(records);
        result.map(|deletion| json!(deletion))
    }
    fn settle(&self, app: &mut AppState, value: Value) -> Result<Value, String> {
        self.operation.check(app)?;
        if let Some(records) = self.records.lock().unwrap().clone() {
            app.publish_deletion_records(&self.operation.id, records);
        }
        let deletion: ProjectDeletion = serde_json::from_value(value).map_err(|e| e.to_string())?;
        app.queue_deletion_retirement(deletion, self.operation.clone())?;
        Ok(Value::Null)
    }
    fn invalidates_on_error(&self) -> bool {
        true
    }
    fn invalidate(&self, app: &mut AppState) {
        if self.operation.check(app).is_err() {
            return;
        }
        if let Some(records) = self.records.lock().unwrap().clone() {
            app.publish_deletion_records(&self.operation.id, records);
        }
        if app.deferred_work.is_none() {
            self.operation.finish(app);
        }
    }
}
fn complete_capture(captured: &CapturedDeletion) -> Result<ProjectDeletionPlan, String> {
    let mut plan = captured.plan.clone();
    plan.roots = plan
        .roots
        .iter()
        .map(|p| AppState::canonical_root(p))
        .collect();
    validate_paths(&plan.roots, &captured.sources, &captured.workspaces)?;
    for (source, path, registry_root) in &captured.candidates {
        let Some(isolation) = crate::isolation::Isolation::of(path) else {
            continue;
        };
        plan.checkouts.push(crate::store::ProjectDeletionCheckout {
            source: source.clone(),
            path: path.clone(),
            registry_root: registry_root.clone(),
            registered_name: crate::isolation::checkout_name(path)
                .ok_or("checkout has no registration name")?,
            isolation,
        });
    }
    plan.lock_markers = plan
        .lock_markers
        .iter()
        .map(|path| {
            let parent = path.parent().ok_or("invalid workspace marker parent")?;
            let name = path.file_name().ok_or("invalid workspace marker name")?;
            Ok(crate::worktree::canonical_planned_path(parent).join(name))
        })
        .collect::<Result<Vec<_>, String>>()?;
    plan.inventory = capture_plan_inventory(&plan)?;
    Ok(plan)
}
fn validate_paths(
    roots: &[PathBuf],
    sources: &[PathBuf],
    workspaces: &[crate::workspace::Workspace],
) -> Result<(), String> {
    let sources: Vec<_> = sources
        .iter()
        .map(|p| AppState::canonical_root(p))
        .collect();
    if roots
        .iter()
        .any(|root| sources.iter().any(|source| source.starts_with(root)))
    {
        return Err("Cannot delete a workspace containing a source repository".into());
    }
    for w in workspaces.iter().filter(|w| w.managed) {
        let root = AppState::canonical_root(&w.root);
        if w.directories
            .iter()
            .any(|d| !AppState::canonical_root(&d.path).starts_with(&root))
        {
            return Err(
                "Cannot delete a workspace whose source checkout resolves outside its managed root"
                    .into(),
            );
        }
    }
    Ok(())
}
