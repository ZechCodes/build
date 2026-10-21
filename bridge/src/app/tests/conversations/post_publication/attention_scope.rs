//! RPC attention coalescing must keep independent commits and both human stamps.
mod notifications;

use super::*;
use crate::app::read_view::{ReadView, ReadViewSlot, SnapshotDelta};
use crate::changes::{BoardLists, EntityFacts, FactsRequest};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    owner: String,
    agent: String,
    peer: String,
}

impl Fixture {
    fn new() -> Self {
        let (directory, repo) = init_repo();
        let mut app = qa_state(&repo, directory.path());
        let owner = adopted_run(&mut app, &repo, directory.path(), "attention-post");
        let peer = adopted_run(&mut app, &repo, directory.path(), "attention-peer");
        for id in [&owner, &peer] {
            let mut active = app.take_run(id).unwrap();
            active.run.state = RunState::Review;
            app.finish_run_mutation(id.clone(), active).unwrap();
        }
        app.seed_anchors_for_records_without_one();
        let mut entries: std::collections::HashMap<_, _> = app
            .board
            .attention()
            .persistence()
            .into_entries()
            .into_iter()
            .collect();
        for id in [&owner, &peer] {
            let attention = entries.entry(id.clone()).or_default();
            attention.last_interaction_at = None;
            attention.resume_at = None;
            attention.anchor_at = Some("2000-01-01T00:00:00Z".into());
            attention.last_user_message_at = attention.anchor_at.clone();
        }
        app.board.attention_mut().replace_entries(entries);
        app.persist_attention();
        app.delivery_queue.clear_queued();
        app.frame_clock = crate::timing::FrameClock::reporting_to(Arc::new(|_| {}));
        app.publish_all_snapshot_projection();
        let agent = primary_agent_id(&app, &owner);
        Self {
            _directory: directory,
            state: Arc::new(Mutex::new(app)),
            owner,
            agent,
            peer,
        }
    }

    fn params(&self, body: &str) -> Value {
        json!({"entity_id":self.owner,"agent_id":self.agent,"conversation_id":self.agent,"body":body})
    }

    fn measure<T>(&self, work: impl FnOnce(&mut AppState) -> T) -> (T, Value) {
        let clock = Arc::clone(&self.state.lock().unwrap().frame_clock);
        let samples = clock.capture_phases();
        let timer = clock.frame("thread.post");
        let result = work(&mut timer.lock(&self.state));
        drop(timer);
        (result, samples.snapshot()["thread.post"].clone())
    }

    fn dispatch(
        &self,
        params: &Value,
    ) -> (
        (
            Result<Value, crate::api::ApiError>,
            Option<crate::app::DeferredJob>,
        ),
        Value,
    ) {
        self.measure(|app| app.dispatch_deferring("thread.post", params))
    }

    fn attention(&self, owner: &str) -> crate::attention::Attention {
        self.state
            .lock()
            .unwrap()
            .board
            .attention()
            .attention(owner)
            .unwrap()
            .clone()
    }

    fn saved_attention(&self, owner: &str) -> crate::attention::Attention {
        self.state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .load_attention()[owner]
            .clone()
    }

    fn sql(&self, sql: &str) {
        self.state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .writer_for_test()
            .execute_batch(sql)
            .unwrap();
    }

    fn message_count(&self, body: &str) -> usize {
        self.state.lock().unwrap().store.as_ref().unwrap().thread_items(&self.agent).unwrap().iter()
            .filter(|item| matches!(item, crate::thread::ThreadItem::Message(message) if message.body == body)).count()
    }

    fn assert_full_oracle(&self) {
        let app = self.state.lock().unwrap();
        let retained = app.read_view.load();
        let actual = image(&retained, &app);
        app.publish_all_snapshot_projection();
        assert_eq!(actual, image(&app.read_view.load(), &app));
        assert_eq!(
            actual,
            image(&retained, &app),
            "full publication changed a retained root"
        );
    }

