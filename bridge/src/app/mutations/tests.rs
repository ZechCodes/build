use super::*;
use crate::harness::HarnessContext;
use crate::store::{MutationAdmission, MutationOwner, Store};

struct Fixture {
    _dir: tempfile::TempDir,
    service: MutationService,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let context =
            HarnessContext::resolved(dir.path().join("mcp.sock"), dir.path().to_path_buf())
                .unwrap();
        let app =
            AppState::new_unrooted_configured(dir.path().join("worktrees"), "main", true, context)
                .with_task_store(dir.path().join("store"))
                .unwrap();
        app.initialize_read_context();
        let mut context = (*app.read_context.load_full().unwrap()).clone();
        context
            .mutations
            .owners
            .insert(("project".into(), "project-test".into()));
        app.read_context.store(Some(Arc::new(context)));
        let store = app.store.clone().unwrap();
        Self {
            _dir: dir,
            service: MutationService::new(&app),
            store,
        }
    }

    fn admit(&self, principal: &str, id: &str, body: &str) {
        self.admit_in_project(principal, id, body, "project-test");
    }

    fn admit_in_project(&self, principal: &str, id: &str, body: &str, project: &str) {
        self.store
            .writer()
            .admit_mutation(&MutationAdmission {
                principal: principal.into(),
                operation_id: id.into(),
                method: "test.command".into(),
                params: json!({}),
                scope: json!({"project_id":project}),
                owners: vec![MutationOwner {
                    kind: "project".into(),
                    id: project.into(),
                    generation: 0,
                }],
                resources: vec![],
                reservations: vec![],
                after: Some(vec![]),
                recovery_policy: "sql_only".into(),
                accepted_epoch: "original-boot".into(),
                acceptance: json!({"body": body}),
            })
            .unwrap();
    }

    async fn call(&self, sender: SessionSender, method: &str, params: Value) -> Value {
        let connection = self.store.read_pool().acquire().await;
        let frame = serde_json::from_value(json!({"session_id":sender.session_id(),"message_id":"test","frame_type":"data","sender":"client","created_at":"","payload":{"id":1,"method":method,"params":params}})).unwrap();
        self.service.call(sender, frame, Some(connection))
    }

    fn succeed(&self, id: &str, result: Value) {
        let writer = self.store.writer();
        assert!(writer.claim_mutation(DEVICE_USER, id).unwrap());
        writer
            .commit_mutation(DEVICE_USER, id, |_| Ok(result))
            .unwrap();
        writer.confirm_mutation(DEVICE_USER, id, json!({"published":[{"kind":"project","id":"project-test","generation":0,"sequence":1}],"refresh":[{"kind":"project","id":"project-test"}]})).unwrap();
    }
}

fn id(index: u128) -> String {
    uuid::Uuid::from_u128(index).to_string()
}

#[tokio::test]
async fn status_is_principal_scoped_and_current_authority_is_required() {
    let fixture = Fixture::new();
    fixture.admit(DEVICE_USER, &id(1), "own");
    fixture.admit("another_actor", &id(2), "secret");
    let (sender, _rx, _) = SessionSender::observable("session");
    let answer = fixture
        .call(
            sender.clone(),
            "writes.status",
            json!({"operation_ids":[id(1),id(2)]}),
        )
        .await;
    assert_eq!(
        answer["result"]["operations"][0]["state"], "accepted",
        "{answer}"
    );
    assert_eq!(
        answer["result"]["operations"][0]["ack"]["accepted_epoch"],
        "original-boot"
    );
    assert_eq!(answer["result"]["operations"][1]["state"], "not_found");
    assert!(!answer.to_string().contains("secret"));
    let mut context = (*fixture.service.inner.context.load_full().unwrap()).clone();
    context.mutations.owners.clear();
    fixture.service.inner.context.store(Some(Arc::new(context)));
    let answer = fixture
        .call(sender, "writes.status", json!({"operation_ids":[id(1)]}))
        .await;
    assert_eq!(
        answer["result"]["operations"][0]["state"], "not_found",
        "{answer}"
    );
}

#[tokio::test]
async fn committed_success_and_failed_outcomes_recover_without_confirm_delivery() {
    let fixture = Fixture::new();
    fixture.admit(DEVICE_USER, &id(1), "accepted");
    fixture.succeed(&id(1), json!({"saved":true}));
    fixture.admit(DEVICE_USER, &id(2), "will fail");
    fixture
        .store
        .writer()
        .fail_mutation(
            DEVICE_USER,
            &id(2),
            "unapplied",
            json!({"code":"stale_generation","message":"changed","retryable":false}),
        )
        .unwrap();
    let (sender, _rx, _) = SessionSender::observable("reconnect");
    let answer = fixture
        .call(
            sender,
            "writes.status",
            json!({"operation_ids":[id(2),id(1)]}),
        )
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["operations"][0]["state"], "failed");
    assert_eq!(answer["result"]["operations"][1]["result"]["saved"], true);
    assert_eq!(
        answer["result"]["operations"][1]["published"][0]["generation"],
        0
    );
}

