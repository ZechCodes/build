//! Obligations skipped by archived boot, resumed independently after restore.
use super::run_recovery::{RunRecoveryEvidence, RunRecoveryWork};
use crate::app::{off_the_workers, AppState, OffLockJob, TabKey};
use crate::orchestrator::ActiveRun;
use crate::store::PersistedRun;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub(in crate::app) struct RestoredRecovery {
    pub deferred: HashSet<String>,
    pub in_flight: HashSet<String>,
    ready: HashSet<String>,
    pub provider_projects: HashSet<String>,
    pub provider_discovery_retry: bool,
    pub recipient_wakes: HashMap<String, crate::provider_recovery::RecoveryEpisode>,
    pub delivery_due: bool,
}

struct RestoreRun {
    expected: PersistedRun,
    work: RunRecoveryWork,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}
impl OffLockJob for RestoreRun {
    type Claim = String;
    type Decided = (PersistedRun, RunRecoveryEvidence);
    fn claim(&mut self) -> String {
        self.expected.id.clone()
    }
    fn decide(self) -> Self::Decided {
        #[cfg(test)]
        if let Some(gate) = self.gate {
            gate.arrive();
        }
        (self.expected, self.work.decide())
    }
    fn apply(app: &mut AppState, id: String, (expected, evidence): Self::Decided) {
        Self::abandon(app, id.clone());
        if app.refuse_archived_entity(&id).is_err() || app.run_recovery_held(&id) {
            return;
        }
        let Some(current) = app.recovery_run_record(&id) else {
            // Another lifecycle operation may have the run checked out.
            return;
        };
        if serde_json::to_value(&current).ok() != serde_json::to_value(&expected).ok() {
            return;
        }
        let plan_path = app.runs[&id].plan_path.clone();
        let active = ActiveRun::reattach(&expected, plan_path);
        match app.apply_recovered_run(expected, active, evidence) {
            Ok(()) => {
                app.restored_recovery.ready.remove(&id);
                if app.is_project_conversation_owner(&id) && !app.runs[&id].run.state.is_terminal()
                {
                    if let Err(error) = app.ensure_project_pin(&id, None) {
                        app.restored_recovery.ready.insert(id.clone());
                        eprintln!("restored project roster {id}: {error}");
                        return;
                    }
                }
                app.restored_recovery.deferred.remove(&id);
                app.release_restored_recipient_wakes();
                app.restored_recovery.delivery_due = true;
            }
            Err(error) => eprintln!("restored run recovery {id}: {error}"),
        }
    }
    fn abandon(app: &mut AppState, id: String) {
        if app.restored_recovery.in_flight.remove(&id) {
            app.active_deferred_filesystem_jobs =
                app.active_deferred_filesystem_jobs.saturating_sub(1);
        }
    }
}

impl AppState {
    pub(in crate::app) fn run_recovery_reserved(&self, id: &str) -> bool {
        self.restored_recovery.ready.contains(id)
    }

    pub(in crate::app) fn delivery_owner_held(&self, id: &str) -> bool {
        self.run_recovery_reserved(id) || self.refuse_archived_entity(id).is_err()
    }

    pub(in crate::app) fn refuse_run_recovery(&self, id: &str) -> Result<(), String> {
        if self.run_recovery_reserved(id) {
            return Err(crate::api::v1::reviews::errors::encode(
                "busy",
                "Run recovery is in progress; try again",
                serde_json::Value::Null,
            ));
        }
        Ok(())
    }

    pub(in crate::app) fn refuse_recovering_checkout(
        &self,
        project: &str,
        path: &std::path::Path,
    ) -> Result<(), String> {
        for id in &self.restored_recovery.ready {
            let Some(active) = self.runs.get(id) else {
                continue;
            };
            if self.projects.project_id_of(id) == Some(project)
                && crate::app::workspaces::same_path(path, &active.worktree.path)
            {
                self.refuse_run_recovery(id)?;
            }
        }
        Ok(())
    }

    fn recovery_run_record(&self, id: &str) -> Option<PersistedRun> {
        self.runs.get(id)?;
        self.committed_run_snapshot(id)
    }

