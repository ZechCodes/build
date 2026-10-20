use super::*;
use std::cell::Cell;

thread_local! { static SERIALIZED: Cell<usize> = const { Cell::new(0) }; }

pub(super) fn record_serialized() {
    SERIALIZED.with(|count| count.set(count.get() + 1));
}

fn serialized(work: impl FnOnce()) -> usize {
    let previous = SERIALIZED.replace(0);
    work();
    SERIALIZED.replace(previous)
}

struct Fixture {
    _root: tempfile::TempDir,
    app: AppState,
    project: String,
    agents: Vec<(String, String)>,
}

impl Fixture {
    fn new(owners: usize, items: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let context = crate::harness::HarnessContext::resolved(
            root.path().join("mcp.sock"),
            root.path().to_owned(),
        )
        .unwrap();
        let mut app =
            AppState::new_unrooted_configured(root.path().join("wt"), "main", true, context);
        let project = app.add_project(root.path().to_owned(), "main".into());
        let mut agents = Vec::new();
        for index in 0..owners {
            let owner = format!("tail-owner-{index}");
            let checkout = root.path().join(&owner);
            std::fs::create_dir(&checkout).unwrap();
            let mut run = crate::orchestrator::ActiveRun::workspace_conversation(
                crate::run::RunId::new(&owner),
                owner.clone(),
                checkout,
                Default::default(),
            );
            let agent = run
                .agents
                .add(owner.as_str(), Default::default(), "2026-10-19T00:00:00Z")
                .id
                .clone();
            for item in 0..items {
                run.agents.by_id_mut(&agent).unwrap().thread.post_user(
                    format!("resident message {item}"),
                    None,
                    "2026-10-19T00:00:00Z",
                );
            }
            app.projects.bind_entity(owner.clone(), project.clone());
            app.runs.insert(owner.clone(), run);
            agents.push((owner, agent));
        }
        app.initialize_snapshot_projection();
        Self {
            _root: root,
            app,
            project,
            agents,
        }
    }

    fn thread(&self, index: usize) -> &Thread {
        let (owner, agent) = &self.agents[index];
        &self.app.runs[owner].agents.by_id(agent).unwrap().thread
    }

    fn thread_mut(&mut self, index: usize) -> &mut Thread {
        let (owner, agent) = &self.agents[index];
        self.app.mark_snapshot_owner_pending(owner);
        &mut self
            .app
            .runs
            .get_mut(owner)
            .unwrap()
            .agents
            .by_id_mut(agent)
            .unwrap()
            .thread
    }

    fn tip(&self, index: usize, cursor: u64) -> ThreadTip {
        let (owner, agent) = &self.agents[index];
        self.app
            .read_view
            .load()
            .facts
            .fact(&FactsRequest {
                entity_id: owner.clone(),
                thread: true,
                thread_after: vec![(agent.clone(), cursor)],
                ..Default::default()
            })
            .threads
            .remove(0)
    }

    fn publish_counted(&self) -> usize {
        serialized(|| self.app.publish_all_snapshot_projection())
    }
}

#[test]
fn metadata_only_global_publication_does_not_serialize_unchanged_tails() {
    let mut fixture = Fixture::new(16, 32);
    let summary = crate::session_summary::SessionSummary::default().updated(300);
    fixture
        .app
        .session_summaries
        .insert(fixture.project.clone(), summary);
    let count = fixture.publish_counted();
    eprintln!("metadata-only publication serialized {count} tail items");
    assert_eq!(
        count, 0,
        "metadata-only publication serialized {count} tail items"
    );
    assert_eq!(fixture.tip(0, 32).project_session, Some(summary));
}

#[test]
fn incremental_fact_capture_refreshes_shared_project_session_without_copying_unchanged_tails() {
    let mut fixture = Fixture::new(3, 8);
    let initial = crate::session_summary::SessionSummary::default().updated(300);
    fixture
        .app
        .session_summaries
        .insert(fixture.project.clone(), initial);
    fixture.app.publish_all_snapshot_projection();
    let previous = fixture.app.read_view.load();
    let changed = fixture.agents[0].0.clone();
    let unchanged = fixture.agents[1].0.clone();
    let tail = Arc::clone(&previous.facts.threads[&unchanged][0].items);
    assert_eq!(
        previous.facts.threads[&unchanged][0].tip.project_session,
        Some(initial)
    );

    let later = initial.updated(600);
    fixture
        .app
        .session_summaries
        .insert(fixture.project.clone(), later);
    let next = fixture
        .app
        .push_facts_incremental_snapshot(previous.facts.lists.clone(), &BTreeSet::from([changed]));
    assert!(Arc::ptr_eq(&tail, &next.threads[&unchanged][0].items));
    assert_eq!(
        previous.facts.threads[&unchanged][0].tip.project_session,
        Some(initial),
        "updating shared metadata must preserve the old committed root"
    );
    assert_eq!(
        next.threads[&unchanged][0].tip.project_session,
        Some(later),
        "an unchanged conversation still carries the current project session"
    );
}