#[tokio::test]
async fn status_pages_fit_budget_and_cursors_bind_the_ordered_operation_set() {
    let fixture = Fixture::new();
    let ids: Vec<_> = (1..=12).map(id).collect();
    for id in &ids {
        fixture.admit(DEVICE_USER, id, &"x".repeat(60 * 1024));
    }
    let (sender, _rx, _) = SessionSender::observable("pages");
    let first = fixture
        .call(
            sender.clone(),
            "writes.status",
            json!({"operation_ids":ids}),
        )
        .await;
    assert_eq!(first["ok"], true, "{first}");
    assert!(serde_json::to_vec(&first["result"]).unwrap().len() <= writes::MAX_STATUS_BYTES);
    let cursor = first["result"]["next_cursor"]
        .as_str()
        .expect("bounded page");
    let second = fixture
        .call(
            sender.clone(),
            "writes.status",
            json!({"operation_ids":ids,"cursor":cursor}),
        )
        .await;
    let count = first["result"]["operations"].as_array().unwrap().len();
    assert_eq!(
        second["result"]["operations"][0]["operation_id"],
        ids[count]
    );
    let repeated = fixture
        .call(
            sender.clone(),
            "writes.status",
            json!({"operation_ids":ids,"cursor":cursor}),
        )
        .await;
    assert_eq!(second, repeated);
    let changed = fixture
        .call(
            sender,
            "writes.status",
            json!({"operation_ids":[id(1)],"cursor":cursor}),
        )
        .await;
    assert_eq!(changed["error_code"], "invalid_params", "{changed}");
}