    fn run_recovery_held(&self, id: &str) -> bool {
        let Some(active) = self.runs.get(id) else {
            return false;
        };
        let root = Self::canonical_root(&active.worktree.path);
        active.agents.iter().any(|agent| {
            let key = TabKey::agent(&root, &agent.id);
            self.session_registry.agent_snapshot(&key).is_some()
                || self.session_registry.claim_is_held(&key)
                || self.delivery_queue.agent_in_flight(&key)
        })
    }

    /// Only archived-boot obligations enter this set; ordinary projects retain
    /// their existing startup/sweep behavior. Claims exclude starts and archive.
    pub(in crate::app) fn schedule_restored_run_recovery(&mut self, project: Option<&str>) {
        if let Some(project) = project {
            self.activate_project_recovery(project);
        }
        let ids: Vec<_> = self
            .restored_recovery
            .deferred
            .iter()
            .filter(|id| {
                project.is_none_or(|project| self.projects.project_id_of(id) == Some(project))
            })
            .filter(|id| self.restored_recovery.ready.contains(*id))
            .filter(|id| {
                self.projects
                    .project_id_of(id)
                    .and_then(|project| self.projects.get(project))
                    .is_some()
            })
            .filter(|id| self.refuse_archived_entity(id).is_ok())
            .filter(|id| !self.restored_recovery.in_flight.contains(*id))
            .filter(|id| !self.run_recovery_held(id))
            .cloned()
            .collect();
        let capacity = 8usize.saturating_sub(self.restored_recovery.in_flight.len());
        for id in ids.into_iter().take(capacity) {
            let Some(record) = self.recovery_run_record(&id) else {
                continue;
            };
            let active = ActiveRun::reattach(&record, self.runs[&id].plan_path.clone());
            let work = self.capture_run_recovery(&record, &active);
            self.restored_recovery.in_flight.insert(id.clone());
            self.active_deferred_filesystem_jobs += 1;
            self.run_off_lock(RestoreRun {
                expected: record,
                work,
                #[cfg(test)]
                gate: self.off_lock_gate.clone(),
            });
        }
    }

    fn activate_project_recovery(&mut self, project: &str) {
        let Some(key) = self
            .projects
            .get(project)
            .map(|project| project.project_key.clone())
        else {
            return;
        };
        let ids: Vec<_> = self
            .restored_recovery
            .deferred
            .iter()
            .filter(|id| self.projects.project_key_for(id) == key)
            .cloned()
            .collect();
        for id in ids {
            self.projects.bind_entity(id.clone(), project.to_string());
            self.restored_recovery.ready.insert(id);
        }
    }

    pub(in crate::app) fn retry_restored_provider_notices(&mut self) {
        if self.restored_recovery.provider_discovery_retry {
            self.rebuild_provider_notice_retries();
        }
        let projects: Vec<_> = self
            .restored_recovery
            .provider_projects
            .iter()
            .cloned()
            .collect();
        for project in projects {
            self.replay_restored_provider_notices(&project);
        }
    }

    pub(in crate::app) fn replay_restored_provider_notices(&mut self, project: &str) {
        self.restored_recovery.delivery_due = true;
        match self.replay_provider_recovery_for_project(project) {
            Ok(true) => {
                self.restored_recovery.provider_projects.remove(project);
            }
            Ok(false) => {
                self.restored_recovery
                    .provider_projects
                    .insert(project.to_string());
            }
            Err(error) => {
                self.restored_recovery
                    .provider_projects
                    .insert(project.to_string());
                eprintln!("restored provider notices {project}: {error}");
            }
        }
    }

    pub(in crate::app) fn spawn_restored_recovery_service(state: &Arc<Mutex<Self>>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(state);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(state) = weak.upgrade() else {
                    return;
                };
                off_the_workers(move || {
                    let clock = {
                        let mut app = state.lock().unwrap();
                        app.schedule_restored_run_recovery(None);
                        app.retry_restored_provider_notices();
                        std::mem::take(&mut app.restored_recovery.delivery_due)
                            .then(|| Arc::clone(&app.frame_clock))
                    };
                    if let Some(clock) = clock {
                        crate::app::DeliveryRunner::drain(
                            &state,
                            &clock.frame("project.restore_delivery"),
                        );
                    }
                })
                .await;
            }
        });
    }
}
