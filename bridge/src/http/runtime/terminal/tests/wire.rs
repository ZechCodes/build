use super::*;

#[tokio::test]
async fn mounted_tls_binary_envelopes_and_correlated_refusals_match_fixture() {
    let cases = fixture()["invalid_messages"].as_array().unwrap();
    let burst = fixture()["limits"]["tickets"]["issue_burst_per_family"]
        .as_u64()
        .unwrap() as usize;
    for group in cases.chunks(burst) {
        let server = Server::start(fake_handler()).await;
        let opening = server.opening().await;
        for case in group {
            if case.get("outstanding_ids").is_some() {
                continue;
            }
            check_message_case(&server, &opening, case).await;
        }
        server.stop().await;
    }
}

async fn check_message_case(server: &Server, opening: &Value, case: &Value) {
    let mut socket = server.socket(&server.ticket(opening).await).await;
    let message = match case["opcode"].as_u64().unwrap() {
        1 => Message::Text(case_text(&case["utf8"]).into()),
        2 if case["name"] == "invalid_utf8" => Message::Binary(vec![b'{', 0xff, b'}']),
        2 => Message::Binary(case_text(&case["utf8"]).as_bytes().to_vec()),
        _ => unreachable!(),
    };
    socket.send(message).await.unwrap();
    if let Some(code) = case["close_code"].as_u64() {
        assert_closed(&mut socket, Some(code as u16)).await;
    } else {
        let expected: Value = serde_json::from_str(case_text(
            &fixture()["messages"][case_text(&case["reply"])]["utf8"],
        ))
        .unwrap();
        let mut actual = object(&mut socket).await;
        actual["bridge_epoch"] = expected["bridge_epoch"].clone();
        assert_eq!(actual, expected, "{}", case["name"]);
        assert_eq!(
            call(&mut socket, "ping", "ping", json!({})).await["ok"],
            true
        );
    }
}

#[test]
fn fixture_dimensions_input_and_methods_are_checked_before_dispatch() {
    for case in fixture()["limits"]["grid"]["cases"].as_array().unwrap() {
        let request = json!({"id":"grid","method":case["method"],"params":case["params"]});
        let Incoming::Call(request) = Incoming::parse(&serde_json::to_vec(&request).unwrap())
            .ok()
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            request.decode().is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
    for case in fixture()["limits"]["decoded_input_cases"]
        .as_array()
        .unwrap()
    {
        let data = STANDARD.encode(vec![0; case["bytes"].as_u64().unwrap() as usize]);
        let request =
            json!({"id":"input","method":"term.input","params":{"term_id":"test","data":data}});
        let Incoming::Call(request) = Incoming::parse(&serde_json::to_vec(&request).unwrap())
            .ok()
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            request.decode().is_ok(),
            case["accepted"].as_bool().unwrap()
        );
    }
    for method in fixture()["wire"]["methods"].as_array().unwrap() {
        assert!(Method::parse(case_text(method)).is_ok());
    }
}

#[tokio::test]
async fn mounted_tls_enforces_aggregate_client_cap_with_1009() {
    let server = Server::start(fake_handler()).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    let cases = fixture()["limits"]["message_cases"].as_array().unwrap();
    let invalid = cases
        .iter()
        .find(|case| case["name"] == "client_aggregate_overflow")
        .unwrap();
    let fragments = invalid["fragments"].as_array().unwrap();
    for (index, size) in fragments.iter().enumerate() {
        let opcode = if index == 0 {
            Data::Binary
        } else {
            Data::Continue
        };
        socket
            .send(Message::Frame(Frame::message(
                vec![b' '; size.as_u64().unwrap() as usize],
                OpCode::Data(opcode),
                index + 1 == fragments.len(),
            )))
            .await
            .unwrap();
    }
    assert_closed(
        &mut socket,
        Some(invalid["close_code"].as_u64().unwrap() as u16),
    )
    .await;
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_fragments_snapshot_and_refuses_oversize_ordinary_reply() {
    let snapshot_size = 2 * 1024 * 1024;
    let handler = greeting(FrameHandler::new(
        FrameClock::new(),
        move |_, frame, _| match frame.payload["method"].as_str().unwrap() {
            "term.attach" => success(
                &frame,
                json!({"term_id":"shell","snapshot":"eA==".repeat(snapshot_size / 4)}),
            ),
            "term.create" => success(&frame, json!({"data":"x".repeat(1024 * 1024)})),
            other => panic!("unexpected {other}"),
        },
    ));
    let server = Server::start(handler).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    let reply = call(
        &mut socket,
        "attach",
        "term.attach",
        json!({"term_id":"shell"}),
    )
    .await;
    assert_eq!(case_text(&reply["result"]["snapshot"]).len(), snapshot_size);
    assert_eq!(
        call(&mut socket, "tail", "ping", json!({})).await["ok"],
        true
    );
    send(&mut socket, "over", "term.create", json!({})).await;
    assert_closed(&mut socket, Some(1009)).await;
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_invalid_grid_dimensions_never_reach_execution() {
    let server = Server::start(fake_handler()).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    for case in fixture()["limits"]["grid"]["cases"].as_array().unwrap() {
        if case["accepted"] == false {
            let result = call(
                &mut socket,
                case_text(&case["name"]),
                case_text(&case["method"]),
                case["params"].clone(),
            )
            .await;
            assert_eq!(
                result["error_code"], "invalid_params",
                "{case:?} {result:?}"
            );
            assert_eq!(result["error"], "Invalid terminal dimensions");
            assert_eq!(result["retryable"], false);
        }
    }
    assert_eq!(
        call(&mut socket, "ping", "ping", json!({})).await["ok"],
        true
    );
    server.stop().await;
}
