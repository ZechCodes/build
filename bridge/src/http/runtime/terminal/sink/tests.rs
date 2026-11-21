use serde_json::json;

use crate::execution::session::{ClientSessions, Opening, StreamClass, VerifiedCaller};
use crate::http::limits::MAX_TERMINAL_CLIENT_BYTES;

use super::super::tests::fixture;
use super::*;

#[test]
fn payload_class_matches_every_fixture_message_case() {
    for case in fixture()["limits"]["message_cases"].as_array().unwrap() {
        let size = case["bytes"].as_u64().unwrap() as usize;
        let accepted = case["accepted"].as_bool().unwrap();
        let reply_to = case["reply_to"]
            .as_str()
            .map(|method| Method::parse(method).unwrap());
        let shape = json!({
            "type": case["push"], "ok": case["ok"],
            "data": "", "result": {"snapshot": ""}
        });
        let snapshot = snapshot_data_bytes(&shape, reply_to).map(|_| size - 128);
        let class = payload_class(size, snapshot);
        // The inbound rows use the listener's distinct message cap.
        let expected = match case["budget"].as_str().unwrap() {
            "client" => {
                assert_eq!(size <= MAX_TERMINAL_CLIENT_BYTES, accepted);
                Some(Class::Ordinary)
            }
            "ordinary" => accepted.then_some(Class::Ordinary),
            "snapshot" => accepted.then_some(Class::Snapshot),
            _ => panic!("unexpected message budget {case:?}"),
        };
        assert_eq!(class, expected, "{}", case["name"]);
    }
}

#[test]
fn snapshot_metadata_cap_is_inclusive() {
    let limits = &fixture()["limits"];
    let payload = limits["message_bytes"]["ordinary"].as_u64().unwrap() as usize;
    let metadata = limits["snapshot_bound"]["json_metadata_bytes"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(
        payload_class(payload + metadata, Some(payload)),
        Some(Class::Snapshot)
    );
    assert_eq!(payload_class(payload + metadata + 1, Some(payload)), None);
}

#[test]
fn queued_attach_is_replaced_with_a_correlated_refusal() {
    let sessions = ClientSessions::new(crate::bridge_epoch::current());
    let session = sessions
        .open(VerifiedCaller::verified_relay("device", Opening::new()))
        .unwrap();
    let (socket, mut receive) = SocketSink::new();
    let binding = sessions
        .bind_events(&session, StreamClass::Terminal, socket.clone())
        .unwrap();

    // Queue both through shared reply leases before the writer claims either.
    let _requests: Vec<_> = ["older", "newer"]
        .into_iter()
        .map(|id| {
            socket
                .interests
                .lock()
                .unwrap()
                .begin(id.into(), Method::Attach, None)
                .unwrap();
            let request = sessions
                .request_for_stream(
                    &session,
                    StreamClass::Terminal,
                    socket.reply(id.into(), Method::Attach),
                    Some(&binding),
                )
                .unwrap();
            request
                .reply()
                .send(json!({"id":id, "ok":true, "result":{"term_id":"shell","snapshot":""}}))
                .unwrap();
            request
        })
        .collect();

    let older = receive.try_recv().unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&older.bytes).unwrap()["ok"],
        true
    );
    assert!(older.fence.is_live());
    assert_superseded_reply(older);
    let newer = receive.try_recv().unwrap().into_write().unwrap();
    let reply: Value = serde_json::from_slice(&newer.bytes).unwrap();
    assert_eq!(reply["id"], "newer");
    assert_eq!(reply["ok"], true);
}

fn assert_superseded_reply(queued: Outbound) {
    let reply: Value = serde_json::from_slice(&queued.into_write().unwrap().bytes).unwrap();
    let busy: Value =
        serde_json::from_str(fixture()["messages"]["busy"]["utf8"].as_str().unwrap()).unwrap();
    assert_eq!(reply["id"], "older");
    assert_eq!(reply["bridge_epoch"], crate::bridge_epoch::current());
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error_code"], busy["error_code"]);
    assert_eq!(reply["retryable"], busy["retryable"]);
}

#[test]
fn full_interest_map_keeps_existing_attachment() {
    let mut interests = Interests::default();
    let cap = fixture()["limits"]["attachment_interests_per_socket"]
        .as_u64()
        .unwrap();
    for index in 0..cap {
        let id = format!("attach-{index}");
        interests.begin(id.clone(), Method::Attach, None).unwrap();
        interests.attach_reply(&id, Some(&format!("term-{index}")));
    }
    let original = interests.for_effect("term-0").unwrap();
    assert!(interests
        .begin("replacement".into(), Method::Attach, None)
        .is_none());
    assert!(Arc::ptr_eq(
        &original,
        &interests.for_effect("term-0").unwrap()
    ));
    assert!(original.is_live());
}
