//! `harnesses.list`, `harnesses.refresh` and the `harnesses.changed` push,
//! over the RPC a browser speaks (#434).

use super::*;
use crate::harness::installed::{Readings, NO_PROBE};
use crate::harness::inventory::{DeviceEnvironment, Inventory, RunningSessions};

async fn next_harnesses_changed(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: &str,
    within: Duration,
) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let outbound = tokio::time::timeout_at(deadline, rx.recv()).await.ok()??;
        let event = SessionSender::decrypt_push(key, &outbound);
        if event["type"] == "harnesses.changed" {
            return Some(event);
        }
    }
}

#[tokio::test]
async fn a_greeted_session_lists_refreshes_and_hears_the_inventory_move() {
    let (dir, repo) = init_repo();
    let device_home = tempfile::tempdir().unwrap();
    let home = device_home.path().to_path_buf();
    let inventory = Inventory::builder()
        .environment(move || DeviceEnvironment::new(home.clone(), [("PATH", "/nonexistent")]))
        .readings(Readings::answering_inline(&NO_PROBE))
        .running(RunningSessions::new())
        .build();
    let state = qa_state(&repo, dir.path())
        .with_harness_inventory(Arc::clone(&inventory))
        .shared();
    let handler = AppState::handler(state);
    let (session, mut rx, key) = SessionSender::observable("harnesses");

    let greeting = handler.call(session.clone(), req("session.hello", json!({})));
    let greeted = &greeting["result"];
    assert!(greeted["events"]
        .as_array()
        .unwrap()
        .contains(&json!("harnesses.changed")));
    for verb in ["harnesses.list", "harnesses.refresh"] {
        assert!(greeted["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!(verb)));
    }

    let listed = handler.call(session.clone(), req("harnesses.list", json!({})));
    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["result"]["harnesses"].as_array().unwrap().len(), 5);
    assert_eq!(
        listed["result"]["auth_contexts"].as_array().unwrap().len(),
        3
    );
    let revision = listed["result"]["revision"].as_u64().unwrap();

    let receipt = handler.call(session.clone(), req("harnesses.refresh", json!({})));
    assert_eq!(
        receipt["result"],
        json!({ "request": 1, "revision": revision })
    );
    let refused = handler.call(
        session.clone(),
        req("harnesses.refresh", json!({ "force": true })),
    );
    assert_eq!(refused["error_code"], "invalid_params", "{refused}");

    // The service sweeps; the session hears the new revision.
    std::thread::spawn({
        let inventory = Arc::clone(&inventory);
        move || inventory.sweep()
    });
    let news = next_harnesses_changed(&mut rx, &key, Duration::from_secs(10))
        .await
        .expect("a harnesses.changed push");
    let after = handler.call(session, req("harnesses.list", json!({})));
    assert_eq!(news["revision"], after["result"]["revision"]);
    assert!(after["result"]["revision"].as_u64().unwrap() > revision);
    assert_eq!(
        after["result"]["refresh"],
        json!({ "requested": 1, "completed": 1 })
    );
    assert_eq!(
        after["result"]["harnesses"][0]["installation"]["state"],
        "not_installed"
    );
}
