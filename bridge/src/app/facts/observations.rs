//! A bounded coalescing producer for checkout observations. Readers enqueue
//! immutable scope requests; every filesystem/Git probe runs on this worker.

use super::snapshot::PushFacts;
use super::{read_git, run_stat_of, AppState, CheckoutSubject};
use crate::app::read_view::{ReadViewSlot, SnapshotDelta};
use crate::changes::{BoardLists, EntityFacts, Kind};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

const PENDING_OBSERVATIONS: usize = 512;

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct CheckoutObservation {
    pub(super) facts: EntityFacts,
    stat: Option<Value>,
    summary: Option<(String, Vec<PathBuf>, Value)>,
}

#[derive(Clone)]
pub(super) struct ObservationRequest {
    entity: String,
    subject: Arc<CheckoutSubject>,
    generation: Arc<()>,
    summary: Option<(String, Vec<PathBuf>)>,
    git: bool,
    files: bool,
}

impl ObservationRequest {
    pub(super) fn new(
        entity: &str,
        subject: &Arc<CheckoutSubject>,
        facts: &PushFacts,
        git: bool,
        files: bool,
    ) -> Self {
        Self {
            entity: entity.into(),
            subject: Arc::clone(subject),
            generation: facts.observation_generation(entity),
            summary: facts.summaries.get(entity).cloned(),
            git,
            files,
        }
    }

    fn merge(&mut self, newer: Self) {
        if Arc::ptr_eq(&self.subject, &newer.subject)
            && Arc::ptr_eq(&self.generation, &newer.generation)
        {
            self.git |= newer.git;
            self.files |= newer.files;
        } else {
            *self = newer;
        }
    }

    fn is_current(&self, facts: &PushFacts) -> bool {
        facts.same_subject(&self.entity, &self.subject)
            && facts.same_observation_generation(&self.entity, &self.generation)
    }

    fn probe(self, facts: &PushFacts) -> Option<ProbeResult> {
        if !self.is_current(facts) {
            return None;
        }
        let mut observation = facts
            .observed
            .get(&self.entity)
            .map(|value| value.as_ref().clone())
            .unwrap_or_default();
        observation.facts.entity_id = self.entity.clone();
        if self.files {
            observation.facts.root_listing =
                super::super::fs::directory_listing(&self.subject.root, "").ok();
        }
        if let (true, Some(diff)) = (self.git, &self.subject.diff) {
            read_git(&self.subject, diff, &mut observation.facts);
            observation.stat = run_stat_of(&self.subject);
            observation.summary = self.summary.as_ref().map(|(workspace, repositories)| {
                (
                    workspace.clone(),
                    repositories.clone(),
                    super::super::board::cache::workspace_work_summary(repositories),
                )
            });
        }
        Some(ProbeResult {
            request: self,
            observation,
        })
    }
}

struct ProbeResult {
    request: ObservationRequest,
    observation: CheckoutObservation,
}

#[derive(Default)]
struct Pending {
    items: BTreeMap<String, ObservationRequest>,
    refresh_all: bool,
    running: bool,
}

pub(super) struct ObservationSink {
    view: ReadViewSlot,
    state: Weak<Mutex<AppState>>,
    pending: Mutex<Pending>,
}

impl ObservationSink {
    pub(super) fn new(view: ReadViewSlot, state: Weak<Mutex<AppState>>) -> Self {
        Self {
            view,
            state,
            pending: Mutex::default(),
        }
    }

    pub(super) fn submit(self: &Arc<Self>, requests: Vec<ObservationRequest>) {
        if requests.is_empty() {
            return;
        }
        let mut pending = self.pending.lock().unwrap();
        for request in requests {
            if pending.items.len() == PENDING_OBSERVATIONS
                && !pending.items.contains_key(&request.entity)
            {
                // Overflow requests one later full observation pass; it cannot
                // lose an invalidation or grow an unbounded queue of values.
                pending.refresh_all = true;
                pending.items.clear();
            }
            match pending.items.get_mut(&request.entity) {
                Some(held) => held.merge(request),
                None => {
                    pending.items.insert(request.entity.clone(), request);
                }
            }
        }
        if pending.running {
            return;
        }
        pending.running = true;
        drop(pending);
        let worker = Arc::clone(self);
        if std::thread::Builder::new()
            .name("bridge-facts".into())
            .spawn(move || worker.run())
            .is_err()
        {
            self.pending.lock().unwrap().running = false;
        }
    }

