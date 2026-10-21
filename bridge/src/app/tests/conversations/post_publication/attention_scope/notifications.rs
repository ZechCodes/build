//! A consumed early invalidation cannot replace the accepted attention push.
use super::*;

struct Subscriber {
    bus: Arc<crate::changes::ChangeBus>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: String,
}

impl Subscriber {
    fn new(fixture: &Fixture, owners: &[&str]) -> Self {
        let bus = {
            let mut app = fixture.state.lock().unwrap();
            assert!(app
                .facts_handle
                .set(crate::app::facts::FactsSource::new(
                    app.read_view.clone(),
                    std::sync::Weak::new(),
                ))
                .is_ok());
            app.changes = crate::app::facts::bus_with_sources(
                Duration::ZERO,
                &app.watchers,
                &app.facts_handle,
            );
            app.changes()
        };
        let (session, receiver, key) = SessionSender::observable("rpc-attention-subscriber");
        for (id, scope) in std::iter::once(("board".to_owned(), crate::changes::Scope::All)).chain(
            owners.iter().map(|owner| {
                (
                    format!("entity-{owner}"),
                    crate::changes::Scope::Entity((*owner).into()),
                )
            }),
        ) {
            bus.subscribe(
                &session,
                crate::changes::SubscriptionSpec {
                    id,
                    scope,
                    kinds: [crate::changes::Kind::State].into_iter().collect(),
                    mode: Default::default(),
                    priority: Default::default(),
                },
            );
        }
        let mut reader = Self { bus, receiver, key };
        // Establish every subscriber's baseline before the tested mutation.
        reader.bus.note_board();
        for owner in owners {
            reader.bus.note_entity(owner);
        }
        assert!(!reader.flush_on_worker().is_empty());
        assert!(!reader.bus.has_pending());
        reader
    }

    fn flush_on_worker(&mut self) -> Vec<Value> {
        let bus = Arc::clone(&self.bus);
        // Encryption/facts lookup run on a worker holding no app guard. The
        // test may deliberately retain the guard while this worker flushes.
        std::thread::spawn(move || bus.flush()).join().unwrap();
        self.drain()
    }

    fn consume_before_final_publication(&mut self, app: &AppState, owner: &str) -> Value {
        let bus = Arc::clone(&self.bus);
        let facts = Arc::clone(&app.facts_handle);
        let owner = owner.to_owned();
        let old_attention = std::thread::spawn(move || {
            bus.flush();
            // An early board invalidation is a refetch hint, not an owner
            // state item. Read its immutable facts on this same off-lock
            // worker, exactly where a subscriber's immediate refetch lands.
            crate::app::facts::entity_facts(
                &facts,
                &[FactsRequest {
                    entity_id: owner,
                    state: true,
                    ..Default::default()
                }],
            )[0]
            .state
            .as_ref()
            .unwrap()["attention"]
                .clone()
        })
        .join()
        .unwrap();
        let early = self.drain();
        assert!(
            early.iter().any(|event| {
                event["type"] == "changes"
                    && event["items"].as_array().is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item["entity_id"] == crate::changes::BOARD_ITEM_ID)
                    })
            }),
            "the actual early board invalidation must be consumed: {early:?}"
        );
        assert!(
            !self.bus.has_pending(),
            "the early notes were actually consumed"
        );
        assert!(
            old_attention.is_object(),
            "immutable owner attention exists"
        );
        old_attention
    }

    fn drain(&mut self) -> Vec<Value> {
        let mut events = Vec::new();
        while let Ok(envelope) = self.receiver.try_recv() {
            events.push(SessionSender::decrypt_push(&self.key, &envelope));
        }
        events
    }
}

fn pushed_attention(events: &[Value], subscription: &str, owner: &str) -> Option<Value> {
    pushed_state(events, subscription, owner).and_then(|state| state.get("attention").cloned())
}

fn pushed_state(events: &[Value], subscription: &str, owner: &str) -> Option<Value> {
    events
        .iter()
        .filter(|event| event["type"] == "changes" && event["subscription_id"] == subscription)
        .flat_map(|event| event["items"].as_array().into_iter().flatten())
        .filter(|item| item["entity_id"] == owner)
        .filter_map(|item| item.get("state").cloned())
        .next_back()
}

fn require_final_attention(reader: &mut Subscriber, expected: &[(String, Value)]) -> Vec<Value> {
    // No manual invalidation here: only the accepted producer may wake this
    // otherwise idle subscriber after the early note has been consumed.
    let events = reader.flush_on_worker();
    for (owner, attention) in expected {
        for subscription in ["board".to_owned(), format!("entity-{owner}")] {
            assert_eq!(
                pushed_attention(&events, &subscription, owner),
                Some(attention.clone()),
                "accepted attention must notify {subscription} after an early flush: {events:?}"
            );
        }
    }
    assert!(!reader.bus.has_pending());
    events
}

