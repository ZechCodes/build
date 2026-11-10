//! Capture registry commits under app; enqueue and deliver through the off-lock flusher.
use super::AppState;
use std::sync::Arc;

impl AppState {
    pub(in crate::app) fn publish_secret_project_routes(&self, config: &serde_json::Value) {
        self.secret_membership_publication
            .borrow_mut()
            .capture(config);
        self.changes.wake_secret_publications();
    }

    pub(in crate::app) fn attach_secret_publication_source(state: &Arc<std::sync::Mutex<Self>>) {
        let app = state.lock().unwrap();
        let bus = Arc::clone(&app.changes);
        let writer = app.store.as_ref().map(|store| store.writer());
        if let Some(writer) = &writer {
            let weak = Arc::downgrade(&bus);
            writer.secret_read_retries.set_wake(Arc::new(move || {
                if let Some(bus) = weak.upgrade() {
                    bus.wake_secret_publications();
                }
            }));
        }
        drop(app);
        let weak = Arc::downgrade(state);
        let weak_bus = Arc::downgrade(&bus);
        bus.set_secret_publication_source(Arc::new(move || {
            let state = weak.upgrade()?;
            let writer = {
                let app = secret_publication_lock(&state, &weak_bus)?;
                if app.secret_membership_dirty.get() {
                    return None;
                }
                app.store.as_ref().map(|store| store.writer())
            };
            // No writer lifetime is retained by the bus; no SQL runs under app.
            if writer
                .as_ref()
                .is_some_and(|w| w.secret_membership_clean().ok() != Some(true))
            {
                return None;
            }
            let app = secret_publication_lock(&state, &weak_bus)?;
            app.take_secret_publication()
        }));
    }

    fn take_secret_publication(&self) -> Option<crate::changes::SecretPublication> {
        if self.secret_membership_dirty.get() {
            return None;
        }
        let mut batch =
            std::mem::take(&mut self.secret_membership_publication.borrow_mut().pending);
        if let Some(store) = &self.store {
            for scope in store.writer().secret_read_retries.take() {
                match scope {
                    Some(project) => {
                        batch.projects.insert(project);
                    }
                    None => batch.all = true,
                }
            }
        }
        Some(batch)
    }
}

/// Plain committed data only; recording it never enters the subscriptions mutex.
#[derive(Default)]
pub(in crate::app) struct SecretMembershipPublication {
    routes: std::collections::BTreeMap<String, String>,
    pending: crate::changes::SecretPublication,
}
impl SecretMembershipPublication {
    fn capture(&mut self, config: &serde_json::Value) {
        let routes: std::collections::BTreeMap<String, String> = config["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some((
                    p["id"].as_str()?.to_owned(),
                    p["environment_id"].as_str()?.to_owned(),
                ))
            })
            .collect();
        for project in self.routes.keys().chain(routes.keys()) {
            if self.routes.get(project) != routes.get(project) {
                self.pending.projects.insert(project.clone());
            }
        }
        self.pending.routes = Some(routes.iter().map(|(p, e)| (p.clone(), e.clone())).collect());
        self.routes = routes;
    }
}

fn secret_publication_lock<'a>(
    state: &'a std::sync::Mutex<AppState>,
    bus: &std::sync::Weak<crate::changes::ChangeBus>,
) -> Option<std::sync::MutexGuard<'a, AppState>> {
    match state.try_lock() {
        Ok(app) => Some(app),
        Err(_) => {
            if let Some(bus) = bus.upgrade() {
                bus.wake_secret_publications();
            }
            None
        }
    }
}
