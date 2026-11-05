use super::*;

fn subscription(scope: Scope) -> SubscriptionSpec {
    SubscriptionSpec {
        id: "secrets".into(),
        scope,
        kinds: [Kind::SecretsChanged].into_iter().collect(),
        mode: Mode::Realtime,
        priority: Priority::Background,
    }
}

#[test]
fn project_secret_events_route_without_originating_conversation_facts() {
    let bus = ChangeBus::new(Duration::ZERO);
    let (project, mut project_rx, key) = SessionSender::observable("project");
    let (other, mut other_rx, _) = SessionSender::observable("other");
    bus.subscribe(&project, subscription(Scope::Entity("project-a".into())));
    bus.subscribe(
        &other,
        subscription(Scope::Entity("conversation-gone".into())),
    );
    bus.note_secret_change(json!({"type":"secrets.changed","scope":"project","project_id":"project-a","revision":3,"secret_ids":["sec_synthetic"]}));
    assert_eq!(bus.flush(), 1);
    assert!(other_rx.try_recv().is_err());
    let frame = SessionSender::decrypt_push(&key, &project_rx.try_recv().unwrap());
    assert_eq!(frame["project_id"], "project-a");
    assert_eq!(frame["revision"], 3);
    assert_eq!(frame["secret_ids"], json!(["sec_synthetic"]));
    assert!(frame.get("name").is_none());
}

#[test]
fn reordered_events_coalesce_ids_and_never_regress_revision() {
    let bus = ChangeBus::new(Duration::ZERO);
    let (sender, mut rx, key) = SessionSender::observable("reader");
    bus.subscribe(&sender, subscription(Scope::All));
    for (revision, id) in [(9, "sec_new"), (2, "sec_old"), (9, "sec_new")] {
        bus.note_secret_change(json!({"type":"secrets.changed","scope":"project","project_id":"project-a","revision":revision,"secret_ids":[id]}));
    }
    assert_eq!(bus.flush(), 1);
    let frame = SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(frame["revision"], 9);
    assert_eq!(frame["secret_ids"], json!(["sec_new", "sec_old"]));
    assert!(rx.try_recv().is_err());
}

#[test]
fn subscription_replacement_requeues_union_and_kind_removal_fences_inflight_events() {
    let bus = ChangeBus::new(Duration::ZERO);
    let (sender, mut rx, key) = SessionSender::observable("requeue");
    let mut spec = subscription(Scope::All);
    bus.subscribe(&sender, spec.clone());
    bus.note_secret_change(json!({"type":"secrets.changed","scope":"project","project_id":"project-a","revision":1,"secret_ids":["sec_old"]}));
    let due = bus.take_due();
    spec.kinds.insert(Kind::State);
    bus.subscribe(&sender, spec.clone());
    bus.note_secret_change(json!({"type":"secrets.changed","scope":"project","project_id":"project-a","revision":2,"secret_ids":["sec_new"]}));
    assert_eq!(bus.deliver(due, Vec::new()), 0);
    assert_eq!(bus.flush(), 1);
    let frame = SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(frame["secret_ids"], json!(["sec_new", "sec_old"]));
    assert_eq!(frame["revision"], 2);
    bus.note_secret_change(json!({"type":"secrets.changed","scope":"project","project_id":"project-a","revision":3,"secret_ids":["sec_last"]}));
    let due = bus.take_due();
    spec.kinds = [Kind::State].into_iter().collect();
    bus.subscribe(&sender, spec);
    assert_eq!(bus.deliver(due, Vec::new()), 0);
    assert_eq!(bus.flush(), 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn secret_ids_are_bounded_and_scope_invalidation_survives_other_kind_overflow() {
    let bus = ChangeBus::new(Duration::ZERO);
    let (sender, mut rx, key) = SessionSender::observable("bounded");
    let mut spec = subscription(Scope::All);
    spec.kinds.insert(Kind::State);
    bus.subscribe(&sender, spec);
    for index in 0..101 {
        bus.note_secret_change(json!({"type":"secrets.changed","scope":"project","project_id":"project-a","revision":index+1,"secret_ids":[format!("sec_{index}")]}));
    }
    for index in 0..=PENDING_KEY_CAP {
        bus.note_kind(&format!("entity-{index}"), Kind::State);
    }
    bus.flush();
    let mut found = false;
    while let Ok(message) = rx.try_recv() {
        let frame = SessionSender::decrypt_push(&key, &message);
        if frame["type"] == SECRETS_CHANGED_EVENT {
            found = true;
            assert_eq!(frame["secret_ids"], json!([]));
            assert_eq!(frame["truncated"], true);
        }
    }
    assert!(found);
}

#[test]
fn scope_overflow_becomes_explicit_device_refresh_without_losing_new_changes() {
    let bus = ChangeBus::new(Duration::ZERO);
    let (sender, mut rx, key) = SessionSender::observable("scope-overflow");
    bus.subscribe(&sender, subscription(Scope::All));
    for index in 0..=crate::secrets::MAX_RETAINED_BLOBS {
        bus.note_secret_change(json!({"type":"secrets.changed","scope":"project","project_id":format!("project-{index}"),"revision":index+1,"secret_ids":["sec_synthetic"]}));
    }
    assert_eq!(bus.flush(), 1);
    let frame = SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(frame["scope"], "device");
    assert_eq!(frame["truncated"], true);
    assert_eq!(frame["secret_ids"], json!([]));
    assert_eq!(frame["revision"], 1001);
    assert!(rx.try_recv().is_err());
}

#[test]
fn secret_event_and_kind_follow_the_metadata_capability_gate() {
    for name in [SECRETS_CHANGED_EVENT, Kind::SecretsChanged.as_str()] {
        assert!(!event_is_advertised(name, &[]));
        assert!(!event_is_advertised(name, &["secrets.attachments"]));
        assert!(event_is_advertised(name, &["secrets.metadataEvents"]));
    }
    assert!(event_is_advertised(COMPACTION_CHANGED_EVENT, &[]));
}
