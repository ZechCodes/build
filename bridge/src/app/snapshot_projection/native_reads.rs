//! Compatibility reads use the same committed owner publication as the board.
use super::*;
use crate::app::read_view::SharedJson;
use crate::app::DigestScope;
use crate::thread::ThreadDetail;

impl AppState {
    pub(super) fn initialize_native_snapshots(&self) {
        let mut delta = SnapshotDelta::new(self.read_view.next_version());
        for (id, capture) in &self.captures {
            delta = delta.put(
                "capture.get",
                id,
                crate::app::captures::capture_json(capture),
            );
        }
        self.read_view.publish_delta(delta);
    }

    /// Captures persist independently of borrowed plan/run mutations.
    pub(in crate::app) fn publish_capture_snapshot(&self, id: &str) {
        if !self.snapshot_projection_initialized() {
            return;
        }
        let delta = SnapshotDelta::new(self.read_view.next_version());
        let delta = match self.captures.get(id) {
            Some(capture) => delta.put(
                "capture.get",
                id,
                crate::app::captures::capture_json(capture),
            ),
            None => delta.remove("capture.get", id),
        };
        self.read_view.publish_delta(delta);
    }

    pub(super) fn prepare_native_project_snapshots(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
    ) -> SnapshotDelta {
        for project in self.projects.iter() {
            if previous.get_shared("tasks.columns", &project.id).is_none() {
                delta = delta.put(
                    "tasks.columns",
                    &project.id,
                    crate::app::tracker::columns_json(&project.id),
                );
            }
        }
        for id in previous
            .global
            .as_deref()
            .into_iter()
            .flat_map(|cache| cache.project_ids())
        {
            if self.projects.get(id).is_none() {
                delta = delta.remove("tasks.columns", id);
            }
        }
        delta
    }

    pub(super) fn prepare_native_owner_snapshots(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owners: &BTreeSet<String>,
    ) -> SnapshotDelta {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.native_owners");
        for owner in owners {
            delta = self.prepare_implementation_binding(delta, previous, owner);
            delta = self.prepare_native_task_snapshot(delta, previous, owner);
        }
        delta
    }

    fn prepare_implementation_binding(
        &self,
        delta: SnapshotDelta,
        previous: &ReadView,
        owner: &str,
    ) -> SnapshotDelta {
        // Keep the old link so retirement or reassignment refreshes its task
        // without enumerating every retained implementation lineage.
        match self
            .runs
            .get(owner)
            .and_then(|run| run.run.plan_id.as_ref())
        {
            Some(plan) => put_changed(delta, previous, "task.implementation", owner, json!(plan.0)),
            None if previous.get_shared("task.implementation", owner).is_some() => {
                delta.remove("task.implementation", owner)
            }
            None => delta,
        }
    }

    fn prepare_native_task_snapshot(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owner: &str,
    ) -> SnapshotDelta {
        let mut retained = BTreeSet::new();
        if let Some(active) = self.plans.get(owner) {
            let base = self.plan_view(owner, active, ThreadDetail::Digest, DigestScope::Detail);
            let Value::Object(fields) = base else {
                unreachable!("task detail object")
            };
            let base = SharedJson::object(
                fields
                    .into_iter()
                    .map(|(key, value)| (key, SharedJson::value(value))),
            );
            delta = put_shared(delta, previous, "task.get", owner, base.clone());
            delta = put_changed(
                delta,
                previous,
                "task.stages",
                owner,
                self.task_stages(&json!({"task_id":owner}))
                    .expect("the task exists"),
            );
            for agent in active.agents.iter() {
                let key = format!("{owner}/{}", agent.id);
                retained.insert(agent.id.clone());
                delta =
                    self.prepare_native_task_agent(delta, previous, owner, &key, &agent.id, &base);
            }
        } else if previous.get_shared("task.get", owner).is_some() {
            delta = delta.remove("task.get", owner).remove("task.stages", owner);
        }
        // The old roster is a bounded owner index, not a scan of all chunks.
        for agent in previous_task_agents(previous, owner) {
            if !retained.contains(agent) {
                let key = format!("{owner}/{agent}");
                delta = delta.remove("task.get", &key).remove("task.get.error", key);
            }
        }
        delta
    }

    fn prepare_native_task_agent(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owner: &str,
        key: &str,
        agent: &str,
        base: &SharedJson,
    ) -> SnapshotDelta {
        match self.agent_conversation(owner, Some(agent)) {
            Ok(thread) => {
                let mut fields = base.fields().expect("task detail object").clone();
                fields.insert(
                    "thread".into(),
                    SharedJson::value(self.snapshot_thread_digest(thread)),
                );
                delta = put_shared(delta, previous, "task.get", key, SharedJson::object(fields));
                if previous.get_shared("task.get.error", key).is_some() {
                    delta = delta.remove("task.get.error", key);
                }
            }
            Err(error) => {
                delta = delta.remove("task.get", key);
                delta = put_changed(delta, previous, "task.get.error", key, json!(error));
            }
        }
        delta
    }
}

fn previous_task_agents<'a>(previous: &'a ReadView, owner: &str) -> impl Iterator<Item = &'a str> {
    previous
        .get_shared("task.get", owner)
        .and_then(SharedJson::fields)
        .and_then(|fields| fields.get("agents"))
        .and_then(SharedJson::leaf)
        .and_then(|agents| agents.as_array())
        .into_iter()
        .flatten()
        .filter_map(|agent| agent["id"].as_str())
}

fn put_shared(
    delta: SnapshotDelta,
    previous: &ReadView,
    method: &'static str,
    owner: &str,
    value: SharedJson,
) -> SnapshotDelta {
    if previous
        .get_shared(method, owner)
        .is_some_and(|old| old == &value)
    {
        delta
    } else {
        delta.put_shared(method, owner, value)
    }
}
