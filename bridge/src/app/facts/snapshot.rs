//! Bounded, immutable inputs for a push flush. No application/store handle is
//! consulted while rendering these rows and conversation tails.

mod terminals;

use super::observations::{CheckoutObservation, ObservationRequest, ObservationSink};
use super::{AppState, CheckoutSubject, ReadSubject};
use crate::app::read_view::shared_json::DigestPatch;
use crate::app::read_view::{ReadViewSlot, SharedJson};
use crate::changes::{BoardLists, EntityFacts, FactsRequest, ThreadTip, THREAD_PUSH_MAX_ITEMS};
use crate::thread::Thread;
use serde_json::{json, Value};
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[derive(Clone, Default)]
pub(in crate::app) struct PushFacts {
    pub(super) subjects: BTreeMap<String, Arc<CheckoutSubject>>,
    entities: BTreeMap<String, Arc<()>>,
    observation_generations: BTreeMap<String, Arc<()>>,
    pub(super) observed: BTreeMap<String, Arc<CheckoutObservation>>,
    states: BTreeMap<String, SharedJson>,
    terminals: BTreeMap<String, Arc<Value>>,
    threads: BTreeMap<String, Vec<PushThread>>,
    lists: SharedJson,
    pub(super) summaries: BTreeMap<String, (String, Vec<PathBuf>)>,
}

#[cfg(test)]
impl PushFacts {
    pub(in crate::app) fn state_for_test(&self, owner: &str) -> Option<SharedJson> {
        self.states.get(owner).cloned()
    }
}

#[derive(Clone)]
struct PushThread {
    tip: ThreadTip,
    floor: u64,
    source: TailSource,
    items: Arc<Vec<(u64, Value)>>,
    external: Arc<BTreeMap<u64, (u64, Value)>>,
    external_sequence: u64,
}

/// Tip identity fences resets/bindings; this local shape also detects a
/// withdrawn resident item whose monotonic publication counter did not move.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TailSource {
    publication_sequence: u64,
    total_count: u64,
    resident_from: u64,
    resident_count: usize,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
}

impl TailSource {
    fn capture(thread: &Thread) -> Self {
        Self {
            publication_sequence: thread.publication_sequence(),
            total_count: thread.total_item_count(),
            resident_from: thread.resident_from_sequence(),
            resident_count: thread.items.len(),
            first_sequence: thread
                .items
                .first()
                .map(crate::thread::ThreadItem::sequence),
            last_sequence: thread.items.last().map(crate::thread::ThreadItem::sequence),
        }
    }
}

impl PushThread {
    fn recapture(tip: ThreadTip, thread: &Thread, previous: Option<&Self>) -> Self {
        let previous = previous.filter(|previous| same_tip_identity(&previous.tip, &tip));
        if let Some(previous) =
            previous.filter(|previous| previous.source == TailSource::capture(thread))
        {
            let mut retained = previous.clone();
            retained.tip = tip;
            retained.tip.last_sequence = retained.tip.last_sequence.max(retained.external_sequence);
            return retained;
        }
        let mut captured = Self::capture(tip, thread);
        if let Some(previous) = previous {
            captured.merge_previous(previous);
        }
        captured
    }

    fn capture(tip: ThreadTip, thread: &Thread) -> Self {
        // Keep only the newest mutation counters, not the resident history.
        // The greatest omitted counter certifies every supported cursor.
        let mut newest = BTreeSet::new();
        for item in &thread.items {
            newest.insert(item.latest_sequence());
            if newest.len() > THREAD_PUSH_MAX_ITEMS + 1 {
                newest.pop_first();
            }
        }
        let mut floor = if newest.len() > THREAD_PUSH_MAX_ITEMS {
            newest.first().copied().unwrap_or(0)
        } else {
            0
        };
        if thread.forward_page_reaches_stored_history(floor) {
            floor = thread
                .items
                .first()
                .map(|item| item.sequence().saturating_sub(1))
                .unwrap_or(thread.last_sequence());
        }
        let items = thread
            .items
            .iter()
            .filter(|item| item.latest_sequence() > floor)
            .map(|item| {
                #[cfg(test)]
                tail_tests::record_serialized();
                (item.latest_sequence(), json!(item))
            })
            .collect();
        Self {
            tip,
            floor,
            source: TailSource::capture(thread),
            items: Arc::new(items),
            external: Arc::new(BTreeMap::new()),
            external_sequence: 0,
        }
    }

