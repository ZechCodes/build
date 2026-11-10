//! Reserve configuration only for its synchronous atomic file write. SQL and
//! owned-file/history cleanup run in separate stages without that reservation.
use super::*;
use crate::app::git::deferred::DeferredGitWork;

struct RemoveRegistration {
    operation: DeletionOperation,
    deletion: ProjectDeletion,
    config: Value,
    path: Option<PathBuf>,
    writer: crate::app::config::files::ConfigFileWrite,
    failure: DeletionFailure,
    outcome: std::sync::Mutex<Option<Result<(), String>>>,
}
pub(super) fn queue_config_removal(
    app: &mut AppState,
    deletion: ProjectDeletion,
    operation: DeletionOperation,
) -> Result<(), String> {
    if app.deletion_config_write {
        return Err("configuration write in progress; retry project deletion".into());
    }
    let mut config = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    config["projects"]
        .as_array_mut()
        .expect("project array")
        .retain(|p| p["id"] != deletion.plan.project_id);
    let mut policies = app.compaction_policies.clone();
    policies.forget_project(&deletion.plan.project_id);
    policies.forget_workspaces(&deletion.plan.workspace_ids);
    policies.write_config(&mut config);
    let work = RemoveRegistration {
        operation,
        deletion,
        config,
        path: app.config_path.clone(),
        writer: app.config_file_writer(),
        failure: DeletionFailure::capture(app),
        outcome: std::sync::Mutex::new(None),
    };
    app.deletion_config_write = true;
    app.defer_project_deletion(work);
    Ok(())
}
impl DeferredGitWork for RemoveRegistration {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let result = self
            .failure
            .check("config")
            .and_then(|()| match &self.path {
                Some(path) => self.writer.write(path, &self.config),
                None => Ok(()),
            });
        *self.outcome.lock().unwrap() = Some(result);
        Ok(Value::Null)
    }
    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        self.operation.check(app)?;
        app.deletion_config_write = false;
        if let Err(error) = self
            .outcome
            .lock()
            .unwrap()
            .clone()
            .ok_or("missing config outcome")?
        {
            app.queue_deletion_reconciliation(
                app.store.clone().ok_or("missing deletion store")?,
                self.operation.clone(),
                Err(error),
            )?;
            return Ok(Value::Null);
        }
        app.apply_removed_registration(&self.deletion.plan);
        app.device_lifecycle_dirty.set(true);
        app.defer_project_deletion(super::execution::DeleteRecordedProject::capture(
            app,
            self.deletion.clone(),
            self.operation.clone(),
        )?);
        Ok(Value::Null)
    }
    fn invalidates_on_error(&self) -> bool {
        true
    }
    fn invalidate(&self, app: &mut AppState) {
        if self.operation.check(app).is_ok() {
            app.deletion_config_write = false;
            if app.deferred_work.is_none() {
                self.operation.finish(app);
            }
        }
    }
}
