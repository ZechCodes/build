//! The real MCP writer carries its origin through update admission.
use super::*;
use crate::update::admission_test_support as gate_test;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_mcp_gate_state_wait_keeps_the_actual_socket_origin() {
    let mut fixture = Fixture::new();
    let service = gate_test::service(fixture._root.path());
    let gate = service.admission();
    let mut arrivals = gate_test::observe(&gate);
    let clock = {
        let mut app = fixture.state.lock().unwrap();
        app.updates = Some(service);
        Arc::clone(&app.frame_clock)
    };
    // The socket must use handles captured after its service is installed.
    fixture.plane = ControlPlane::new(&fixture.state.lock().unwrap());
    let registry = clock.capture_write_waits();
    let mut frame = fixture.frame(BridgeAction::SetTopic {
        topic: "real admission gate origin".into(),
    });
    frame["id"] = json!("gate-mcp-722");
    let mut held = gate_test::HeldGate::take(Arc::clone(&gate));
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let mut server = tokio::spawn(handle_done_stream(
        Arc::clone(&fixture.state),
        daemon,
        Arc::clone(&fixture.plane),
    ));
    let mut client = tokio::io::BufReader::new(client);
    let written = client
        .get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .await;
    let arrival = tokio::time::timeout(gate_test::SAFETY, arrivals.recv()).await;
    let observed = gate_test::wait_for_admission(&registry, "mcp.control:set_topic").await;
    let released = held.release();
    let mut response = String::new();
    let received = tokio::time::timeout(gate_test::SAFETY, client.read_line(&mut response)).await;
    drop(client);
    let joined = match tokio::time::timeout(gate_test::SAFETY, &mut server).await {
        Ok(result) => result.is_ok(),
        Err(_) => {
            server.abort();
            let _ = server.await;
            false
        }
    };
    assert!(written.is_ok() && released && arrival.is_ok_and(|value| value.is_some()));
    assert!(received.is_ok_and(|result| result.is_ok_and(|count| count > 0)));
    assert!(joined, "owned socket server joined after gate release");
    let reply: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(gate_test::idle(&gate));
    gate_test::assert_observed(observed, "gate-mcp-722");
}
