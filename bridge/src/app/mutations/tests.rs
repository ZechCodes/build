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
        self.store
            .writer()
            .admit_mutation(&MutationAdmission {
                principal: principal.into(),
                operation_id: id.into(),
                method: "test.command".into(),
                params: json!({}),
                scope: json!({"project_id":"project-test"}),
                owners: vec![MutationOwner {
                    kind: "project".into(),
                    id: "project-test".into(),
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