    fn take(&self) -> Option<(Vec<ObservationRequest>, bool)> {
        let mut pending = self.pending.lock().unwrap();
        if pending.items.is_empty() && !pending.refresh_all {
            pending.running = false;
            return None;
        }
        let all = std::mem::take(&mut pending.refresh_all);
        Some((
            std::mem::take(&mut pending.items).into_values().collect(),
            all,
        ))
    }

    fn run(&self) {
        while let Some((requests, refresh_all)) = self.take() {
            self.probe_and_apply(requests);
            if refresh_all {
                self.refresh_all();
            }
        }
    }

    fn probe_and_apply(&self, requests: Vec<ObservationRequest>) {
        let observations = requests
            .into_iter()
            .filter_map(|request| request.probe(&self.view.load().facts))
            .collect();
        self.apply(observations);
    }

    fn refresh_all(&self) {
        let snapshot = self.view.load();
        let mut requests = Vec::new();
        for (entity, subject) in &snapshot.facts.subjects {
            requests.push(ObservationRequest::new(
                entity,
                subject,
                &snapshot.facts,
                true,
                true,
            ));
            if requests.len() == PENDING_OBSERVATIONS {
                self.probe_and_apply(std::mem::take(&mut requests));
            }
        }
        self.probe_and_apply(requests);
    }

    fn apply(&self, observations: Vec<ProbeResult>) {
        if observations.is_empty() {
            return;
        }
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut app = state.lock().unwrap();
        let current = self.view.load();
        let changed = observations
            .into_iter()
            .filter(|result| {
                result.request.is_current(&current.facts)
                    && current
                        .facts
                        .observed
                        .get(&result.request.entity)
                        .map(Arc::as_ref)
                        != Some(&result.observation)
            })
            .collect::<Vec<_>>();
        if changed.is_empty() {
            return;
        }
        let memberships = app.workspace_summary_memberships();
        app.sync_workspace_summaries(&memberships);
        let mut summaries = false;
        for result in &changed {
            summaries |= apply_cache_observation(&mut app, result, &current.facts, &memberships);
        }
        app.publish_snapshot_projection();
        self.publish_observations(&changed);
        for result in changed {
            let request = result.request;
            if request.git {
                app.changes.note_kind(&request.entity, Kind::Git);
            }
            if request.files {
                app.changes.note_kind(&request.entity, Kind::Files);
            }
        }
        if summaries {
            app.changes.note_board_lists(BoardLists::WORKSPACES);
        }
    }

    fn publish_observations(&self, changed: &[ProbeResult]) {
        self.view.publish_current(|current, version| {
            let mut facts = (*current.facts).clone();
            let mut published = false;
            for result in changed
                .iter()
                .filter(|result| result.request.is_current(&current.facts))
            {
                facts.observed.insert(
                    result.request.entity.clone(),
                    Arc::new(result.observation.clone()),
                );
                published = true;
            }
            published.then(|| SnapshotDelta::new(version).set_facts(Arc::new(facts)))
        });
    }
}

