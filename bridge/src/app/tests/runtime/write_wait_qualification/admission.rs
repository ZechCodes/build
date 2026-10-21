//! Admission's real mutex acquisition remains attributable after the app guard.
use super::rpc::RpcHarness;
use crate::app::{AppState, DeliveryRunner};
use crate::timing::write_wait::WriteWaitRegistry;
use crate::update::admission_test_support as gate_test;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_rpc_gate_state_wait_keeps_the_actual_frame_origin() {
    let root = tempfile::tempdir().unwrap();
    let service = gate_test::service(root.path());
    let gate = service.admission();
    let mut arrivals = gate_test::observe(&gate);
    let config = root.path().join("settings.json");
    let app = AppState::new_unrooted(root.path(), "main", true, "unused")
        .with_config(config.clone())
        .unwrap()
        .with_update_service(service);
    let state = Arc::new(Mutex::new(app));
    let registry = WriteWaitRegistry::new();
    let rpc = Arc::new(RpcHarness::new(state, Arc::clone(&registry)));
    let mut held = gate_test::HeldGate::take(Arc::clone(&gate));
    let caller = Arc::clone(&rpc);
    let mut task = tokio::spawn(async move {
        caller
            .request(
                0,
                "gate-rpc-722",
                "settings.set",
                json!({"watch_agent_filed_tasks":true}),
            )
            .await
    });
    let arrival = tokio::time::timeout(gate_test::SAFETY, arrivals.recv()).await;
    let observed = gate_test::wait_for_admission(&registry, "settings.set").await;
    let released = held.release();
    let joined = tokio::time::timeout(gate_test::SAFETY, &mut task).await;
    let proof = match joined {
        Ok(result) => result.ok(),
        Err(_) => {
            task.abort();
            let _ = task.await;
            None
        }
    };
    let rpc = Arc::try_unwrap(rpc).ok().expect("request task was joined");
    rpc.finish().await;
    assert!(released && arrival.is_ok_and(|value| value.is_some()));
    let proof = proof.expect("released encrypted request completes");
    assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
    let saved: Value = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
    assert_eq!(saved["watch_agent_filed_tasks"], true);
    assert!(gate_test::idle(&gate));
    gate_test::assert_observed(observed, "gate-rpc-722");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_standalone_drain_gate_state_wait_keeps_its_supplied_origin() {
    let directory = tempfile::tempdir().unwrap();
    let service = gate_test::service(directory.path());
    let gate = service.admission();
    let mut arrivals = gate_test::observe(&gate);
    let app = AppState::new_unrooted(directory.path(), "main", true, "unused")
        .with_update_service(service);
    let clock = Arc::clone(&app.frame_clock);
    let state = Arc::new(Mutex::new(app));
    let registry = clock.capture_write_waits();
    let root = registry.begin("gate-drain-722", "user.present", Instant::now());
    root.admitted();
    let context = root.context();
    let mut held = gate_test::HeldGate::take(Arc::clone(&gate));
    let work = Arc::clone(&state);
    let mut task = tokio::task::spawn_blocking(move || {
        let timer = clock
            .frame("user.present")
            .with_write_context(Some(context));
        DeliveryRunner::drain(&work, &timer);
    });
    let arrival = tokio::time::timeout(gate_test::SAFETY, arrivals.recv()).await;
    let observed = gate_test::wait_for_admission(&registry, "user.present").await;
    let released = held.release();
    let joined = tokio::time::timeout(gate_test::SAFETY, &mut task).await;
    if joined.is_err() {
        let _ = task.await;
    }
    root.complete();
    assert!(released && arrival.is_ok_and(|value| value.is_some()));
    assert!(joined.is_ok_and(|result| result.is_ok()));
    assert!(state.lock().unwrap().delivery_queue.is_idle());
    assert!(gate_test::idle(&gate));
    gate_test::assert_observed(observed, "gate-drain-722");
}
