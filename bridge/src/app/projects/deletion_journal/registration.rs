//! Reserve only the synchronous config commit, including S2.1 membership fencing
//! and projection. Owned-file/history cleanup runs outside that reservation.
use super::*;
use crate::app::git::deferred::DeferredGitWork;

struct RemoveRegistration {
    operation: DeletionOperation,
    deletion: ProjectDeletion,
    config: Value,
    before: Value,
    store: Store,
    path: Option<PathBuf>,
    writer: crate::app::config::files::ConfigFileWrite,
    failure: DeletionFailure,
    revision: crate::app::secret_storage::MembershipWrite,
    outcome: std::sync::Mutex<Option<RegistrationOutcome>>,
}
pub(super) fn queue_config_removal(
    app: &mut AppState,
    deletion: ProjectDeletion,
    operation: DeletionOperation,
) -> Result<(), String> {
    if app.deletion_config_write {
        return Err("configuration write in progress; retry project deletion".into());
    }
    let before = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    let mut config = before.clone();
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
        before,
        store: app.store.clone().ok_or("missing deletion store")?,
        path: app.config_path.clone(),
        writer: app.config_file_writer(),
        failure: DeletionFailure::capture(app),
        revision: app.secret_membership_revision.begin()?,
        outcome: std::sync::Mutex::new(None),
    };
    app.deletion_config_write = true;
    app.secret_membership_dirty.set(true);
    app.defer_project_deletion(work);
    Ok(())
}
impl DeferredGitWork for RemoveRegistration {
    fn run(&self, _: &Value) -> Result<Value, String> {
        *self.outcome.lock().unwrap() = Some(self.commit_config());
        Ok(Value::Null)
    }
    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        // This stage has finished its config IO even if its claim was lost.
        // Release the global reservation before refusing stale publication.
        app.deletion_config_write = false;
        self.operation.check(app)?;
        let outcome = self
            .outcome
            .lock()
            .unwrap()
            .clone()
            .ok_or("missing config outcome")?;
        if outcome.removed {
            app.apply_removed_registration(&self.deletion.plan);
            app.device_lifecycle_dirty.set(true);
        }
        app.secret_membership_dirty.set(!outcome.membership_clean);
        app.publish_secret_project_routes(if outcome.removed {
            &self.config
        } else {
            &self.before
        });
        self.revision.finish();
        if let Err(error) = outcome.result {
            app.queue_deletion_reconciliation(
                self.store.clone(),
                self.operation.clone(),
                Err(error),
            )?;
            return Ok(Value::Null);
        }
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
            self.revision.finish();
            app.deletion_config_write = false;
            if app.deferred_work.is_none() {
                self.operation.finish(app);
            }
        }
    }
}

#[derive(Clone)]
struct RegistrationOutcome {
    result: Result<(), String>,
    removed: bool,
    membership_clean: bool,
}
impl RemoveRegistration {
    fn commit_config(&self) -> RegistrationOutcome {
        let mut removed = false;
        let result = self.write_registration(&mut removed);
        // A reported file-write error leaves the old config authoritative.
        // Restore that projection within the same short reservation, never
        // from a stale snapshot after long cleanup or a sibling's commit.
        let membership_clean = if removed {
            result.is_ok()
        } else {
            self.store
                .writer()
                .secrets_reconcile_config(&self.before)
                .is_ok()
        };
        RegistrationOutcome {
            result,
            removed,
            membership_clean,
        }
    }
    fn write_registration(&self, removed: &mut bool) -> Result<(), String> {
        self.failure.check("config")?;
        self.store
            .writer()
            .mark_sync_dirty("secrets", "membership", crate::store::sync::now_ms())
            .map_err(|e| e.to_string())?;
        if let Some(path) = &self.path {
            self.writer.write(path, &self.config)?;
        }
        *removed = true;
        self.store
            .writer()
            .secrets_reconcile_config(&self.config)
            .map_err(|e| e.to_string())
    }
}