    fn bind_alias(&self) -> String {
        let mut app = self.state.lock().unwrap();
        let alias_agent = primary_agent_id(&app, &self.peer);
        let mut active = app.take_run(&self.peer).unwrap();
        active
            .agents
            .by_id_mut(&alias_agent)
            .unwrap()
            .bind_conversation(&self.agent);
        app.finish_run_mutation(self.peer.clone(), active).unwrap();
        app.publish_all_snapshot_projection();
        alias_agent
    }
    fn add_other_alias(&self) -> (String, String) {
        let mut app = self.state.lock().unwrap();
        let repo = app.project_at(0).repo_path.clone();
        let owner = adopted_run(
            &mut app,
            &repo,
            self._directory.path(),
            "attention-third-alias",
        );
        let agent = primary_agent_id(&app, &owner);
        let mut active = app.take_run(&owner).unwrap();
        active.run.state = RunState::Review;
        active
            .agents
            .by_id_mut(&agent)
            .unwrap()
            .bind_conversation(&self.agent);
        app.finish_run_mutation(owner.clone(), active).unwrap();
        app.delivery_queue.clear_queued();
        app.publish_all_snapshot_projection();
        (owner, agent)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Image {
    chunks: BTreeMap<(&'static str, String), Value>,
    histories: BTreeMap<String, Value>,
    facts: Vec<EntityFacts>,
}

fn image(root: &ReadView, app: &AppState) -> Image {
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
    let after = app
        .runs
        .values()
        .flat_map(|run| run.agents.iter())
        .map(|agent| (agent.id.clone(), 0))
        .collect::<Vec<_>>();
    let mut requests = app
        .runs
        .keys()
        .map(|owner| FactsRequest {
            entity_id: owner.clone(),
            state: true,
            thread: true,
            terminals: true,
            thread_after: after.clone(),
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
    requests.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));
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
        facts: crate::app::facts::entity_facts(&handle, &requests),
    }
}

fn count(sample: &Value, phase: &str) -> u64 {
    sample[phase]["count"].as_u64().unwrap_or(0)
}

fn assert_committed_post_delivery(app: &AppState, owner: &str) {
    assert!(app.user_session().last_activity_ms.is_some());
    assert_eq!(
        app.store.as_ref().unwrap().load_user_session().unwrap(),
        Some(app.user_session())
    );
    assert!(app.delivery_queue.queued().any(|turn| turn.owner == owner));
}

fn assert_reply_before_interaction(
    app: &AppState,
    owner: &str,
    result: &Value,
    prior: &crate::attention::Attention,
) {
    let current = app.run_view(
        owner,
        &app.runs[owner],
        crate::thread::ThreadDetail::Full,
        crate::app::DigestScope::Detail,
    );
    for field in [
        "state",
        "thread",
        "agents",
        "unread",
        "unread_count",
        "updated_at",
    ] {
        assert_eq!(result[field], current[field], "{field}");
    }
    // The route constructs its reply before the success-only interaction
    // stamp. Preserve that prefix while the final immutable root includes it.
    let created = app.board.attention().clock(owner).created_at.unwrap();
    let mut reply_attention = current["attention"].clone();
    reply_attention["interacted"] = json!(prior.last_interaction_at.is_some());
    reply_attention["resume_at"] = json!(prior.sort_key(&created));
    assert_eq!(result["attention"], reply_attention);
    assert_eq!(result["attention"]["interacted"], false);
    assert_eq!(current["attention"]["interacted"], true);
}

#[test]
fn rpc_post_preserves_two_attention_saves_with_one_final_attention_projection() {
    let fixture = Fixture::new();
    let old = fixture.state.lock().unwrap().read_view.load();
    let before = image(&old, &fixture.state.lock().unwrap());
    let prior = fixture.attention(&fixture.owner);
    let params = fixture.params("one scoped user post");
    let ((result, deferred), phases) = fixture.dispatch(&params);
    let result = result.unwrap();
    assert!(deferred.is_none());
    assert_eq!(fixture.message_count("one scoped user post"), 1);
    let attention = fixture.attention(&fixture.owner);
    assert_ne!(attention.last_user_message_at, prior.last_user_message_at);
    assert!(attention.last_interaction_at.is_some());
    assert_eq!(fixture.saved_attention(&fixture.owner), attention);
    let app = fixture.state.lock().unwrap();
    assert_committed_post_delivery(&app, &fixture.owner);
    assert_reply_before_interaction(&app, &fixture.owner, &result, &prior);
    assert_eq!(image(&old, &app), before);
    drop(app);
    fixture.assert_full_oracle();
    assert_eq!(count(&phases, "entered.finisher.save"), 1);
    assert_eq!(
        count(&phases, "finisher.attention"),
        2,
        "both attention writes remain independent"
    );
    assert_eq!(
        count(&phases, "publication.global"),
        2,
        "canonical append followed by one union attention projection"
    );
    assert_eq!(count(&phases, "publication.install"), 2);
}

#[test]
fn rpc_alias_post_flushes_addressed_canonical_and_other_alias_named_rows_together() {
    let fixture = Fixture::new();
    let alias_agent = fixture.bind_alias();
    let (other_owner, other_agent) = fixture.add_other_alias();
    let prior_other = fixture
        .state
        .lock()
        .unwrap()
        .read_view
        .load()
        .get_owned("agent.list", &other_owner)
        .unwrap();
    let params = json!({"entity_id":fixture.peer,"agent_id":alias_agent,"conversation_id":fixture.agent,"body":"alias union post"});
    let ((answer, deferred), phases) = fixture.dispatch(&params);
    assert!(answer.is_ok(), "{answer:?}");
    assert!(deferred.is_none());
    assert_eq!(fixture.message_count("alias union post"), 1);
    let app = fixture.state.lock().unwrap();
    let alias_row = app
        .read_view
        .load()
        .get_owned("agent.list", &other_owner)
        .unwrap();
    assert!(alias_row.to_string().contains(&other_agent));
    assert_ne!(
        alias_row, prior_other,
        "dependent alias named row must advance"
    );
    drop(app);
    for owner in [&fixture.owner, &fixture.peer] {
        assert_eq!(fixture.saved_attention(owner), fixture.attention(owner));
        assert!(fixture.attention(owner).last_user_message_at.is_some());
    }
    assert!(fixture
        .attention(&fixture.peer)
        .last_interaction_at
        .is_some());
    fixture.assert_full_oracle();
    assert_eq!(
        count(&phases, "entered.finisher.save"),
        2,
        "both independent canonical/addressed commits remain"
    );
    assert_eq!(count(&phases, "finisher.attention"), 3);
    assert_eq!(
        count(&phases, "publication.global"),
        3,
        "two roster commits plus one alias-union attention projection"
    );
}

#[test]
fn rpc_partial_alias_save_failure_flushes_committed_message_attention_without_interaction() {
    let fixture = Fixture::new();
    let alias_agent = fixture.bind_alias();
    let before_interaction = fixture.attention(&fixture.peer).last_interaction_at;
    let before_session = fixture.state.lock().unwrap().user_session();
    fixture.sql(&format!("CREATE TRIGGER reject_addressed_post BEFORE INSERT ON implementations WHEN NEW.id = '{}' BEGIN SELECT RAISE(ABORT, 'injected addressed failure'); END;",fixture.peer.replace('\'',"''")));
    let params = json!({"entity_id":fixture.peer,"agent_id":alias_agent,"conversation_id":fixture.agent,"body":"committed despite alias refusal"});
    let ((answer, deferred), _) = fixture.dispatch(&params);
    assert!(answer.is_err(), "{answer:?}");
    assert!(deferred.is_none());
    assert_eq!(fixture.message_count("committed despite alias refusal"), 1);
    assert_eq!(
        fixture.attention(&fixture.peer).last_interaction_at,
        before_interaction
    );
    let app = fixture.state.lock().unwrap();
    assert_eq!(app.user_session(), before_session);
    assert!(!app
        .delivery_queue
        .queued()
        .any(|turn| turn.owner == fixture.peer));
    drop(app);
    for owner in [&fixture.owner, &fixture.peer] {
        assert_eq!(fixture.saved_attention(owner), fixture.attention(owner));
    }
    fixture.assert_full_oracle();
    // A later ordinary attention API cannot inherit the ended RPC scope.
    let (_, phases) = fixture.measure(|app| app.touch_attention(&fixture.owner));
    assert_eq!(count(&phases, "publication.install"), 1);
}

#[test]
fn rpc_rejections_and_failed_append_do_not_stamp_or_publish_attention() {
    let fixture = Fixture::new();
    for failing_append in [false, true] {
        let old = fixture.state.lock().unwrap().read_view.load();
        let before = fixture.attention(&fixture.owner);
        let session = fixture.state.lock().unwrap().user_session();
        let params = if failing_append {
            fixture
                .state
                .lock()
                .unwrap()
                .store
                .as_ref()
                .unwrap()
                .fail_next_write();
            fixture.params("rejected append")
        } else {
            json!({"entity_id":"unknown-owner","body":"rejected append"})
        };
        let ((answer, deferred), phases) = fixture.dispatch(&params);
        assert!(answer.is_err());
        assert!(deferred.is_none());
        assert_eq!(fixture.message_count("rejected append"), 0);
        assert_eq!(fixture.attention(&fixture.owner), before);
        let app = fixture.state.lock().unwrap();
        assert_eq!(app.user_session(), session);
        assert!(Arc::ptr_eq(&old, &app.read_view.load()));
        assert_eq!(count(&phases, "finisher.attention"), 0);
        assert_eq!(count(&phases, "publication.install"), 0);
    }
}

#[test]
fn rpc_attention_save_failure_still_publishes_both_live_stamps_and_durable_message() {
    let fixture = Fixture::new();
    let before = fixture.saved_attention(&fixture.owner);
    fixture.sql("CREATE TRIGGER reject_post_attention BEFORE INSERT ON attention BEGIN SELECT RAISE(ABORT, 'injected attention failure'); END;");
    let ((answer, deferred), phases) = fixture.dispatch(&fixture.params("attention save refused"));
    assert!(answer.is_ok(), "{answer:?}");
    assert!(deferred.is_none());
    assert_eq!(fixture.message_count("attention save refused"), 1);
    assert_eq!(fixture.saved_attention(&fixture.owner), before);
    let current = fixture.attention(&fixture.owner);
    assert!(current.last_interaction_at.is_some());
    assert_ne!(current.last_user_message_at, before.last_user_message_at);
    fixture.assert_full_oracle();
    assert_eq!(count(&phases, "finisher.attention"), 2);
    assert_eq!(count(&phases, "publication.global"), 2);
}

#[test]
fn rpc_operation_replay_only_stamps_interaction_and_cannot_duplicate_delivery() {
    let fixture = Fixture::new();
    let mut params = fixture.params("operation replay post");
    params["operation_id"] = json!("post-attention-operation");
    assert!(fixture.dispatch(&params).0 .0.is_ok());
    let first_attention = fixture.attention(&fixture.owner);
    let receipt = fixture
        .state
        .lock()
        .unwrap()
        .operation_receipt("post-attention-operation")
        .unwrap()
        .unwrap();
    let ((answer, deferred), phases) = fixture.dispatch(&params);
    assert!(answer.is_ok());
    assert!(deferred.is_none());
    assert_eq!(fixture.message_count("operation replay post"), 1);
    assert_eq!(
        fixture.attention(&fixture.owner).last_user_message_at,
        first_attention.last_user_message_at
    );
    let app = fixture.state.lock().unwrap();
    assert_eq!(
        app.operation_receipt("post-attention-operation").unwrap(),
        Some(receipt)
    );
    assert_eq!(
        app.delivery_queue
            .queued()
            .filter(|turn| turn.owner == fixture.owner)
            .count(),
        1
    );
    drop(app);
    fixture.assert_full_oracle();
    assert_eq!(count(&phases, "entered.finisher.save"), 0);
    assert_eq!(count(&phases, "finisher.attention"), 1);
    assert_eq!(count(&phases, "publication.global"), 1);
}

#[test]
fn direct_and_agent_posts_keep_their_existing_immediate_attention_behavior() {
    let fixture = Fixture::new();
    let prior_session = fixture.state.lock().unwrap().user_session();
    let (answer, phases) =
        fixture.measure(|app| app.thread_post(&fixture.params("direct human post")));
    assert!(answer.is_ok());
    assert_eq!(fixture.message_count("direct human post"), 1);
    assert_eq!(fixture.attention(&fixture.owner).last_interaction_at, None);
    assert_eq!(count(&phases, "publication.global"), 2);
    assert_eq!(count(&phases, "finisher.attention"), 1);
    let before = fixture.attention(&fixture.owner);
    let params = fixture.params("agent-origin post");
    let requester = crate::operation::OperationRequester {
        entity_id: fixture.owner.clone(),
        agent_id: fixture.agent.clone(),
        conversation_id: fixture.agent.clone(),
    };
    let (answer, phases) = fixture.measure(|app| app.thread_post_from_agent(&params, requester));
    assert!(answer.is_ok(), "{answer:?}");
    assert_eq!(fixture.message_count("agent-origin post"), 1);
    assert_eq!(
        fixture.attention(&fixture.owner).last_user_message_at,
        before.last_user_message_at
    );
    assert_eq!(fixture.attention(&fixture.owner).last_interaction_at, None);
    assert_eq!(fixture.state.lock().unwrap().user_session(), prior_session);
    assert_eq!(count(&phases, "publication.global"), 1);
    assert_eq!(count(&phases, "finisher.attention"), 0);
    fixture.assert_full_oracle();
}

#[test]
fn deferred_dispatch_publishes_message_attention_before_returning_the_job() {
    // Current legacy plan posts are retired. This explicit work sentinel tests
    // the dispatcher's deferred-return lifetime, not a natural plan route.
    let fixture = Fixture::new();
    let ran = Arc::new(AtomicBool::new(false));
    let reply = Arc::new(Mutex::new(None::<Value>));
    let capture_reply = Arc::clone(&reply);
    let capture_ran = Arc::clone(&ran);
    let callback_state = Arc::downgrade(&fixture.state);
    fixture.state.lock().unwrap().deferred_work =
        Some(DeferredWork::External(Box::new(move || {
            assert!(
                callback_state.upgrade().unwrap().try_lock().is_ok(),
                "external work must run with the app guard released"
            );
            capture_ran.store(true, Ordering::SeqCst);
            Ok(capture_reply.lock().unwrap().take().unwrap())
        })));
    let params = fixture.params("before external job");
    let ((answer, deferred), phases) = fixture.dispatch(&params);
    let answer = answer.unwrap();
    let job = deferred.expect("real dispatch must hand out the sentinel job");
    assert!(!ran.load(Ordering::SeqCst));
    assert_eq!(fixture.attention(&fixture.owner).last_interaction_at, None);
    assert_eq!(
        fixture.saved_attention(&fixture.owner),
        fixture.attention(&fixture.owner)
    );
    fixture.assert_full_oracle();
    assert_eq!(count(&phases, "publication.global"), 2);
    *reply.lock().unwrap() = Some(answer);
    assert!(fixture.state.try_lock().is_ok());
    let done = job.run();
    assert!(ran.load(Ordering::SeqCst));
    let (answer, phases) = fixture.measure(|app| app.apply_deferred("thread.post", &params, done));
    assert!(answer.is_ok(), "{answer:?}");
    assert!(fixture
        .attention(&fixture.owner)
        .last_interaction_at
        .is_some());
    assert_eq!(count(&phases, "finisher.attention"), 1);
    assert_eq!(
        count(&phases, "publication.global"),
        1,
        "settlement is outside the original RPC scope"
    );
    fixture.assert_full_oracle();
}

#[test]
fn borrowed_peer_failure_keeps_the_existing_atomic_publication_barrier() {
    let fixture = Fixture::new();
    let old = fixture.state.lock().unwrap().read_view.load();
    let mut rejected = fixture
        .state
        .lock()
        .unwrap()
        .take_run(&fixture.peer)
        .unwrap();
    rejected.run.goal = "UNCOMMITTED BORROWED PEER".into();
    let ((answer, deferred), _) = fixture.dispatch(&fixture.params("accepted while peer borrowed"));
    assert!(answer.is_ok(), "{answer:?}");
    assert!(deferred.is_none());
    assert_eq!(fixture.message_count("accepted while peer borrowed"), 1);
    assert!(Arc::ptr_eq(
        &old,
        &fixture.state.lock().unwrap().read_view.load()
    ));
    {
        let mut app = fixture.state.lock().unwrap();
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app
            .finish_run_mutation(fixture.peer.clone(), rejected)
            .is_err());
        let rendered = format!("{:?}", image(&app.read_view.load(), &app));
        assert!(rendered.contains("accepted while peer borrowed"));
        assert!(!rendered.contains("UNCOMMITTED BORROWED PEER"));
    }
    fixture.assert_full_oracle();
}

#[test]
fn rpc_scope_unwind_keeps_dirty_receipts_and_restores_immediate_attention() {
    let fixture = Fixture::new();
    let old = fixture.state.lock().unwrap().read_view.load();
    let before = image(&old, &fixture.state.lock().unwrap());
    let (_, phases) = fixture.measure(|app| {
        let mut dirty = Vec::new();
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.with_rpc_post_attention("thread.post", |app| {
                app.note_user_message(&fixture.owner);
                dirty = app.snapshot_observations.observation_owners.snapshot();
                panic!("injected RPC scope unwind");
            });
        }));
        assert!(refused.is_err());
        assert!(app.rpc_post_attention_owners.is_none());
        assert!(Arc::ptr_eq(&old, &app.read_view.load()));
        assert!(dirty.iter().any(|(owner, _)| owner == &fixture.owner));
        assert_eq!(
            app.snapshot_observations.observation_owners.snapshot(),
            dirty
        );
        // A normal direct stamp is immediate again after the caught panic.
        app.touch_attention(&fixture.owner);
        assert!(!Arc::ptr_eq(&old, &app.read_view.load()));
    });
    assert_eq!(count(&phases, "finisher.attention"), 2);
    assert_eq!(count(&phases, "publication.global"), 1);
    assert_eq!(count(&phases, "publication.install"), 1);
    assert_eq!(
        fixture.saved_attention(&fixture.owner),
        fixture.attention(&fixture.owner)
    );
    assert_eq!(image(&old, &fixture.state.lock().unwrap()), before);
    fixture.assert_full_oracle();
}