    fn merge_previous(&mut self, previous: &Self) {
        self.external_sequence = previous.external_sequence;
        self.tip.last_sequence = self.tip.last_sequence.max(previous.external_sequence);
        let mut merged = self
            .items
            .iter()
            .map(|(latest, item)| {
                let sequence = item["data"]["sequence"].as_u64().unwrap_or(*latest);
                (sequence, (*latest, item.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        for (sequence, (latest, item)) in previous.external.iter() {
            if merged.get(sequence).is_none_or(|(held, _)| held < latest) {
                merged.insert(*sequence, (*latest, item.clone()));
                Arc::make_mut(&mut self.external).insert(*sequence, (*latest, item.clone()));
            }
        }
        if !self.external.is_empty() {
            self.floor = self.floor.max(previous.floor);
        }
        while merged.len() > THREAD_PUSH_MAX_ITEMS {
            let (&sequence, (latest, _)) =
                merged.iter().min_by_key(|(_, (latest, _))| latest).unwrap();
            self.floor = self.floor.max(*latest);
            merged.remove(&sequence);
        }
        Arc::make_mut(&mut self.external)
            .retain(|sequence, (latest, _)| *latest > self.floor && merged.contains_key(sequence));
        self.items = Arc::new(
            merged
                .into_values()
                .filter(|(latest, _)| *latest > self.floor)
                .collect(),
        );
    }

    fn apply_committed(&mut self, delta: &crate::store::CommittedThreadItems) {
        self.tip.last_sequence = self.tip.last_sequence.max(delta.sequence);
        self.external_sequence = self.external_sequence.max(delta.sequence);
        let mut selected = BTreeMap::new();
        for (latest, value) in self.items.iter() {
            let sequence = value["data"]["sequence"].as_u64().unwrap_or(*latest);
            select_push_item(
                &mut selected,
                sequence,
                *latest,
                PushItem::Published(value),
                &mut self.floor,
            );
        }
        for item in &delta.items {
            select_push_item(
                &mut selected,
                item.sequence(),
                item.latest_sequence(),
                PushItem::Committed(item),
                &mut self.floor,
            );
        }
        let mut external = BTreeMap::new();
        let mut items = Vec::new();
        for (sequence, (latest, item)) in selected {
            if latest <= self.floor {
                continue;
            }
            let retained =
                matches!(&item, PushItem::Committed(_)) || self.external.contains_key(&sequence);
            let value = item.render();
            if retained {
                external.insert(sequence, (latest, value.clone()));
            }
            items.push((latest, value));
        }
        self.items = Arc::new(items);
        self.external = Arc::new(external);
    }

    fn render(&self, after: &[(String, u64)]) -> ThreadTip {
        let mut tip = self.tip.clone();
        if let Some((_, since)) = after
            .iter()
            .find(|(id, _)| *id == tip.agent_id)
            .filter(|(_, since)| *since >= self.floor)
        {
            tip.since_sequence = Some(*since);
            tip.items = self
                .items
                .iter()
                .filter(|(sequence, _)| sequence > since)
                .map(|(_, item)| item.clone())
                .collect();
        }
        tip
    }
}

enum PushItem<'a> {
    Published(&'a Value),
    Committed(&'a crate::thread::ThreadItem),
}

impl PushItem<'_> {
    fn render(self) -> Value {
        match self {
            Self::Published(value) => value.clone(),
            Self::Committed(item) => json!(item),
        }
    }
}

fn select_push_item<'a>(
    selected: &mut BTreeMap<u64, (u64, PushItem<'a>)>,
    sequence: u64,
    latest: u64,
    item: PushItem<'a>,
    floor: &mut u64,
) {
    if selected
        .get(&sequence)
        .is_none_or(|(held, _)| *held <= latest)
    {
        selected.insert(sequence, (latest, item));
    }
    if selected.len() > THREAD_PUSH_MAX_ITEMS {
        let (&sequence, (latest, _)) = selected
            .iter()
            .min_by_key(|(_, (latest, _))| latest)
            .unwrap();
        *floor = (*floor).max(*latest);
        selected.remove(&sequence);
    }
}

pub(in crate::app) type FactsHandle = Arc<OnceLock<FactsSource>>;

pub(in crate::app) struct FactsSource {
    pub(super) view: ReadViewSlot,
    observations: Arc<ObservationSink>,
}

impl FactsSource {
    pub(in crate::app) fn new(view: ReadViewSlot, state: Weak<Mutex<AppState>>) -> Self {
        Self {
            observations: Arc::new(ObservationSink::new(view.clone(), state)),
            view,
        }
    }

    pub(super) fn entity_ids(&self) -> Vec<String> {
        let snapshot = self.view.load();
        snapshot
            .facts
            .subjects
            .keys()
            .chain(snapshot.facts.terminals.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

impl PushFacts {
    pub(in crate::app) fn patch_terminal_attention(
        &mut self,
        patch: &mut crate::app::read_view::shared_json::AttentionPatch<'_>,
    ) {
        for row in self
            .states
            .values_mut()
            .chain(std::iter::once(&mut self.lists))
        {
            if let Some(next) = patch.apply(row) {
                *row = next;
            }
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn apply_committed_history_delta(
        &mut self,
        delta: &crate::store::CommittedThreadItems,
    ) {
        let mut patch =
            DigestPatch::new(&delta.thread_id, delta.generation_revision, delta.sequence);
        self.apply_committed_history_delta_with_patch(delta, &mut patch);
    }

    pub(in crate::app) fn apply_committed_history_delta_with_patch(
        &mut self,
        delta: &crate::store::CommittedThreadItems,
        patch: &mut DigestPatch<'_>,
    ) {
        for thread in self.threads.values_mut().flatten().filter(|thread| {
            thread.tip.conversation_id.as_deref() == Some(delta.conversation_id.as_str())
                && thread.tip.thread_id.as_deref() == Some(delta.thread_id.as_str())
                && thread.tip.thread_generation_revision == Some(delta.generation_revision)
        }) {
            thread.apply_committed(delta);
        }
        for state in self
            .states
            .values_mut()
            .chain(std::iter::once(&mut self.lists))
        {
            if let Some(next) = patch.apply(state) {
                *state = next;
            }
        }
    }

    pub(super) fn observation_generation(&self, entity: &str) -> Arc<()> {
        Arc::clone(
            self.observation_generations
                .get(entity)
                .expect("every published checkout has an observation generation"),
        )
    }

    pub(super) fn same_observation_generation(&self, entity: &str, generation: &Arc<()>) -> bool {
        self.observation_generations
            .get(entity)
            .is_some_and(|current| Arc::ptr_eq(current, generation))
    }

    pub(super) fn invalidate_observations(&mut self, entities: &[String]) {
        for entity in entities {
            self.observation_generations
                .insert(entity.clone(), Arc::new(()));
            self.observed.remove(entity);
        }
    }

    pub(super) fn same_subject(&self, entity: &str, subject: &Arc<CheckoutSubject>) -> bool {
        self.subjects
            .get(entity)
            .is_some_and(|current| Arc::ptr_eq(current, subject))
    }

    fn fact(&self, request: &FactsRequest) -> EntityFacts {
        let mut fact = EntityFacts {
            entity_id: request.entity_id.clone(),
            state: if request.lists.is_empty() {
                request
                    .state
                    .then(|| {
                        self.states
                            .get(&request.entity_id)
                            .map(SharedJson::materialize)
                    })
                    .flatten()
            } else {
                Some(self.lists(request.lists))
            },
            terminals: request
                .terminals
                .then(|| {
                    self.terminals
                        .get(&request.entity_id)
                        .map(|terminals| terminals.as_ref().clone())
                })
                .flatten(),
            threads: if request.thread {
                self.threads
                    .get(&request.entity_id)
                    .into_iter()
                    .flatten()
                    .map(|thread| thread.render(&request.thread_after))
                    .collect()
            } else {
                Vec::new()
            },
            ..EntityFacts::default()
        };
        self.copy_observed(request, &mut fact);
        fact
    }

    fn copy_observed(&self, request: &FactsRequest, fact: &mut EntityFacts) {
        let Some(observed) = self
            .observed
            .get(&request.entity_id)
            .map(|observed| &observed.facts)
        else {
            return;
        };
        if request.git {
            fact.status_key = observed.status_key.clone();
            fact.head = observed.head.clone();
            fact.status = observed.status.clone();
            fact.log = observed.log.clone();
            fact.unpushed = observed.unpushed.clone();
            fact.diff = observed.diff.clone();
            fact.diff_bytes = observed.diff_bytes;
        }
        if request.files {
            fact.root_listing = observed.root_listing.clone();
        }
    }

    fn observation_request(&self, request: &FactsRequest) -> Option<ObservationRequest> {
        if !request.git && !request.files {
            return None;
        }
        let subject = self.subjects.get(&request.entity_id)?;
        Some(ObservationRequest::new(
            &request.entity_id,
            subject,
            self,
            request.git,
            request.files,
        ))
    }

    fn withdraw_replaced(&self, current: &Self, facts: &mut [EntityFacts]) {
        for fact in facts {
            let same = match (
                self.entities.get(&fact.entity_id),
                current.entities.get(&fact.entity_id),
            ) {
                (Some(old), Some(live)) => Arc::ptr_eq(old, live),
                _ => {
                    fact.entity_id == crate::changes::BOARD_ITEM_ID
                        && self.entities.iter().all(|(entity, old)| {
                            current
                                .entities
                                .get(entity)
                                .is_some_and(|live| Arc::ptr_eq(old, live))
                        })
                }
            };
            if !same {
                *fact = EntityFacts {
                    entity_id: std::mem::take(&mut fact.entity_id),
                    withdrawn: true,
                    ..EntityFacts::default()
                };
            }
        }
    }

    fn lists(&self, requested: BoardLists) -> Value {
        let fields = [
            ("projects", requested.projects),
            ("workspaces", requested.workspaces),
            ("usage_limits", requested.usage_limits),
        ];
        Value::Object(
            fields
                .into_iter()
                .filter(|(_, wanted)| *wanted)
                .filter_map(|(name, _)| {
                    self.lists
                        .field(name)
                        .map(|value| (name.to_owned(), value.materialize()))
                })
                .collect(),
        )
    }
}

/// Every logical value in one flush comes from one committed root. External
/// observations are generation-fenced after their I/O and applied separately.
pub(in crate::app) fn entity_facts(
    handle: &FactsHandle,
    requests: &[FactsRequest],
) -> Vec<EntityFacts> {
    let Some(source) = handle.get() else {
        return Vec::new();
    };
    let snapshot = source.view.load();
    let mut facts = requests
        .iter()
        .map(|request| snapshot.facts.fact(request))
        .collect::<Vec<_>>();
    let observations = requests
        .iter()
        .filter_map(|request| snapshot.facts.observation_request(request))
        .collect();
    let current = source.view.load();
    snapshot.facts.withdraw_replaced(&current.facts, &mut facts);
    source.observations.submit(observations);
    facts
}

impl AppState {
    pub(in crate::app) fn push_facts_shared_snapshot(&self, lists: SharedJson) -> PushFacts {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.push_facts");
        let previous = self.read_view.load();
        let mut snapshot = PushFacts {
            lists,
            ..PushFacts::default()
        };
        #[cfg(test)]
        let owners_phase = crate::timing::phase_samples::phase("publication.push_facts.owners");
        for owner in self.plans.keys().chain(self.runs.keys()) {
            {
                if let Some(state) = self.entity_state_shared_item(owner) {
                    snapshot.states.insert(owner.clone(), state);
                }
            }
            {
                snapshot.threads.insert(
                    owner.clone(),
                    self.push_thread_tails(owner, &previous.facts),
                );
            }
        }
        #[cfg(test)]
        drop(owners_phase);
        self.capture_push_checkouts(&mut snapshot, &previous.facts);
        self.capture_terminal_aliases(&mut snapshot, &previous.facts);
        self.capture_push_entities(&mut snapshot, &previous.facts);
        snapshot
    }

    fn push_thread_tails(&self, owner: &str, previous: &PushFacts) -> Vec<PushThread> {
        self.thread_tips(owner, &[])
            .into_iter()
            .filter_map(|tip| {
                let agent = self
                    .entity_agents(owner)
                    .ok()?
                    .iter()
                    .find(|agent| agent.id == tip.agent_id)?;
                let thread = self
                    .agent_conversation(owner, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                let old = previous
                    .threads
                    .get(owner)
                    .into_iter()
                    .flatten()
                    .find(|old| same_tip_identity(&old.tip, &tip));
                Some(PushThread::recapture(tip, thread, old))
            })
            .collect()
    }

    fn capture_push_checkouts(&self, snapshot: &mut PushFacts, previous: &PushFacts) {
        let subjects = {
            #[cfg(test)]
            let _phase =
                crate::timing::phase_samples::phase("publication.push_facts.checkout_subjects");
            self.checkout_subjects()
        };
        #[cfg(test)]
        let _checkouts_phase =
            crate::timing::phase_samples::phase("publication.push_facts.checkouts");
        for (entity, subject) in subjects {
            {
                if let Entry::Vacant(entry) = snapshot.states.entry(entity.clone()) {
                    if let Some(state) = self.entity_state_shared_item(&entity) {
                        entry.insert(state);
                    }
                }
            }
            {
                let mut scope = subject.terminal_scope();
                scope.root = self.cached_snapshot_root(&scope.root);
                snapshot.terminals.insert(
                    entity.clone(),
                    Arc::new(json!({"tabs": self.shell_tabs_for(&scope)})),
                );
            }
            {
                if let Some(workspace) = self.workspace_snapshot_for_conversation(&entity, None) {
                    let repositories = workspace
                        .directories
                        .iter()
                        .filter(|directory| directory.is_git)
                        .map(|directory| directory.path.clone())
                        .collect();
                    snapshot
                        .summaries
                        .insert(entity.clone(), (workspace.id.clone(), repositories));
                }
            }
            let retained = previous.subjects.get(&entity).filter(|old| {
                same_checkout(old, &subject)
                    && previous.summaries.get(&entity) == snapshot.summaries.get(&entity)
                    && same_threads(previous.threads.get(&entity), snapshot.threads.get(&entity))
            });
            let subject = retained.cloned().unwrap_or_else(|| Arc::new(subject));
            if previous.same_subject(&entity, &subject) {
                if let Some(observed) = previous.observed.get(&entity) {
                    snapshot
                        .observed
                        .insert(entity.clone(), Arc::clone(observed));
                }
            }
            snapshot.subjects.insert(entity, subject);
        }
    }

    fn capture_push_entities(&self, snapshot: &mut PushFacts, previous: &PushFacts) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.push_facts.entity_maps");
        let entities = self
            .plans
            .keys()
            .chain(self.runs.keys())
            .chain(self.projects.iter().map(|project| &project.id))
            .chain(
                self.workspaces
                    .list(None)
                    .into_iter()
                    .map(|workspace| &workspace.id),
            )
            .chain(snapshot.subjects.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for entity in entities {
            let same_subject = match (
                previous.subjects.get(&entity),
                snapshot.subjects.get(&entity),
            ) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            };
            let retained = previous.entities.get(&entity).filter(|_| {
                same_subject
                    && same_threads(previous.threads.get(&entity), snapshot.threads.get(&entity))
            });
            let generation = retained.cloned().unwrap_or_else(|| Arc::new(()));
            let observation_generation = previous
                .entities
                .get(&entity)
                .filter(|old| Arc::ptr_eq(old, &generation))
                .and_then(|_| previous.observation_generations.get(&entity))
                .cloned()
                .unwrap_or_else(|| Arc::new(()));
            snapshot
                .observation_generations
                .insert(entity.clone(), observation_generation);
            snapshot.entities.insert(entity, generation);
        }
    }
}

fn same_tip_identity(left: &ThreadTip, right: &ThreadTip) -> bool {
    left.agent_id == right.agent_id
        && left.conversation_id == right.conversation_id
        && left.thread_id == right.thread_id
        && left.thread_generation_revision == right.thread_generation_revision
}

fn same_threads(left: Option<&Vec<PushThread>>, right: Option<&Vec<PushThread>>) -> bool {
    let left = left.into_iter().flatten();
    let right = right.into_iter().flatten();
    left.clone().count() == right.clone().count()
        && left
            .zip(right)
            .all(|(left, right)| same_tip_identity(&left.tip, &right.tip))
}

fn checkout_base(subject: &CheckoutSubject) -> Option<&str> {
    match &subject.diff {
        Some(ReadSubject::Run { base_sha, .. }) => base_sha.as_deref(),
        _ => None,
    }
}

fn same_checkout(left: &CheckoutSubject, right: &CheckoutSubject) -> bool {
    left.root == right.root
        && left.base_branch == right.base_branch
        && left.namespace == right.namespace
        && checkout_base(left) == checkout_base(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tip(thread: &Thread) -> ThreadTip {
        ThreadTip {
            agent_id: "agent".into(),
            last_sequence: thread.last_sequence(),
            ..ThreadTip::default()
        }
    }

    #[test]
    fn bounded_tail_preserves_new_mutations_of_old_items_and_tip_only_gaps() {
        let mut thread = Thread::for_agent("agent");
        for index in 0..(THREAD_PUSH_MAX_ITEMS * 4) {
            thread.post_user(index.to_string(), None, "now");
        }
        let after = thread.last_sequence();
        thread.set_legacy_delivery_status(1, 1, crate::thread::MessageDeliveryStatus::Seen);
        let snapshot = PushThread::capture(tip(&thread), &thread);
        assert_eq!(snapshot.items.len(), THREAD_PUSH_MAX_ITEMS);
        let delta = snapshot.render(&[("agent".into(), after)]);
        assert_eq!(delta.since_sequence, Some(after));
        assert_eq!(
            delta.items,
            thread
                .push_items_after(after, THREAD_PUSH_MAX_ITEMS)
                .unwrap()
        );
        assert_eq!(delta.items[0]["data"]["sequence"], 1);
        let gap = snapshot.render(&[("agent".into(), 0)]);
        assert!(gap.items.is_empty());
        assert_eq!(gap.since_sequence, None);
        let initial = snapshot.render(&[]);
        assert!(initial.items.is_empty());
        assert_eq!(initial.last_sequence, thread.last_sequence());
    }

    #[test]
    fn bounded_tail_does_not_certify_a_cursor_below_resident_history() {
        let mut complete = Thread::for_agent("agent");
        for index in 0..32 {
            complete.post_user(index.to_string(), None, "now");
        }
        let mut thread = Thread::for_agent("agent");
        thread.adopt_stored_tail(complete.items[24..].to_vec(), 24, complete.last_sequence());
        let snapshot = PushThread::capture(tip(&thread), &thread);
        assert!(snapshot.render(&[("agent".into(), 20)]).items.is_empty());
        assert_eq!(
            snapshot.render(&[("agent".into(), 24)]).items,
            thread.push_items_after(24, THREAD_PUSH_MAX_ITEMS).unwrap()
        );
    }

    #[test]
    fn committed_historical_mutations_survive_recapture_without_restoring_removed_items() {
        let mut complete = Thread::for_agent("agent");
        for index in 0..(THREAD_PUSH_MAX_ITEMS * 3) {
            complete.post_user(index.to_string(), None, "now");
        }
        let before = complete.last_sequence();
        let mut resident = Thread::for_agent("agent");
        resident.adopt_stored_tail(
            complete.items[complete.items.len() - 8..].to_vec(),
            before - 8,
            before,
        );
        let mut published = PushThread::capture(tip(&resident), &resident);
        complete.set_legacy_delivery_status(1, 1, crate::thread::MessageDeliveryStatus::Seen);
        let delta = crate::store::CommittedThreadItems {
            conversation_id: "agent".into(),
            thread_id: complete.id.clone(),
            generation_revision: 0,
            sequence: complete.last_sequence(),
            items: vec![complete.items[0].clone()],
        };
        published.apply_committed(&delta);
        let removed = resident.items.pop().unwrap().sequence();
        let mut recaptured = PushThread::capture(tip(&resident), &resident);
        recaptured.merge_previous(&published);
        let tip = recaptured.render(&[("agent".into(), before)]);
        assert_eq!(tip.last_sequence, delta.sequence);
        assert_eq!(tip.items, vec![json!(delta.items[0])]);
        assert!(recaptured
            .items
            .iter()
            .all(|(_, value)| value["data"]["sequence"] != removed));
        assert!(recaptured.items.len() <= THREAD_PUSH_MAX_ITEMS);
    }

    #[test]
    fn invalidating_observations_preserves_pending_canonical_facts() {
        let mut before = PushFacts::default();
        before.entities.insert("run".into(), Arc::new(()));
        before
            .observation_generations
            .insert("run".into(), Arc::new(()));
        let mut current = before.clone();
        current.invalidate_observations(&["run".into()]);
        assert!(!current.same_observation_generation("run", &before.observation_generation("run")));
        let mut facts = vec![EntityFacts {
            entity_id: "run".into(),
            state: Some(json!({"title":"committed"})),
            ..EntityFacts::default()
        }];
        before.withdraw_replaced(&current, &mut facts);
        assert!(!facts[0].withdrawn);
        assert_eq!(facts[0].state, Some(json!({"title":"committed"})));
    }

    #[test]
    fn a_recreated_owner_cannot_reuse_a_deleted_generations_facts() {
        let mut before = PushFacts::default();
        before.entities.insert("run".into(), Arc::new(()));
        let mut recreated = PushFacts::default();
        recreated.entities.insert("run".into(), Arc::new(()));
        let mut facts = vec![EntityFacts {
            entity_id: "run".into(),
            state: Some(json!({"name":"old"})),
            ..EntityFacts::default()
        }];
        before.withdraw_replaced(&recreated, &mut facts);
        assert!(facts[0].withdrawn);
        assert!(facts[0].state.is_none());
    }
}

mod owner_capture;

#[cfg(test)]
mod tail_tests;

#[cfg(test)]
mod sharing_tests;
