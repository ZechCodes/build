//! An off-lock flush may consume early notes before the final status snapshot.
use super::*;
use crate::app::read_view::SnapshotDelta;
use crate::changes::{ChangeBus, Kind, Scope, SubscriptionSpec};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Weak};
use std::time::Duration;

const LATER: &str = "2030-01-01T00:02:00Z";
const OTHER: &str = "run-status-notice-other";

struct Subscriber {
    bus: Arc<ChangeBus>,
    incoming: tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: String,
}

impl Subscriber {
    fn attach(fixture: &Fixture) -> Self {
        let bus = {
            let mut app = fixture.state.lock().unwrap();
            assert!(app
                .facts_handle
                .set(crate::app::facts::FactsSource::new(
                    app.read_view.clone(),
                    Weak::new(),
                ))
                .is_ok());
            app.changes = crate::app::facts::bus_with_sources(
                Duration::ZERO,
                &app.watchers,
                &app.facts_handle,
            );
            app.changes()
        };
        let (sender, incoming, key) = SessionSender::observable("status-notice-subscriber");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                id: "status-notices".into(),
                scope: Scope::All,
                kinds: [Kind::State, Kind::Thread].into_iter().collect(),
                mode: Default::default(),
                priority: Default::default(),
            },
        );
        let mut subscriber = Self { bus, incoming, key };
        subscriber.bus.note_board();
        subscriber.bus.note_entity(OWNER);
        subscriber.consume_before_publication(START);
        subscriber
    }

    fn flush(&mut self) -> Vec<Value> {
        self.bus.flush();
        let mut items = Vec::new();
        while let Ok(envelope) = self.incoming.try_recv() {
            let payload = SessionSender::decrypt_push(&self.key, &envelope);
            assert_eq!(payload["type"], "changes", "{payload}");
            items.extend(payload["items"].as_array().unwrap().iter().cloned());
        }
        items
    }

    fn consume_before_publication(&mut self, at: &str) {
        let items = self.flush();
        assert!(
            items
                .iter()
                .any(|item| item["entity_id"] == OWNER && item["state"]["last_activity"] == at),
            "the flush never consumed the old published owner: {items:?}"
        );
        assert!(!self.bus.has_pending(), "early notes remain pending");
    }
}

fn fixture() -> Fixture {
    let mut fixture = Fixture::passive();
    {
        let mut app = fixture.state.lock().unwrap();
        let owner = app.take_run(OWNER).unwrap();
        app.finish_run_mutation(OWNER.into(), owner).unwrap();
        app.delivery_queue.clear_queued();
    }
    // No live interval means an unrelated incremental owner need not refresh
    // this owner for an elapsed-time field.
    fixture.observe(&worked(START));
    fixture
}

fn worked(at: &str) -> SessionStatusSnapshot {
    let mut status = snapshot(AgentStatus::Waiting, at);
    status.last_worked_at = Some(at.into());
    status
}

fn state(fixture: &Fixture) -> Value {
    let handle = fixture.state.lock().unwrap().facts_handle.clone();
    let mut facts = crate::app::facts::entity_facts(
        &handle,
        &[crate::changes::FactsRequest {
            entity_id: OWNER.into(),
            state: true,
            ..Default::default()
        }],
    );
    facts.remove(0).state.unwrap()
}

fn assert_fresh(items: &[Value], at: &str) {
    let owner = items
        .iter()
        .find(|item| item["entity_id"] == OWNER && item["state"]["last_activity"] == at)
        .unwrap_or_else(|| panic!("no fresh owner notice for {at}: {items:?}"));
    assert!(owner["thread"].is_array(), "{owner}");
    assert!(items
        .iter()
        .any(|item| item["entity_id"] == crate::changes::BOARD_ITEM_ID));
}

fn stage(fixture: &Fixture, at: &str) {
    let mut app = fixture.state.lock().unwrap();
    assert!(app.record_agent_status_snapshot_for_pump(OWNER, &fixture.agent, &worked(at)));
}

fn prepare(fixture: &Fixture) -> SnapshotDelta {
    fixture
        .state
        .lock()
        .unwrap()
        .prepare_owner_snapshot_projection(OWNER)
        .unwrap()
}

fn publish(fixture: &Fixture, delta: SnapshotDelta) {
    fixture
        .state
        .lock()
        .unwrap()
        .publish_prepared_snapshot_projection(delta);
}