#[test]
fn nested_rpc_scope_caught_inner_unwind_preserves_outer_union_until_safe_exit() {
    let fixture = Fixture::new();
    let old = fixture.state.lock().unwrap().read_view.load();
    let (_, phases) = fixture.measure(|app| {
        app.with_rpc_post_attention("thread.post", |app| {
            app.note_user_message(&fixture.owner);
            let mut dirty = Vec::new();
            let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                app.with_rpc_post_attention("thread.post", |app| {
                    app.note_user_message(&fixture.peer);
                    dirty = app.snapshot_observations.observation_owners.snapshot();
                    panic!("injected nested RPC scope unwind");
                });
            }));
            assert!(refused.is_err());
            assert!(Arc::ptr_eq(&old, &app.read_view.load()));
            assert_eq!(
                app.snapshot_observations.observation_owners.snapshot(),
                dirty
            );
            let pending = app.rpc_post_attention_owners.as_ref().unwrap();
            assert!(pending.contains(&fixture.owner));
            assert!(pending.contains(&fixture.peer));
            // A normal nested exit joins that same still-active outer scope.
            app.with_rpc_post_attention("thread.post", |app| app.touch_attention(&fixture.peer));
            assert!(Arc::ptr_eq(&old, &app.read_view.load()));
            app.touch_attention(&fixture.owner);
        });
        assert!(app.rpc_post_attention_owners.is_none());
    });
    assert_eq!(count(&phases, "finisher.attention"), 4);
    assert_eq!(count(&phases, "publication.global"), 1);
    assert_eq!(count(&phases, "publication.install"), 1);
    for owner in [&fixture.owner, &fixture.peer] {
        assert_eq!(fixture.saved_attention(owner), fixture.attention(owner));
        assert!(fixture.attention(owner).last_interaction_at.is_some());
    }
    fixture.assert_full_oracle();
}
