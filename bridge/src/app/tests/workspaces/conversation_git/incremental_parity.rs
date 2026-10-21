//! Retained immutable roots are the oracle boundary: a later owner must never
//! resurrect a rejected candidate or overwrite a committed receipt patch.
use super::*;
use crate::app::read_view::{ReadView, ReadViewSlot, SnapshotDelta};
use crate::changes::{BoardLists, EntityFacts, FactsRequest};
use std::collections::{BTreeMap, BTreeSet};

const CLOCK: &str = "2026-01-01T00:00:00Z";
const LATER_CLOCK: &str = "2026-01-01T00:01:00Z";

mod named_branch;

fn fixture() -> (tempfile::TempDir, AppState) {
    fixture_with_count(4)
}

fn fixture_with_count(count: usize) -> (tempfile::TempDir, AppState) {
    let (tmp, mut state, _) = ownership_fixture(count, false);
    add_tip_agents(&mut state, 1);
    state.store = Some(crate::store::Store::new(tmp.path().join("tasks")).unwrap());
    for number in 0..count {
        let owner = format!("run-{number:02}");
        let mut active = state.take_run(&owner).unwrap();
        active.run.goal = format!("committed goal {number}");
        active
            .agents
            .sole_thread_mut()
            .post_user("initial user message", None, CLOCK);
        state.finish_run_mutation(owner, active).unwrap();
    }
    // Raw fixture runs have no attention clocks; avoid the intentional
    // clockless-owner fallback changing wall-clock fields between renders.
    for (owner, active) in &state.runs {
        state.board.attention_mut().restore_entity_clocks(
            owner.clone(),
            CLOCK.into(),
            CLOCK.into(),
            CLOCK.into(),
            crate::app::run_state_str(&active.run.state),
        );
    }
    let projects = state
        .capture_project_snapshot_observations()
        .into_iter()
        .map(|row| (row.project_id.clone(), row.render()))
        .collect();
    state.install_project_snapshot_observations(projects);
    state.warm_snapshot_observations_for_test();
    for owner in state.runs.keys() {
        state.mark_snapshot_owner_pending(owner);
    }
    state.publish_all_snapshot_projection();
    (tmp, state)
}

