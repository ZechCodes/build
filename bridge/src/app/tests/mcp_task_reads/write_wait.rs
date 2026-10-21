//! The complete MCP frame is an origin before it waits for either writer turn.
use super::*;
use crate::timing::write_wait::{WriteSnapshot, WAIT_BUDGET};

const FAULT: Duration = Duration::from_secs(6);
const SAFETY: Duration = Duration::from_secs(20);

fn assert_active_fault(row: Option<&WriteSnapshot>, fired: bool) {
    let row = row.expect("complete MCP write must be registered before its Turn wait");
    assert!(row.original_id.contains("mcp-write-722"), "{row:?}");
    assert!(
        !row.root_finished,
        "no final reply while both Turns are held"
    );
    assert!(!row.outstanding.is_empty());
    assert!(row.blocked_or_sql_union > WAIT_BUDGET, "{row:?}");
    assert!(
        row.breached(),
        "active wait must fail without timer completion"
    );
    assert!(
        fired,
        "independent watchdog must report the unfinished write"
    );
}

async fn wait_for_frame(phases: &crate::timing::phase_samples::PhaseSamples) -> bool {
    tokio::time::timeout(WATCHDOG, async {
        while phases.active_frames() == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_mcp_six_second_turn_wait_is_visible_while_active() {
    let fixture = Fixture::new();
    let clock = Arc::clone(&fixture.state.lock().unwrap().frame_clock);
    let registry = clock.capture_write_waits();
    let held = fixture.held_turns().await;
    let phases = clock.capture_phases();
    let fired = Arc::new(Mutex::new(Vec::new()));
    let observe = Arc::clone(&fired);
    let watchdog = registry.watchdog(
        Duration::from_millis(20),
        Arc::new(move |row| observe.lock().unwrap().push(row)),
    );
    let mut frame = fixture.frame(BridgeAction::SetTopic {
        topic: "logical wait publication".into(),
    });
    frame["id"] = json!("mcp-write-722");
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let mut server = tokio::spawn(handle_done_stream(
        Arc::clone(&fixture.state),
        daemon,
        Arc::clone(&fixture.plane),
    ));
    let mut client = tokio::io::BufReader::new(client);
    client
        .get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    let reached_wait = wait_for_frame(&phases).await;
    tokio::time::sleep(FAULT).await;
    let active = registry.active_snapshot();
    let live = active
        .iter()
        .find(|row| row.method == "mcp.control:set_topic");
    let live_fired = fired
        .lock()
        .unwrap()
        .iter()
        .any(|row| row.method == "mcp.control:set_topic");
    drop(held);
    let mut response = String::new();
    let received = tokio::time::timeout(SAFETY, client.read_line(&mut response)).await;
    drop(client);
    let joined = match tokio::time::timeout(SAFETY, &mut server).await {
        Ok(result) => result.is_ok(),
        Err(_) => {
            server.abort();
            let _ = server.await;
            false
        }
    };
    drop(watchdog);

    assert!(reached_wait, "real complete frame reached writer admission");
    assert!(received.is_ok_and(|result| result.is_ok_and(|count| count > 0)));
    assert!(joined, "owned socket task must be joined before assertions");
    let reply: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        reply["ok"], true,
        "valid set_topic finishes after release: {reply}"
    );
    assert_active_fault(live, live_fired);
}