#[test]
fn rpc_post_notifies_accepted_attention_after_the_early_note_was_consumed() {
    let fixture = Fixture::new();
    let mut reader = Subscriber::new(&fixture, &[&fixture.owner]);
    let params = fixture.params("accepted attention after early flush");
    let mut expected = Vec::new();
    fixture.measure(|app| {
        app.with_rpc_post_attention("thread.post", |app| {
            let (answer, job) = app.dispatch_deferring("thread.post", &params);
            assert!(answer.is_ok(), "{answer:?}");
            assert!(job.is_none());
            let current = app.attention_json(&fixture.owner);
            let before = reader.consume_before_final_publication(app, &fixture.owner);
            assert_ne!(
                before, current,
                "early facts must precede the final interaction stamp"
            );
            assert_eq!(before["interacted"], false);
            assert_eq!(current["interacted"], true);
            assert!(
                !reader.bus.has_pending(),
                "the early notes were actually consumed"
            );
            expected.push((fixture.owner.clone(), current));
        });
    });
    let _ = require_final_attention(&mut reader, &expected);
    fixture.assert_full_oracle();
}

#[test]
fn rpc_alias_post_notifies_each_accepted_owner_after_the_early_union_was_consumed() {
    let fixture = Fixture::new();
    let alias = fixture.bind_alias();
    let (other, _) = fixture.add_other_alias();
    let mut reader = Subscriber::new(&fixture, &[&fixture.owner, &fixture.peer, &other]);
    let before_other = fixture
        .state
        .lock()
        .unwrap()
        .entity_state_item(&other)
        .unwrap();
    let params = json!({"entity_id":fixture.peer,"agent_id":alias,"conversation_id":fixture.agent,"body":"alias accepted after early flush"});
    let mut expected = Vec::new();
    fixture.measure(|app| {
        app.with_rpc_post_attention("thread.post", |app| {
            let (answer, job) = app.dispatch_deferring("thread.post", &params);
            assert!(answer.is_ok(), "{answer:?}");
            assert!(job.is_none());
            let before = reader.consume_before_final_publication(app, &fixture.peer);
            assert_ne!(before, app.attention_json(&fixture.peer));
            assert!(!reader.bus.has_pending());
            expected.extend(
                [&fixture.owner, &fixture.peer]
                    .map(|owner| (owner.clone(), app.attention_json(owner))),
            );
        });
    });
    let final_other = {
        let app = fixture.state.lock().unwrap();
        crate::app::facts::entity_facts(
            &app.facts_handle,
            &[FactsRequest {
                entity_id: other.clone(),
                state: true,
                ..Default::default()
            }],
        )[0]
        .state
        .clone()
        .unwrap()
    };
    assert_ne!(
        before_other, final_other,
        "the independent alias row must actually advance"
    );
    let events = require_final_attention(&mut reader, &expected);
    assert_eq!(
        pushed_state(&events, &format!("entity-{other}"), &other),
        Some(final_other),
        "the independent shared-conversation alias must receive its changed row: {events:?}"
    );
    fixture.assert_full_oracle();
}

#[test]
fn rpc_partial_alias_error_still_notifies_the_committed_attention_after_early_flush() {
    let fixture = Fixture::new();
    let alias = fixture.bind_alias();
    fixture.sql(&format!("CREATE TRIGGER reject_notified_alias BEFORE INSERT ON implementations WHEN NEW.id = '{}' BEGIN SELECT RAISE(ABORT, 'injected addressed failure'); END;", fixture.peer.replace('\'', "''")));
    let mut reader = Subscriber::new(&fixture, &[&fixture.owner, &fixture.peer]);
    let params = json!({"entity_id":fixture.peer,"agent_id":alias,"conversation_id":fixture.agent,"body":"partial accepted attention after early flush"});
    let mut expected = Vec::new();
    fixture.measure(|app| {
        app.with_rpc_post_attention("thread.post", |app| {
            let (answer, job) = app.dispatch_deferring("thread.post", &params);
            assert!(answer.is_err(), "{answer:?}");
            assert!(job.is_none());
            let before = reader.consume_before_final_publication(app, &fixture.owner);
            assert_ne!(before, app.attention_json(&fixture.owner));
            assert!(!reader.bus.has_pending());
            assert_eq!(app.attention_json(&fixture.peer)["interacted"], false);
            expected.extend(
                [&fixture.owner, &fixture.peer]
                    .map(|owner| (owner.clone(), app.attention_json(owner))),
            );
        });
    });
    assert_eq!(
        fixture.message_count("partial accepted attention after early flush"),
        1
    );
    let _ = require_final_attention(&mut reader, &expected);
    fixture.assert_full_oracle();
}
