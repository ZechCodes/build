use super::*;
use crate::harness::installed::{Readings, NO_PROBE};
use crate::harness::inventory::{DeviceEnvironment, Inventory, RunningSessions};
use std::time::Instant;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repository) = init_repo();
    let home = directory.path().join("private-home");
    std::fs::create_dir(&home).unwrap();
    let inventory = Inventory::builder()
        .environment(move || DeviceEnvironment::new(home.clone(), [("PATH", "/nonexistent")]))
        .readings(Readings::answering_inline(&NO_PROBE))
        .running(RunningSessions::new())
        .build();
    let app =
        qa_state(&repository, directory.path()).with_harness_inventory(Arc::clone(&inventory));
    let mut fixture = RareFixture::new("harness-inventory", app);
    fixture.call("session.hello", json!({})).await;
    let before = fixture.call("harnesses.list", json!({})).await;
    let revision = before.reply["result"]["revision"].as_u64().unwrap();
    let before_completed = before.reply["result"]["refresh"]["completed"]
        .as_u64()
        .unwrap();
    let refreshed = fixture.call("harnesses.refresh", json!({})).await;
    let request = refreshed.reply["result"]["request"].as_u64().unwrap();
    assert!(request > before_completed);
    let retained_origin = fixture.origin_active(&refreshed).await;
    let started = Instant::now();
    tokio::task::spawn_blocking({
        let inventory = Arc::clone(&inventory);
        move || inventory.sweep()
    })
    .await
    .unwrap();
    let elapsed = started.elapsed();
    let after = fixture.call("harnesses.list", json!({})).await;
    assert!(after.reply["result"]["revision"].as_u64().unwrap() > revision);
    assert_eq!(after.reply["result"]["refresh"]["completed"], request);
    assert_eq!(after.reply["result"]["refresh"]["requested"], request);
    for row in after.reply["result"]["harnesses"].as_array().unwrap() {
        assert_eq!(row["installation"]["state"], "not_installed");
    }
    assert!(retained_origin, "refresh acknowledgement lost its pending inventory sweep origin");
    fixture.verify(refreshed, json!({"revision_before":revision,"revision_after":after.reply["result"]["revision"],"completed_request":request,"sweep_wall_ms":elapsed.as_secs_f64()*1000.0})).await;
    fixture.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_inventory_refresh_keeps_origin_until_real_sweep_completes() {
    let steps = execute().await;
    assert_eq!(steps.len(), 1);
}