fn requests(state: &AppState) -> Vec<FactsRequest> {
    let thread_after = state
        .runs
        .values()
        .flat_map(|active| active.agents.iter())
        .map(|agent| (agent.id.clone(), 0))
        .collect::<Vec<_>>();
    let mut entities = state.runs.keys().cloned().collect::<BTreeSet<_>>();
    entities.extend(state.projects.iter().map(|project| project.id.clone()));
    let mut requests = entities
        .into_iter()
        .map(|entity_id| FactsRequest {
            entity_id,
            state: true,
            thread: true,
            terminals: true,
            thread_after: thread_after.clone(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    requests.push(FactsRequest {
        entity_id: crate::changes::BOARD_ITEM_ID.into(),
        lists: BoardLists {
            projects: true,
            workspaces: true,
            usage_limits: true,
        },
        ..Default::default()
    });
    requests
}

#[derive(Debug, PartialEq, Eq)]
struct Image {
    chunks: BTreeMap<(&'static str, String), Value>,
    histories: BTreeMap<String, Value>,
    facts: Vec<EntityFacts>,
}

fn image(root: &ReadView, requests: &[FactsRequest]) -> Image {
    let chunks = root
        .keys()
        .filter(|key| {
            matches!(
                key.method,
                "board.list"
                    | "project.list"
                    | "workspace.list"
                    | "workspace.get"
                    | "agent.list"
                    | "entity.get"
            )
        })
        .filter_map(|key| {
            root.get_owned(key.method, &key.owner)
                .map(|value| ((key.method, key.owner.clone()), value))
        })
        .collect();
    let histories = root
        .history_keys()
        .map(|owner| {
            (
                owner.to_owned(),
                root.history(owner).unwrap().render(&json!({})),
            )
        })
        .collect();
    // A separate slot fixes the facts reader to this exact retained root.
    // No checkout observations are requested, and no live application is used.
    let slot = ReadViewSlot::default();
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).set_facts(Arc::clone(&root.facts)));
    let handle = crate::app::facts::FactsHandle::default();
    assert!(handle
        .set(crate::app::facts::FactsSource::new(
            slot,
            std::sync::Weak::new()
        ))
        .is_ok());
    Image {
        chunks,
        histories,
        facts: crate::app::facts::entity_facts(&handle, requests),
    }
}

fn assert_full_reference(state: &AppState, root: &ReadView, requests: &[FactsRequest]) {
    let actual = image(root, requests);
    state.publish_all_snapshot_projection();
    assert_eq!(actual, image(&state.read_view.load(), requests));
    assert_eq!(
        actual,
        image(root, requests),
        "the full producer mutated a retained root"
    );
}

fn assert_fast_eligible(state: &AppState, owner: &str) {
    let _context = state.begin_snapshot_ownership();
    assert!(
        state
            .prepare_incremental_owner_snapshot(
                SnapshotDelta::new(state.read_view.next_version()),
                &state.read_view.load(),
                &BTreeSet::from([owner.to_owned()]),
            )
            .is_some(),
        "a stable accepted layout should admit the incremental producer"
    );
}

#[test]
fn rejected_incremental_candidate_never_enters_the_accepted_cache() {
    let (_tmp, mut state) = fixture();
    let requests = requests(&state);
    let old = state.read_view.load();
    let before = image(&old, &requests);
    let original_root = state.runs["run-00"].worktree.path.clone();
    let mut rejected = state.take_run("run-00").unwrap();
    rejected.run.goal = "UNCOMMITTED GOAL".into();
    rejected
        .agents
        .sole_thread_mut()
        .post_agent("UNCOMMITTED NEWS", None, CLOCK);
    state.store.as_ref().unwrap().fail_next_write();
    assert!(state
        .finish_run_mutation("run-00".into(), rejected)
        .is_err());
    assert_eq!(state.runs["run-00"].worktree.path, original_root);
    assert_eq!(before, image(&state.read_view.load(), &requests));

    assert_fast_eligible(&state, "run-02");
    let mut accepted = state.take_run("run-02").unwrap();
    accepted.run.goal = "accepted unrelated goal".into();
    accepted
        .agents
        .sole_thread_mut()
        .post_agent("accepted unrelated news", None, LATER_CLOCK);
    state
        .finish_run_mutation("run-02".into(), accepted)
        .unwrap();
    let next = state.read_view.load();
    let rendered = format!("{:?}", image(&next, &requests));
    assert!(rendered.contains("accepted unrelated goal"));
    assert!(!rendered.contains("UNCOMMITTED"));
    assert_full_reference(&state, &next, &requests);
    assert_eq!(before, image(&old, &requests));
}

#[test]
fn accepted_owner_waits_for_failed_borrowed_peer_before_cache_publication() {
    let (_tmp, mut state) = fixture();
    let requests = requests(&state);
    let old = state.read_view.load();
    let before = image(&old, &requests);
    let mut accepted = state.take_run("run-00").unwrap();
    let mut rejected = state.take_run("run-01").unwrap();
    accepted.run.goal = "accepted while peer borrowed".into();
    accepted
        .agents
        .sole_thread_mut()
        .post_agent("accepted peer news", None, LATER_CLOCK);
    rejected.run.goal = "UNCOMMITTED BORROWED GOAL".into();
    rejected
        .agents
        .sole_thread_mut()
        .post_agent("UNCOMMITTED BORROWED NEWS", None, CLOCK);
    state
        .finish_run_mutation("run-00".into(), accepted)
        .unwrap();
    assert_eq!(
        before,
        image(&state.read_view.load(), &requests),
        "a borrowed peer must hold the publication barrier"
    );
    state.store.as_ref().unwrap().fail_next_write();
    assert!(state
        .finish_run_mutation("run-01".into(), rejected)
        .is_err());
    let next = state.read_view.load();
    let rendered = format!("{:?}", image(&next, &requests));
    assert!(rendered.contains("accepted while peer borrowed"));
    assert!(rendered.contains("accepted peer news"));
    assert!(!rendered.contains("UNCOMMITTED"));
    assert_full_reference(&state, &next, &requests);

    assert_fast_eligible(&state, "run-02");
    state.publish_owner_snapshot_projection("run-02");
    assert_full_reference(&state, &state.read_view.load(), &requests);
    assert_eq!(before, image(&old, &requests));
}

fn assert_receipt(
    image: &Image,
    canonical: &str,
    alias: &str,
    thread: &str,
    generation: u64,
    sequence: u64,
) {
    fn digests(value: &Value, thread: &str, generation: u64, sequence: u64) -> usize {
        match value {
            Value::Array(values) => values
                .iter()
                .map(|value| digests(value, thread, generation, sequence))
                .sum(),
            Value::Object(values) => {
                let matching = values.get("thread_id").and_then(Value::as_str) == Some(thread)
                    && values
                        .get("thread_generation_revision")
                        .and_then(Value::as_u64)
                        == Some(generation);
                if matching && values.contains_key("last_sequence") {
                    assert!(
                        values["last_sequence"].as_u64().unwrap() >= sequence,
                        "receipt digest regressed: {value}"
                    );
                }
                usize::from(matching && values.contains_key("last_sequence"))
                    + values
                        .values()
                        .map(|value| digests(value, thread, generation, sequence))
                        .sum::<usize>()
            }
            _ => 0,
        }
    }
    assert!(
        image
            .chunks
            .values()
            .map(|value| digests(value, thread, generation, sequence))
            .sum::<usize>()
            > 0
    );
    // History reads resolve aliases to a canonical conversation chunk;
    // pushed tips retain each roster agent's own identity.
    let history = image.histories.get(canonical).unwrap();
    assert!(
        history["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["data"]["delivery_status"] == "sent"),
        "missing committed canonical history receipt: {history}"
    );
    for agent in [canonical, alias] {
        let tips = image
            .facts
            .iter()
            .flat_map(|fact| &fact.threads)
            .filter(|tip| tip.agent_id == agent)
            .collect::<Vec<_>>();
        assert!(!tips.is_empty(), "no pushed thread for {agent}");
        for tip in tips {
            assert_eq!(
                tip.conversation_id.as_deref().unwrap_or(&tip.agent_id),
                canonical
            );
            assert_eq!(tip.thread_id.as_deref(), Some(thread));
            assert_eq!(tip.thread_generation_revision, Some(generation));
            assert!(tip.last_sequence >= sequence);
            assert!(
                tip.items
                    .iter()
                    .any(|item| item["data"]["delivery_status"] == "sent"),
                "pushed tail lost receipt for {agent}: {tip:?}"
            );
        }
    }
}

#[test]
fn receipt_patch_survives_unrelated_owner_and_task_publication() {
    let (_tmp, mut state) = fixture();
    let canonical = state.runs["run-00"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    let alias = state.runs["run-01"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    let mut rebound = state.take_run("run-01").unwrap();
    rebound
        .agents
        .by_id_mut(&alias)
        .unwrap()
        .bind_conversation(&canonical);
    state.finish_run_mutation("run-01".into(), rebound).unwrap();
    state.publish_all_snapshot_projection();
    let requests = requests(&state);
    let old = state.read_view.load();
    let before = image(&old, &requests);
    let committed = state
        .store
        .as_ref()
        .unwrap()
        .set_delivery_status(
            &canonical,
            None,
            1,
            1,
            crate::thread::MessageDeliveryStatus::Sent,
        )
        .unwrap();
    state.apply_committed_thread_items(&committed);
    let patched = state.read_view.load();
    let patched_before = image(&patched, &requests);
    assert_receipt(
        &patched_before,
        &canonical,
        &alias,
        &committed.thread_id,
        committed.generation_revision,
        committed.sequence,
    );

    assert_fast_eligible(&state, "run-02");
    let mut unrelated = state.take_run("run-02").unwrap();
    unrelated.run.goal = "unrelated publication after receipt".into();
    unrelated.agents.sole_thread_mut().post_agent(
        "unrelated news after receipt",
        None,
        LATER_CLOCK,
    );
    state
        .finish_run_mutation("run-02".into(), unrelated)
        .unwrap();
    let next = state.read_view.load();
    assert_receipt(
        &image(&next, &requests),
        &canonical,
        &alias,
        &committed.thread_id,
        committed.generation_revision,
        committed.sequence,
    );
    assert_full_reference(&state, &next, &requests);

    let project = state.projects.project_id_of("run-00").unwrap().to_owned();
    let task = state
        .tasks_create(&json!({"project_id":project, "title":"new task after receipt"}))
        .unwrap();
    assert_eq!(task["task"]["title"], "new task after receipt");
    let next = state.read_view.load();
    assert_receipt(
        &image(&next, &requests),
        &canonical,
        &alias,
        &committed.thread_id,
        committed.generation_revision,
        committed.sequence,
    );
    assert_full_reference(&state, &next, &requests);
    assert_eq!(before, image(&old, &requests));
    assert_eq!(patched_before, image(&patched, &requests));
}

#[test]
fn retained_projection_roots_do_not_retain_replaced_or_removed_sessions() {
    let (_tmp, mut state) = fixture();
    let root = state.runs["run-01"].worktree.path.clone();
    let agent = state.runs["run-01"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    let key = crate::app::TabKey::agent(&root, &agent);
    let tab = dictated_agent_tab(
        &root,
        "run-01",
        &agent,
        DictatedSession::reporting(crate::harness::AgentStatus::Waiting),
    );
    let weak = Arc::downgrade(&tab.session);
    let initial_strong = Arc::strong_count(&tab.session);
    state.session_registry.test_insert_tab(key.clone(), tab);
    state.publish_all_snapshot_projection();
    let old = state.read_view.load();
    let requests = requests(&state);
    let before = image(&old, &requests);
    assert_eq!(
        weak.strong_count(),
        initial_strong,
        "accepted layout retained a session resource"
    );

    let replacement = dictated_agent_tab(
        &root,
        "run-01",
        &agent,
        DictatedSession::reporting(crate::harness::AgentStatus::Waiting),
    );
    let replacement_weak = Arc::downgrade(&replacement.session);
    drop(
        state
            .session_registry
            .test_insert_tab(key.clone(), replacement)
            .unwrap(),
    );
    assert!(
        weak.upgrade().is_none(),
        "an old projection root kept a replaced session alive"
    );
    state.publish_owner_snapshot_projection("run-02");
    let replaced = state.read_view.load();
    assert_full_reference(&state, &replaced, &requests);
    let replaced_before = image(&replaced, &requests);
    drop(state.session_registry.test_remove_tab(&key).unwrap());
    assert!(
        replacement_weak.upgrade().is_none(),
        "accepted or retained roots kept a removed session alive"
    );
    let project = state.projects.project_id_of("run-00").unwrap().to_owned();
    let task = state
        .tasks_create(
            &json!({"project_id":project, "title":"task publication after registry removal"}),
        )
        .unwrap();
    assert_eq!(
        task["task"]["title"],
        "task publication after registry removal"
    );
    assert_full_reference(&state, &state.read_view.load(), &requests);
    state.publish_owner_snapshot_projection("run-02");
    assert_full_reference(&state, &state.read_view.load(), &requests);
    assert_eq!(before, image(&old, &requests));
    assert_eq!(replaced_before, image(&replaced, &requests));
}