fn retry(fixture: &Fixture) {
    fixture
        .state
        .lock()
        .unwrap()
        .publish_owner_snapshot_projection(OWNER);
}

fn observer_notice_survives_early_flush(attention_save_fails: bool) {
    let mut fixture = fixture();
    let mut subscriber = Subscriber::attach(&fixture);
    let old = fixture.state.lock().unwrap().read_view.load();
    let old_image = image(&old);
    assert_eq!(state(&fixture)["last_activity"], START);
    if attention_save_fails {
        fixture.sql("CREATE TRIGGER reject_status_notice_attention BEFORE INSERT ON attention BEGIN SELECT RAISE(ABORT, 'injected attention failure'); END;");
    }
    let (arrived, entered) = mpsc::channel();
    let (release, released) = mpsc::channel();
    fixture.pump.before_final_publication(move || {
        arrived.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let observer = std::thread::spawn(move || {
        let phases = fixture.observe(&worked(STOP));
        (fixture, phases)
    });
    let reached = entered.recv_timeout(Duration::from_secs(5));
    // Flush on this thread while the observer holds the app mutex. The real
    // immutable facts source must neither acquire that mutex nor refresh it.
    let early = subscriber.flush();
    let consumed = !subscriber.bus.has_pending();
    let _ = release.send(());
    let (fixture, phases) = observer.join().unwrap();
    reached.unwrap();
    assert!(
        early
            .iter()
            .any(|item| item["entity_id"] == OWNER && item["state"]["last_activity"] == START),
        "the pre-final flush did not consume an old owner payload: {early:?}"
    );
    assert!(consumed, "pre-final notes remained pending");
    for item in early.iter().filter(|item| item["entity_id"] == OWNER) {
        assert_eq!(item["state"]["last_activity"], START);
    }
    assert_eq!(state(&fixture)["last_activity"], STOP);
    assert_eq!(image(&old), old_image);
    assert_eq!(count(&phases, "finisher.snapshot"), 0);
    assert_eq!(count(&phases, "finisher.attention"), 1);
    assert_eq!(count(&phases, "publication.install"), 1);
    // No unrelated note or second publication may rescue this delivery.
    assert_fresh(&subscriber.flush(), STOP);
    fixture.assert_full_oracle();
}

#[test]
fn status_final_publication_owes_fresh_notice_after_pre_final_flush() {
    observer_notice_survives_early_flush(false);
}

#[test]
fn status_attention_save_failure_still_owes_the_observed_final_notice() {
    observer_notice_survives_early_flush(true);
}

#[test]
fn rejected_snapshot_preserves_the_notice_for_an_accepted_retry() {
    let fixture = fixture();
    let mut subscriber = Subscriber::attach(&fixture);
    stage(&fixture, STOP);
    subscriber.consume_before_publication(START);
    let candidate = prepare(&fixture);
    let advanced = {
        let app = fixture.state.lock().unwrap();
        let previous = app.read_view.load();
        assert!(app.read_view.publish_delta(
            SnapshotDelta::new(app.read_view.next_version())
                .set_global(previous.global.clone().unwrap())
        ));
        app.read_view.load()
    };
    publish(&fixture, candidate);
    assert!(Arc::ptr_eq(
        &advanced,
        &fixture.state.lock().unwrap().read_view.load()
    ));
    assert_eq!(state(&fixture)["last_activity"], START);
    assert!(
        subscriber.flush().is_empty(),
        "rejected candidate emitted a notice"
    );
    retry(&fixture);
    assert_eq!(state(&fixture)["last_activity"], STOP);
    assert_fresh(&subscriber.flush(), STOP);
}

#[test]
fn borrowed_owner_refusal_keeps_the_notice_until_the_owner_is_restored() {
    let fixture = fixture();
    let mut subscriber = Subscriber::attach(&fixture);
    stage(&fixture, STOP);
    subscriber.consume_before_publication(START);
    let retained = fixture.state.lock().unwrap().read_view.load();
    {
        let mut app = fixture.state.lock().unwrap();
        let owner = app.take_run(OWNER).unwrap();
        assert!(app.prepare_owner_snapshot_projection(OWNER).is_none());
        assert!(Arc::ptr_eq(&retained, &app.read_view.load()));
        app.runs.insert(OWNER.into(), owner);
    }
    assert!(subscriber.flush().is_empty());
    retry(&fixture);
    assert_fresh(&subscriber.flush(), STOP);
}

#[test]
fn older_accepted_candidate_cannot_acknowledge_a_newer_attention_notice() {
    let fixture = fixture();
    let mut subscriber = Subscriber::attach(&fixture);
    stage(&fixture, STOP);
    subscriber.consume_before_publication(START);
    let candidate = prepare(&fixture);
    stage(&fixture, LATER);
    subscriber.consume_before_publication(START);
    publish(&fixture, candidate);
    assert_eq!(state(&fixture)["last_activity"], STOP);
    subscriber.flush();
    retry(&fixture);
    assert_eq!(state(&fixture)["last_activity"], LATER);
    assert_fresh(&subscriber.flush(), LATER);
}

#[test]
fn unrelated_incremental_candidate_cannot_consume_another_owners_notice() {
    let fixture = fixture();
    {
        let mut app = fixture.state.lock().unwrap();
        // Registered agents are deliberate incidental refresh dependencies.
        // Remove that dependency so this test proves genuinely narrow coverage.
        app.session_registry.test_remove_tab(&fixture.key).unwrap();
        let repo = app.project_at(0).repo_path.clone();
        insert_run(
            &mut app,
            &repo,
            fixture._directory.path(),
            OTHER,
            RunState::Building,
        );
        app.seed_anchors_for_records_without_one();
        app.publish_all_snapshot_projection();
    }
    let mut subscriber = Subscriber::attach(&fixture);
    stage(&fixture, STOP);
    subscriber.consume_before_publication(START);
    let candidate = {
        let app = fixture.state.lock().unwrap();
        let _ownership = app.begin_snapshot_ownership();
        let previous = app.read_view.load();
        app.prepare_incremental_owner_snapshot(
            SnapshotDelta::new(app.read_view.next_version()),
            &previous,
            &[OTHER.to_string()].into_iter().collect(),
        )
        .expect("the unrelated, inactive owner must take the incremental path")
    };
    publish(&fixture, candidate);
    assert_eq!(state(&fixture)["last_activity"], START);
    assert!(!subscriber
        .flush()
        .iter()
        .any(|item| item["entity_id"] == OWNER));
    retry(&fixture);
    assert_eq!(state(&fixture)["last_activity"], STOP);
    assert_fresh(&subscriber.flush(), STOP);
}

#[test]
fn full_candidate_announces_accepted_absence_of_a_removed_owed_owner() {
    let fixture = fixture();
    let mut subscriber = Subscriber::attach(&fixture);
    stage(&fixture, STOP);
    subscriber.consume_before_publication(START);
    let candidate = {
        let mut app = fixture.state.lock().unwrap();
        app.runs.remove(OWNER).unwrap();
        // Permanent departure retires the committed checkpoint; a borrowed
        // owner intentionally blocks the complete publication instead.
        app.committed_entities.forget(OWNER);
        app.session_registry.test_remove_tab(&fixture.key).unwrap();
        app.prepare_all_snapshot_projection().unwrap()
    };
    publish(&fixture, candidate);
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .read_view
        .load()
        .get_owned("agent.list", OWNER)
        .is_none());
    let items = subscriber.flush();
    assert!(
        items
            .iter()
            .any(|item| item["entity_id"] == crate::changes::BOARD_ITEM_ID
                && item["state"]["removed"]
                    .as_array()
                    .is_some_and(|removed| removed.contains(&json!(OWNER)))),
        "accepted owner absence was never announced: {items:?}"
    );
    assert!(items
        .iter()
        .any(|item| item["entity_id"] == crate::changes::BOARD_ITEM_ID));
}

#[test]
fn stale_status_instance_neither_reaches_final_publication_nor_notifies() {
    let mut fixture = fixture();
    let mut subscriber = Subscriber::attach(&fixture);
    let entered = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&entered);
    fixture.pump.before_final_publication(move || {
        observed.store(true, Ordering::SeqCst);
    });
    let retained = {
        let mut app = fixture.state.lock().unwrap();
        app.session_registry
            .test_tab_mut(&fixture.key)
            .unwrap()
            .session_instance
            .as_mut()
            .unwrap()
            .agent_id = "replacement-status-notice".into();
        app.read_view.load()
    };
    assert!(!fixture.pump.observe(&worked(STOP)));
    assert!(!entered.load(Ordering::SeqCst));
    assert!(Arc::ptr_eq(
        &retained,
        &fixture.state.lock().unwrap().read_view.load()
    ));
    assert!(subscriber.flush().is_empty());
}