#[test]
fn incremental_fact_capture_shares_unchanged_owners_and_matches_full_capture() {
    let mut fixture = Fixture::new(3, 8);
    for (owner, _) in &fixture.agents {
        let attention = fixture.app.board.attention_mut();
        attention.record_created_if_absent(owner, "2026-10-19T00:00:00Z".into());
        attention.seed_anchor(owner, "2026-10-19T00:00:00Z");
    }
    fixture.app.publish_all_snapshot_projection();
    let previous = fixture.app.read_view.load();
    let changed = fixture.agents[0].0.clone();
    let unchanged = fixture.agents[1].0.clone();
    let old_state = previous.facts.states[&changed].materialize();
    fixture
        .thread_mut(0)
        .post_agent("A new committed message", None, "2026-10-19T00:01:00Z");
    let next = fixture.app.push_facts_incremental_snapshot(
        previous.facts.lists.clone(),
        &BTreeSet::from([changed.clone()]),
    );
    let full = fixture
        .app
        .push_facts_shared_snapshot(previous.facts.lists.clone());
    assert!(next.states[&unchanged].ptr_eq(&previous.facts.states[&unchanged]));
    assert!(Arc::ptr_eq(
        &next.threads[&unchanged][0].items,
        &previous.facts.threads[&unchanged][0].items
    ));
    assert_eq!(previous.facts.states[&changed].materialize(), old_state);
    assert_eq!(
        next.states.keys().collect::<Vec<_>>(),
        full.states.keys().collect::<Vec<_>>()
    );
    for (owner, state) in &next.states {
        assert_eq!(
            state.materialize(),
            full.states[owner].materialize(),
            "{owner}"
        );
    }
    for (owner, agent) in &fixture.agents {
        let request = FactsRequest {
            entity_id: owner.clone(),
            state: true,
            thread: true,
            terminals: true,
            thread_after: vec![(agent.clone(), 0)],
            ..Default::default()
        };
        assert_eq!(next.fact(&request), full.fact(&request), "{owner}");
    }
}

#[test]
fn changing_one_conversation_serializes_only_its_bounded_tail() {
    let mut fixture = Fixture::new(16, 32);
    let before = fixture.thread(0).last_sequence();
    fixture
        .thread_mut(0)
        .post_agent("one changed conversation", None, "2026-10-19T00:01:00Z");
    let count = fixture.publish_counted();
    eprintln!("one changed conversation serialized {count} tail items");
    assert!(
        count > 0,
        "changed local items must exercise the serializer counter"
    );
    assert!(
        count <= THREAD_PUSH_MAX_ITEMS,
        "one changed conversation serialized {count} items"
    );
    let tip = fixture.tip(0, before);
    assert_eq!(tip.since_sequence, Some(before));
    assert_eq!(tip.items.len(), 1);
    assert_eq!(tip.items[0]["data"]["body"], "one changed conversation");
}

#[test]
fn same_publication_sequence_removal_invalidates_the_cached_tail() {
    let mut fixture = Fixture::new(1, 32);
    let publication = fixture.thread(0).publication_sequence();
    let removed = fixture.thread_mut(0).items.pop().unwrap().sequence();
    assert_eq!(fixture.thread(0).publication_sequence(), publication);
    let count = fixture.publish_counted();
    assert!(count > 0 && count <= THREAD_PUSH_MAX_ITEMS);
    let tip = fixture.tip(0, 0);
    assert_eq!(
        tip.items,
        fixture
            .thread(0)
            .push_items_after(0, THREAD_PUSH_MAX_ITEMS)
            .unwrap()
    );
    assert!(tip
        .items
        .iter()
        .all(|row| row["data"]["sequence"] != removed));
}

fn historical_receipt() -> (Fixture, crate::store::CommittedThreadItems, u64) {
    let mut fixture = Fixture::new(1, THREAD_PUSH_MAX_ITEMS * 3);
    let mut complete = fixture.thread(0).clone();
    let before = complete.last_sequence();
    let tail = complete.items[complete.items.len() - 8..].to_vec();
    fixture
        .thread_mut(0)
        .adopt_stored_tail(tail, before - 8, before);
    fixture.app.publish_all_snapshot_projection();
    complete.set_legacy_delivery_status(1, 1, crate::thread::MessageDeliveryStatus::Seen);
    let delta = crate::store::CommittedThreadItems {
        conversation_id: complete.agent.id.clone(),
        thread_id: complete.id.clone(),
        generation_revision: complete.generation_revision,
        sequence: complete.last_sequence(),
        items: vec![complete.items[0].clone()],
    };
    fixture.app.apply_committed_thread_items(&delta);
    fixture.app.publish_all_snapshot_projection();
    (fixture, delta, before)
}

#[test]
fn reused_tail_retains_external_receipts_and_the_stored_history_floor() {
    let (fixture, delta, before) = historical_receipt();
    assert_eq!(fixture.publish_counted(), 0);
    let tip = fixture.tip(0, before);
    assert_eq!(tip.last_sequence, delta.sequence);
    assert_eq!(tip.items, vec![json!(delta.items[0])]);
    let gap = fixture.tip(0, 0);
    assert!(gap.items.is_empty());
    assert_eq!(gap.since_sequence, None);
}

#[test]
fn reset_tail_cannot_retain_an_older_external_receipt() {
    let (mut fixture, delta, _) = historical_receipt();
    let mut replacement = Thread::for_agent(&fixture.agents[0].1);
    replacement.generation_revision = 1;
    let replacement_id = replacement.id.clone();
    *fixture.thread_mut(0) = replacement;
    fixture.app.publish_all_snapshot_projection();
    fixture.app.apply_committed_thread_items(&delta);
    fixture.app.publish_all_snapshot_projection();
    let tip = fixture.tip(0, 0);
    assert_eq!(tip.thread_id.as_deref(), Some(replacement_id.as_str()));
    assert_eq!(tip.thread_generation_revision, Some(1));
    assert_eq!(tip.last_sequence, 0);
    assert!(tip.items.is_empty());
}
