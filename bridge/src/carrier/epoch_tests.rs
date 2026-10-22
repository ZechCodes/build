use super::*;

#[test]
fn authenticated_frames_are_stamped_at_the_encryption_boundary() {
    let (sender, mut wire, key) = SessionSender::observable("epoch-boundary");
    let payloads = [
        json!({"id": 1, "accepted": true}),
        json!({"id": 1, "ok": true, "result": {}}),
        json!({"id": 2, "ok": false, "error": "refused"}),
        json!({"id": 3, "ok": true, "result": {"sdp": "answer"}}),
        json!({"type": "changes", "changes": []}),
        json!({"type": "term.output", "data": "bytes"}),
        json!({"type": "bridge.update_status", "state": "idle"}),
        json!({"type": "writes.confirmed", "operation_id": "op"}),
        json!({"type": "custom.push", "bridge_epoch": "producer-cannot-choose"}),
    ];
    for payload in payloads {
        assert!(sender.push(payload.clone()));
        let actual = SessionSender::decrypt_push(&key, &wire.try_recv().unwrap());
        let mut expected = payload;
        expected["bridge_epoch"] = json!(sender.bridge_epoch());
        assert_eq!(actual, expected);
    }
    assert!(uuid::Uuid::parse_str(sender.bridge_epoch()).is_ok());
    for invalid in [json!(null), json!([]), json!("not a frame")] {
        assert!(!sender.push(invalid), "a nonobject cannot carry an epoch");
    }
    assert!(wire.try_recv().is_err());
}

#[test]
fn sessions_and_replacement_carriers_share_the_process_epoch() {
    let (first, _, _) = SessionSender::observable("first");
    let (second, _, _) = SessionSender::observable("second");
    let detached = SessionSender::detached("detached");
    assert_eq!(first.bridge_epoch(), second.bridge_epoch());
    assert_eq!(first.bridge_epoch(), detached.bridge_epoch());
    let clone = first.clone();
    assert_eq!(first.bridge_epoch(), clone.bridge_epoch());

    let registry = SessionRegistry::default();
    let key = transport::generate_session_key();
    let (relay, _) = CarrierHandle::open();
    let (channel, _) = CarrierHandle::open();
    registry.open("migrating", key.clone(), &relay).unwrap();
    registry.open("migrating", key.clone(), &channel).unwrap();
    let frame = super::testing::client_request(
        &key,
        "migrating",
        transport::DATA_FRAME_TYPE,
        json!({"id": 1, "method": "rtc.ice", "params": {}}),
    );
    let (_, relay_sender) = registry.admit(&frame, &relay).unwrap();
    let (_, channel_sender) = registry.admit(&frame, &channel).unwrap();
    assert_eq!(relay_sender.bridge_epoch(), first.bridge_epoch());
    assert_eq!(channel_sender.bridge_epoch(), first.bridge_epoch());
}

#[test]
fn simulated_process_restart_rotates_epoch_without_relabeling_old_frames() {
    let (mut old, mut old_wire, old_key) = SessionSender::observable("reused-session");
    let (mut next, mut next_wire, next_key) = SessionSender::observable("reused-session");
    old.bridge_epoch = crate::bridge_epoch::new_boot();
    next.bridge_epoch = crate::bridge_epoch::new_boot();
    assert_ne!(old.bridge_epoch(), next.bridge_epoch());
    assert!(old.push(json!({"type": "term.output", "data": "old"})));
    assert!(next.push(json!({"type": "term.output", "data": "new"})));
    let old_frame = SessionSender::decrypt_push(&old_key, &old_wire.try_recv().unwrap());
    let next_frame = SessionSender::decrypt_push(&next_key, &next_wire.try_recv().unwrap());
    assert_eq!(old_frame["bridge_epoch"], old.bridge_epoch());
    assert_eq!(next_frame["bridge_epoch"], next.bridge_epoch());
    assert_ne!(old_frame["bridge_epoch"], next_frame["bridge_epoch"]);
}

#[test]
fn scoped_pushes_stop_at_the_original_carrier_and_opening() {
    let (sender, mut wire, key) = SessionSender::observable("scoped");
    assert!(sender.push_scoped(json!({"type": "writes.confirmed"})));
    let frame = SessionSender::decrypt_push(&key, &wire.try_recv().unwrap());
    assert_eq!(frame["bridge_epoch"], sender.bridge_epoch());
    sender.opening_ended();
    assert!(!sender.push_scoped(json!({"type": "writes.confirmed"})));
    assert!(wire.try_recv().is_err());

    let registry = SessionRegistry::default();
    let key = transport::generate_session_key();
    let (carrier, mut wire) = CarrierHandle::open();
    registry.open("released", key.clone(), &carrier).unwrap();
    let request = super::testing::client_request(
        &key,
        "released",
        transport::DATA_FRAME_TYPE,
        json!({"id": 1, "method": "rtc.ice", "params": {}}),
    );
    let (_, sender) = registry.admit(&request, &carrier).unwrap();
    sender.carrier_open.store(false, Ordering::SeqCst);
    assert!(!sender.push_scoped(json!({"type": "writes.confirmed"})));
    assert!(wire.try_recv().is_err());
}
