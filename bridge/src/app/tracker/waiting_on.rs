//! Client-independent dependency reconciliation. Slow SQLite work stays off
//! tokio workers; each app-lock section processes at most one fan-out page.
use super::StoredAnswer;
use crate::app::{off_the_workers, AppState};
use crate::tracker::Task;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

impl AppState {
    #[cfg(test)]
    pub(in crate::app) fn pump_waiting_on_changes_for_test(
        state: &Arc<Mutex<Self>>,
    ) -> Result<bool, String> {
        reconcile_page(&Arc::downgrade(state)).ok_or("state disappeared".to_string())?
    }

    /// Graph facts and saved delivery state are read from persistence, including
    /// changes reconciled while no browser is connected.
    pub(super) fn extend_task_waiting_summary(
        &self,
        task: &Task,
        row: &mut Value,
    ) -> Result<(), String> {
        let store = self.tracker_store()?;
        let summary = store
            .waiting_on_summary(&task.project_path, &task.id)
            .stored()?;
        row["dependencies"] = serde_json::to_value(summary).map_err(|error| error.to_string())?;
        if let Some(intent) = store
            .current_delivery_intent(&task.id)
            .stored()?
            .filter(|intent| {
                matches!(
                    intent.state.as_str(),
                    "waiting" | "queued" | "provisioning" | "failed"
                )
            })
        {
            let state = match intent.state.as_str() {
                "provisioning" => "queued",
                state => state,
            };
            row["deferred_assignment"] =
                json!({"id":intent.id,"target":intent.target,"state":state,"error":intent.error});
        }
        self.extend_task_blocks_summary(task, row)
    }

    /// All task answers carry the same first bounded reverse page.
    pub(super) fn extend_task_blocks_summary(
        &self,
        task: &Task,
        row: &mut Value,
    ) -> Result<(), String> {
        let page = self
            .tracker_store()?
            .waiting_on_blocks(&task.project_path, &task.id, 20, None)
            .stored()?;
        row["dependencies"]["blocks"] = json!(page.blocks);
        if let Some(cursor) = page.next_cursor {
            row["dependencies"]["blocks_next_cursor"] = json!(cursor);
        }
        Ok(())
    }

    /// Pending assignments and reblocked delivered tasks are not work the
    /// holder can finish. Legacy assignments without intents remain actionable.
    pub(super) fn task_holder_has_actionable_work(&self, task: &Task) -> Result<bool, String> {
        let store = self.tracker_store()?;
        if store
            .waiting_on_summary(&task.project_path, &task.id)
            .stored()?
            .blocked
        {
            return Ok(false);
        }
        let Some(intent) = store.current_delivery_intent(&task.id).stored()? else {
            return Ok(true);
        };
        Ok(intent.state == "delivered" || self.operation_receipt(&intent.operation_id)?.is_some())
    }

    /// Quiet pending holders still receive comments written to their task.
    pub(super) fn pending_task_delivery_holder(
        &self,
        task: &Task,
    ) -> Result<Option<String>, String> {
        let Some(intent) = self
            .tracker_store()?
            .current_delivery_intent(&task.id)
            .stored()?
        else {
            return Ok(None);
        };
        if intent.state == "delivered" || self.operation_receipt(&intent.operation_id)?.is_some() {
            return Ok(None);
        }
        if let Some(agent) = intent.reservation["agent_id"].as_str().or_else(|| {
            task.assignee
                .as_ref()
                .and_then(crate::tracker::Assignee::agent_id)
        }) {
            return Ok(Some(agent.to_string()));
        }
        if intent.target["kind"] == "project_agent" {
            let project = self
                .projects
                .iter()
                .find(|project| project.repo_path.to_str() == Some(task.project_path.as_str()));
            return Ok(project
                .and_then(|project| self.project_conversation_run(&project.id))
                .and_then(|entity| self.project_pin(&entity))
                .map(str::to_string));
        }
        Ok(None)
    }

    pub(in crate::app) fn spawn_waiting_on_service(state: &Arc<Mutex<Self>>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Some(wakeup) = state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .map(|s| Arc::clone(&s.waiting_on_wakeup))
        else {
            return;
        };
        let weak = Arc::downgrade(state);
        runtime.spawn(async move {
            loop {
                let work = weak.clone();
                match off_the_workers(move || reconcile_page(&work)).await {
                    None => return,
                    Some(Ok(true)) => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Some(Ok(false)) => {}
                    Some(Err(error)) => eprintln!("reconcile task dependencies: {error}"),
                }
                tokio::select! {
                    _ = wakeup.notified() => {},
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                }
            }
        });
    }
}

fn reconcile_page(weak: &Weak<Mutex<AppState>>) -> Option<Result<bool, String>> {
    let state = weak.upgrade()?;
    let app = state.lock().unwrap();
    Some((|| {
        let store = app.tracker_store()?;
        let page = store.reconcile_waiting_on(100).map_err(|e| e.to_string())?;
        let changes = store
            .take_waiting_on_changes(100)
            .map_err(|e| e.to_string())?;
        for (path, task_id) in &changes {
            if let Some(project) = app
                .projects
                .iter()
                .find(|p| p.repo_path.to_str() == Some(path))
            {
                app.note_tasks_changed(&project.id, task_id);
            }
        }
        Ok(page.pending || changes.len() == 100)
    })())
}
