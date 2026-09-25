//! The narrow application adapter for bridge updates. Network work and the
//! independent helper are owned by `UpdateService`, never by the app mutex.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::{off_the_workers, AppState};
use crate::carrier::SessionSender;
use crate::update::{AdmissionGate, UpdateService, UpdateStatus};

impl AppState {
    pub fn with_update_service(mut self, updates: Arc<UpdateService>) -> Self {
        self.updates = Some(updates);
        self
    }

    pub fn update_service(&self) -> Option<Arc<UpdateService>> {
        self.updates.clone()
    }

    /// Tick the update service every `interval`, telling it whether an agent
    /// is mid-turn.
    pub fn spawn_update_checks(app: Arc<Mutex<AppState>>, interval: Duration) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);
            loop {
                interval.tick().await;
                let checking = Arc::clone(&app);
                let (service, working_agents) = off_the_workers(move || {
                    let app = checking.lock().unwrap();
                    (app.update_service(), app.update_has_working_agents())
                })
                .await;
                if let Some(service) = service {
                    if let Err(error) = service.tick(working_agents).await {
                        eprintln!("bridge update tick: {error}");
                    }
                }
            }
        });
    }

    pub(in crate::app) fn update_admission(&self) -> Option<Arc<AdmissionGate>> {
        self.updates.as_ref().map(|service| service.admission())
    }

    /// A scheduled install waits for every kind of active agent turn, including
    /// a turn waiting in the dispatch queue and a router that has no tab yet.
    /// This is a snapshot, taken afresh by the update service's periodic tick;
    /// new work remains allowed while an update is scheduled.
    pub fn update_has_working_agents(&self) -> bool {
        if !self.delivery_queue.is_idle()
            || !self.router_sessions.is_empty()
            || self
                .session_registry
                .agent_working_roots()
                .iter()
                .any(|(_, working)| *working)
        {
            return true;
        }
        self.plans.values().any(|plan| {
            plan.agents
                .iter()
                .any(|agent| agent.working_since.is_some())
        }) || self
            .runs
            .values()
            .any(|run| run.agents.iter().any(|agent| agent.working_since.is_some()))
    }

    pub(in crate::app) fn subscribe_update_status(&mut self, sender: &SessionSender) {
        let Some(service) = self.update_service() else {
            return;
        };
        if let Some(previous) = self.update_subscriptions.remove(sender.session_id()) {
            previous.abort();
        }
        let mut updates = service.subscribe();
        let session_id = sender.session_id().to_string();
        let sender = sender.clone();
        let task = tokio::spawn(async move {
            let first = updates.borrow_and_update().clone();
            if !sender.push(status_event(&first)) {
                return;
            }
            while updates.changed().await.is_ok() {
                let next = updates.borrow_and_update().clone();
                if !sender.push(status_event(&next)) {
                    break;
                }
            }
        });
        self.update_subscriptions.insert(session_id, task);
    }

    pub(in crate::app) fn unsubscribe_update_status(&mut self, session_id: &str) {
        if let Some(task) = self.update_subscriptions.remove(session_id) {
            task.abort();
        }
    }
}

pub(super) fn status_event(status: &UpdateStatus) -> Value {
    let mut event = serde_json::to_value(status).expect("update status serializes");
    event
        .as_object_mut()
        .expect("update status is an object")
        .insert("type".into(), json!("bridge.update_status"));
    event
}