fn apply_cache_observation(
    app: &mut AppState,
    result: &ProbeResult,
    previous: &PushFacts,
    memberships: &[(String, Vec<PathBuf>)],
) -> bool {
    if let (true, Some(status)) = (result.request.git, &result.observation.facts.status) {
        app.record_git_snapshot_branches(status);
    }
    let old = previous.observed.get(&result.request.entity);
    if let Some(stat) = result
        .observation
        .stat
        .as_ref()
        .filter(|stat| old.and_then(|old| old.stat.as_ref()) != Some(*stat))
    {
        app.store_run_stat(
            result.request.entity.clone(),
            stat.clone(),
            std::time::Instant::now(),
        );
    }
    let Some((workspace_id, repositories, summary)) = result
        .observation
        .summary
        .as_ref()
        .filter(|summary| old.and_then(|old| old.summary.as_ref()) != Some(*summary))
    else {
        return false;
    };
    if !memberships
        .iter()
        .any(|(id, roots)| id == workspace_id && roots == repositories)
    {
        return false;
    }
    app.store_diff_entry(
        super::super::board::cache::DiffCacheEntry::WorkspaceSummary {
            workspace_id: workspace_id.clone(),
            repositories: repositories.clone(),
            summary: summary.clone(),
        },
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::facts::snapshot_tests::test_app;
    use crate::carrier::{FrameHandler, SessionSender};
    use crate::git_fixture::{git_in, init_repo_named};
    use serde_json::json;

    fn call(handler: &FrameHandler, method: &str, params: Value) -> Value {
        let reply = handler.call(
            SessionSender::detached("facts-stale"),
            crate::transport::Frame {
                session_id: "facts-stale".into(),
                message_id: "facts-stale".into(),
                frame_type: "data".into(),
                sender: "client".into(),
                created_at: "now".into(),
                payload: json!({"id":"facts-stale", "method":method, "params":params}),
            },
        );
        assert_eq!(reply["ok"], true, "{reply:?}");
        reply["result"].clone()
    }

    fn paused_observation(
        state: &Arc<Mutex<AppState>>,
        entity: &str,
    ) -> (ObservationSink, ProbeResult) {
        let view = state.lock().unwrap().read_view.clone();
        let snapshot = view.load();
        let subject = snapshot.facts.subjects.get(entity).unwrap();
        let request = ObservationRequest::new(entity, subject, &snapshot.facts, true, true);
        let result = request.probe(&snapshot.facts).unwrap();
        (ObservationSink::new(view, Arc::downgrade(state)), result)
    }

    #[test]
    fn a_completed_same_branch_commit_rejects_an_older_probe() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo_named(dir.path(), "repo");
        let mut app = test_app(dir.path());
        let project = app.add_project(repo.clone(), "main".into());
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        let (sink, old) = paused_observation(&state, &project);
        let old_log = old.observation.facts.log.clone();
        std::fs::write(repo.join("later.txt"), "changed").unwrap();
        git_in(&repo, &["add", "later.txt"]);
        call(
            &handler,
            "git.commit",
            json!({"project_id":project,"message":"later"}),
        );
        sink.apply(vec![old]);
        let current = sink.view.load();
        assert!(
            current
                .facts
                .observed
                .get(&project)
                .is_none_or(|observed| observed.facts.log != old_log),
            "a pre-commit probe replaced the committed checkout observation"
        );
    }

    #[test]
    fn a_completed_file_mutation_rejects_an_older_root_listing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = init_repo_named(dir.path(), "repo");
        let mut app = test_app(dir.path());
        let project = app.add_project(repo, "main".into());
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        let (sink, old) = paused_observation(&state, &project);
        let old_listing = old.observation.facts.root_listing.clone();
        call(
            &handler,
            "fs.createFile",
            json!({"project_id":project,"parent":"","name":"later.txt"}),
        );
        sink.apply(vec![old]);
        let current = sink.view.load();
        assert!(
            current
                .facts
                .observed
                .get(&project)
                .is_none_or(|observed| observed.facts.root_listing != old_listing),
            "a pre-write probe replaced the committed root listing"
        );
    }

    #[test]
    fn mutating_a_workspace_dependency_rejects_an_older_summary_probe() {
        let dir = tempfile::tempdir().unwrap();
        let first = init_repo_named(dir.path(), "first");
        let second = init_repo_named(dir.path(), "second");
        let mut app = test_app(dir.path());
        let project = app.add_project(first, "main".into());
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        call(
            &handler,
            "project.add_source",
            json!({"project_id":project,"path":second,"name":"second"}),
        );
        let workspace = call(
            &handler,
            "workspace.create",
            json!({"project_id":project,"name":"work","isolation":"worktree"}),
        );
        let workspace_id = workspace["workspace_id"].as_str().unwrap();
        let owner = call(
            &handler,
            "workspace.ensure_conversation",
            json!({"workspace_id":workspace_id}),
        );
        let entity = owner["entity_id"].as_str().unwrap();
        let (sink, old) = paused_observation(&state, entity);
        assert_eq!(old.request.summary.as_ref().unwrap().1.len(), 2);
        let directory = workspace["directories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|directory| directory["source_id"] == "source-2")
            .unwrap();
        let root = PathBuf::from(directory["path"].as_str().unwrap());
        assert_ne!(old.request.subject.root, root);
        std::fs::write(root.join("later.txt"), "changed").unwrap();
        git_in(&root, &["add", "later.txt"]);
        call(
            &handler,
            "git.commit",
            json!({"workspace_id":workspace_id,"source_id":"source-2","message":"later"}),
        );
        assert!(
            !old.request.is_current(&sink.view.load().facts),
            "a dependency mutation retained an older aggregate probe's token"
        );
        let old_summary = old.observation.summary.clone();
        sink.apply(vec![old]);
        assert!(
            sink.view
                .load()
                .facts
                .observed
                .get(entity)
                .is_none_or(|observed| observed.summary != old_summary),
            "a pre-commit probe reinstalled a summary containing an older dependency"
        );
    }
}