#[tokio::test]
async fn subscribe_replays_confirmation_committed_between_snapshot_and_registration() {
    let fixture = Fixture::new();
    fixture.admit(DEVICE_USER, &id(1), "accepted");
    let writer = fixture.store.writer();
    *fixture.service.inner.after_snapshot.lock().unwrap() = Some(Box::new(move || {
        assert!(writer.claim_mutation(DEVICE_USER, &id(1)).unwrap());
        writer
            .commit_mutation(DEVICE_USER, &id(1), |_| Ok(json!({"saved":true})))
            .unwrap();
        writer
            .confirm_mutation(DEVICE_USER, &id(1), json!({"published":[],"refresh":[]}))
            .unwrap();
    }));
    let (sender, mut rx, key) = SessionSender::observable("snapshot-race");
    let answer = fixture
        .call(
            sender.clone(),
            "writes.subscribe",
            json!({"operation_ids":[id(1)]}),
        )
        .await;
    assert_eq!(
        answer["result"]["operations"][0]["state"], "accepted",
        "{answer}"
    );
    let pushed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let pushed = SessionSender::decrypt_push(&key, &pushed);
    assert_eq!(pushed["type"], "writes.confirmed", "{pushed}");
    assert_eq!(pushed["result"]["saved"], true);
    let status = fixture
        .call(
            sender.clone(),
            "writes.status",
            json!({"operation_ids":[id(1)]}),
        )
        .await;
    assert_eq!(
        status["result"]["operations"][0]["revision"],
        pushed["revision"]
    );
    sender.opening_ended();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(fixture
        .service
        .inner
        .subscriptions
        .lock()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn released_carrier_cannot_register_after_its_snapshot_finishes() {
    let fixture = Fixture::new();
    fixture.admit(DEVICE_USER, &id(1), "accepted");
    let (sender, _rx, _) = SessionSender::observable("released");
    let (sender, release) = sender.scoped_for_test();
    *fixture.service.inner.after_snapshot.lock().unwrap() = Some(Box::new(release));
    let answer = fixture
        .call(sender, "writes.subscribe", json!({"operation_ids":[id(1)]}))
        .await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(fixture
        .service
        .inner
        .subscriptions
        .lock()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn older_snapshot_completion_cannot_replace_newer_subscription() {
    let fixture = Fixture::new();
    let (sender, _rx, _) = SessionSender::observable("reordered");
    let newer_sender = sender.clone();
    let service = fixture.service.clone();
    let pool = fixture.store.read_pool();
    *fixture.service.inner.after_snapshot.lock().unwrap() = Some(Box::new(move || {
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let connection = runtime.block_on(pool.acquire());
            let frame = serde_json::from_value(json!({"session_id":"reordered","message_id":"newer","frame_type":"data","sender":"client","created_at":"","payload":{"id":2,"method":"writes.subscribe","params":{"operation_ids":[id(2)]}}})).unwrap();
            let answer = service.call(newer_sender, frame, Some(connection));
            assert_eq!(answer["ok"], true, "{answer}");
        }).join().unwrap();
    }));
    let answer = fixture
        .call(sender, "writes.subscribe", json!({"operation_ids":[id(1)]}))
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    let subscriptions = fixture.service.inner.subscriptions.lock().unwrap();
    let subscribed = subscriptions.values().next().unwrap();
    assert!(subscribed.ids.contains(&id(2)));
    assert!(!subscribed.ids.contains(&id(1)));
}

#[tokio::test]
async fn replaced_subscription_cannot_deliver_an_old_set_after_waiting_for_sql() {
    let fixture = Fixture::new();
    fixture.admit(DEVICE_USER, &id(1), "old set");
    let (sender, mut rx, _) = SessionSender::observable("replacement");
    fixture
        .call(
            sender.clone(),
            "writes.subscribe",
            json!({"operation_ids":[id(1)]}),
        )
        .await;
    let old = fixture
        .service
        .inner
        .subscriptions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    fixture
        .call(sender, "writes.subscribe", json!({"operation_ids":[id(2)]}))
        .await;
    fixture.succeed(&id(1), json!({"saved":true}));
    replay(&fixture.service, old).await;
    assert!(
        rx.try_recv().is_err(),
        "a replaced set cannot emit its old operation"
    );
}

#[tokio::test]
async fn subscribe_pages_fit_budget_and_replay_retains_the_entire_operation_set() {
    let fixture = Fixture::new();
    let ids: Vec<_> = (1..=12).map(id).collect();
    for id in &ids {
        fixture.admit(DEVICE_USER, id, &"x".repeat(60 * 1024));
    }
    let (sender, mut rx, key) = SessionSender::observable("subscribe-pages");
    let answer = fixture
        .call(
            sender.clone(),
            "writes.subscribe",
            json!({"operation_ids":ids}),
        )
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    let encoded_bytes = serde_json::to_vec(&answer["result"]).unwrap().len();
    assert!(
        encoded_bytes <= writes::MAX_STATUS_BYTES,
        "subscribe snapshot encoded {encoded_bytes} bytes"
    );
    let first = answer["result"]["operations"].as_array().unwrap();
    assert!(
        first.len() < ids.len(),
        "the snapshot must exercise pagination"
    );
    let cursor = answer["result"]["next_cursor"]
        .as_str()
        .expect("bounded subscribe snapshot");
    let continuation = fixture
        .call(
            sender,
            "writes.status",
            json!({"operation_ids":ids,"cursor":cursor}),
        )
        .await;
    assert_eq!(continuation["ok"], true, "{continuation}");
    let resumed_ids: Vec<_> = first
        .iter()
        .chain(continuation["result"]["operations"].as_array().unwrap())
        .map(|operation| operation["operation_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        resumed_ids, ids,
        "subscribe cursor must neither skip nor duplicate operations"
    );
    let off_page_id = ids.last().unwrap();
    assert!(!first
        .iter()
        .any(|operation| operation["operation_id"] == off_page_id.as_str()));
    fixture.store.writer().fail_mutation(DEVICE_USER, off_page_id, "unapplied", json!({"code":"scope_ended","message":"off-page operation completed","retryable":false})).unwrap();
    let pushed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let pushed = SessionSender::decrypt_push(&key, &pushed);
    assert_eq!(pushed["type"], "writes.confirmed", "{pushed}");
    assert_eq!(
        pushed["operation_id"],
        off_page_id.as_str(),
        "pagination must retain subscriptions for operations outside the initial snapshot"
    );
}

#[tokio::test]
async fn subscribe_and_replay_preserve_principal_and_current_scope_authority() {
    let fixture = Fixture::new();
    let mut context = (*fixture.service.inner.context.load_full().unwrap()).clone();
    context
        .mutations
        .owners
        .insert(("project".into(), "project-retained".into()));
    fixture.service.inner.context.store(Some(Arc::new(context)));
    fixture.admit(DEVICE_USER, &id(1), "revoked scope private acceptance");
    fixture.admit_in_project(
        "another_actor",
        &id(2),
        "other principal private acceptance",
        "project-retained",
    );
    fixture.admit_in_project(
        DEVICE_USER,
        &id(3),
        "authorized sentinel",
        "project-retained",
    );
    let (sender, mut rx, key) = SessionSender::observable("subscribe-authority");
    let answer = fixture
        .call(
            sender,
            "writes.subscribe",
            json!({"operation_ids":[id(1),id(2),id(3)]}),
        )
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["operations"][0]["state"], "accepted");
    assert_eq!(answer["result"]["operations"][1]["state"], "not_found");
    assert_eq!(answer["result"]["operations"][2]["state"], "accepted");
    assert!(!answer
        .to_string()
        .contains("other principal private acceptance"));
    let mut context = (*fixture.service.inner.context.load_full().unwrap()).clone();
    context
        .mutations
        .owners
        .remove(&("project".into(), "project-test".into()));
    fixture.service.inner.context.store(Some(Arc::new(context)));
    let (after_revoke, _rx, _) = SessionSender::observable("after-revoke");
    let answer = fixture
        .call(
            after_revoke,
            "writes.subscribe",
            json!({"operation_ids":[id(1),id(2)]}),
        )
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert!(answer["result"]["operations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|operation| operation["state"] == "not_found"));
    let writer = fixture.store.writer();
    for (principal, operation, message) in [
        ("another_actor", id(2), "other principal private outcome"),
        (DEVICE_USER, id(1), "revoked scope private outcome"),
        (DEVICE_USER, id(3), "authorized sentinel outcome"),
    ] {
        writer
            .fail_mutation(
                principal,
                &operation,
                "unapplied",
                json!({"code":"scope_ended","message":message,"retryable":false}),
            )
            .unwrap();
    }
    let pushed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let pushed = SessionSender::decrypt_push(&key, &pushed);
    assert_eq!(pushed["operation_id"], id(3), "only the authorized sentinel may be emitted after replay passes both private outcomes: {pushed}");
    assert!(!pushed.to_string().contains("private outcome"));
    assert!(
        rx.try_recv().is_err(),
        "private operations must not follow the authorized sentinel"
    );
}

#[tokio::test]
async fn retired_application_binding_cannot_register_after_snapshot_with_live_reply() {
    use crate::execution::session::{
        ClientSessions, DeliveryError, EncodedEvent, EventSink, Opening, ReplySink, StreamClass,
        VerifiedCaller,
    };
    struct Sink;
    impl EventSink for Sink {
        fn try_send(&self, _: EncodedEvent) -> Result<(), DeliveryError> {
            Ok(())
        }
    }
    impl ReplySink for Sink {
        fn try_send(&self, _: EncodedEvent) -> Result<(), DeliveryError> {
            Ok(())
        }
    }
    let fixture = Fixture::new();
    fixture.admit(DEVICE_USER, &id(1), "old");
    fixture.admit(DEVICE_USER, &id(2), "current");
    let sessions = Arc::new(ClientSessions::new("application-binding-test"));
    let session = sessions
        .open(VerifiedCaller::verified_relay("device", Opening::new()))
        .unwrap();
    let first = sessions
        .bind_events(&session, StreamClass::Application, Arc::new(Sink))
        .unwrap();
    let request = sessions
        .request(&session, Arc::new(Sink), Some(&first))
        .unwrap();
    let old = SessionSender::from_request(&request);
    let observed = old.clone();
    let replacing = sessions.clone();
    let service = fixture.service.clone();
    let successor_connection = fixture.store.read_pool().acquire().await;
    *fixture.service.inner.after_snapshot.lock().unwrap() = Some(Box::new(move || {
        let second = replacing
            .replace_events(
                &session,
                StreamClass::Application,
                Some(&first),
                Arc::new(Sink),
            )
            .unwrap();
        let current_request = replacing
            .request(&session, Arc::new(Sink), Some(&second))
            .unwrap();
        let current = SessionSender::from_request(&current_request);
        let frame = serde_json::from_value(json!({
            "session_id": session.id(), "message_id": "successor", "frame_type": "data", "sender": "client", "created_at": "",
            "payload": {"id": 2, "method": "writes.subscribe", "params": {"operation_ids": [id(2)]}}
        })).unwrap();
        let answer = service.call(current, frame, Some(successor_connection));
        assert_eq!(answer["ok"], true, "{answer}");
        assert!(
            observed.read_is_live(),
            "independent request reply is still live"
        );
        assert!(
            !observed.events_are_live(),
            "captured application binding has retired"
        );
    }));
    let answer = fixture
        .call(old, "writes.subscribe", json!({"operation_ids":[id(1)]}))
        .await;
    assert_eq!(answer["ok"], false, "{answer}");
    let subscriptions = fixture.service.inner.subscriptions.lock().unwrap();
    assert_eq!(subscriptions.len(), 1);
    let current = subscriptions.values().next().unwrap();
    assert!(current.sender.events_are_live());
    assert!(current.ids.contains(&id(2)));
    assert!(!current.ids.contains(&id(1)));
}
