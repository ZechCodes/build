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
            summary: facts.summaries.get(entity).cloned(),
            git,
            files,
        }
    }

    fn merge(&mut self, newer: Self) {
        if Arc::ptr_eq(&self.subject, &newer.subject) {
            self.git |= newer.git;
            self.files |= newer.files;
        } else {
            *self = newer;
        }
    }

    fn probe(self, facts: &PushFacts) -> Option<ProbeResult> {
        if !facts.same_subject(&self.entity, &self.subject) {
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
                current
                    .facts
                    .same_subject(&result.request.entity, &result.request.subject)
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
        let summaries = changed.iter().fold(false, |summaries, result| {
            apply_cache_observation(&mut app, result, &current.facts, &memberships) || summaries
        });
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
        let current = self.view.load();
        let mut facts = (*current.facts).clone();
        for result in changed {
            if facts.same_subject(&result.request.entity, &result.request.subject) {
                facts.observed.insert(
                    result.request.entity.clone(),
                    Arc::new(result.observation.clone()),
                );
            }
        }
        self.view
            .publish_delta(SnapshotDelta::new(self.view.next_version()).set_facts(Arc::new(facts)));
    }
}

fn apply_cache_observation(
    app: &mut AppState,
    result: &ProbeResult,
    previous: &PushFacts,
    memberships: &[(String, Vec<PathBuf>)],
) -> bool {
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
