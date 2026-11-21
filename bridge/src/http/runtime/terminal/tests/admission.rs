use super::*;

#[tokio::test]
async fn mounted_tls_origin_vectors_precede_ticket_spend() {
    let server = Server::start(fake_handler()).await;
    let opening = server.opening().await;
    let ticket = server.ticket(&opening).await;
    for case in fixture()["handshake"]["invalid_requests"]
        .as_array()
        .unwrap()
    {
        if case["header"] != "origin" {
            continue;
        }
        let mut headers = server.handshake(&ticket);
        headers.remove(header::ORIGIN);
        for origin in case["values"].as_array().unwrap() {
            headers.append(header::ORIGIN, case_text(origin).parse().unwrap());
        }
        let (head, mut client) = server.upgrade(&headers).await;
        assert_eq!(
            status(&head),
            case["status"].as_u64().unwrap(),
            "{}",
            case["name"]
        );
        let body = read_json_body(&mut client, &head).await;
        assert_eq!(body["error_code"], case["refusal"]);
        assert_eq!(body["retryable"], false);
        assert!(body.get("request_id").is_some());
        assert!(!body.to_string().contains(&ticket));
        if case["name"] == "no_origin" || case["name"] == "duplicate_origin" {
            headers.remove(header::SEC_WEBSOCKET_KEY);
            assert_eq!(status(&server.upgrade(&headers).await.0), 403);
        }
    }
    let mut malformed = server.handshake(&ticket);
    malformed.remove(header::SEC_WEBSOCKET_KEY);
    assert_eq!(status(&server.upgrade(&malformed).await.0), 400);
    let mut socket = server.socket(&ticket).await;
    assert_eq!(
        status(&server.upgrade(&server.handshake(&ticket)).await.0),
        401
    );
    assert_eq!(
        call(&mut socket, "ping", "ping", json!({})).await["ok"],
        true
    );
    server.stop().await;
    assert_closed(&mut socket, None).await;
}

#[tokio::test]
async fn mounted_tls_family_budget_is_shared_with_issue_and_redemption() {
    let server = Server::start(fake_handler()).await;
    let cap = fixture()["limits"]["sockets"]["held_family"]
        .as_u64()
        .unwrap() as usize;
    let extra = server.opening().await;
    let pending = server.ticket(&extra).await;
    let mut held = Vec::new();
    let refill = fixture()["limits"]["tickets"]["issue_refill_interval_ms"]
        .as_u64()
        .unwrap();
    for index in 0..cap {
        if index + 1 == cap {
            tokio::time::sleep(Duration::from_millis(refill)).await;
        }
        let opening = server.opening().await;
        let ticket = server.ticket(&opening).await;
        held.push(server.socket(&ticket).await);
    }
    // A replenished issue bucket can no longer mask a broken listener quota.
    tokio::time::sleep(Duration::from_millis(refill)).await;
    assert_eq!(server.issue(&extra).await.0, 429);
    let headers = server.handshake(&pending);
    assert_eq!(status(&server.upgrade(&headers).await.0), 429);
    drop(held);
    // The capacity refusal leaves the pending ticket unspent.
    timeout(TIMEOUT, async {
        loop {
            let (head, _) = server.upgrade(&headers).await;
            match status(&head) {
                101 => return,
                429 => tokio::task::yield_now().await,
                other => panic!("unexpected redemption refusal {other}"),
            }
        }
    })
    .await
    .unwrap();
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_idle_reader_wakes_at_revocation_or_expiry() {
    for revoke in [true, false] {
        let lifetime = if revoke {
            TIMEOUT * 6
        } else {
            Duration::from_secs(2)
        };
        let server = Server::start_with(fake_handler(), lifetime).await;
        let opening = server.opening().await;
        let (application, _) = application_binding(&server, &opening);
        let mut socket = server.socket(&server.ticket(&opening).await).await;
        if revoke {
            server.verifier.revoked.store(true, Ordering::Release);
            server.verifier.changed.notify_waiters();
        }
        timeout(Duration::from_secs(4), assert_closed(&mut socket, None))
            .await
            .unwrap();
        assert!(
            application.is_live(),
            "the terminal proof ended before the Opening"
        );
        server.stop().await;
    }
}

#[tokio::test]
async fn mounted_tls_idle_reader_wakes_when_opening_closes() {
    let server = Server::start(fake_handler()).await;
    let opening = server.opening().await;
    let (application, _) = application_binding(&server, &opening);
    let mut socket = open_idle_socket(&server, &opening).await;
    let session = case_text(&opening["session_id"]);
    let (code, _) = server
        .json(
            "DELETE",
            &format!("/v1/sessions/{session}"),
            "opening",
            session,
            json!({}),
        )
        .await;
    assert_eq!(code, 204);
    assert!(!application.is_live());
    timeout(Duration::from_secs(2), assert_closed(&mut socket, None))
        .await
        .expect("Opening close must wake the idle socket before its 60s deadline");
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_idle_reader_wakes_when_renewal_retires_proof() {
    let server = Server::start(fake_handler()).await;
    let opening = server.opening().await;
    let (application, _) = application_binding(&server, &opening);
    let mut socket = open_idle_socket(&server, &opening).await;
    let session = case_text(&opening["session_id"]);
    // The Opening credential has identical identity and scopes but a distinct
    // digest from the credential that issued the terminal proof.
    let (code, _) = server
        .json(
            "PATCH",
            &format!("/v1/sessions/{session}"),
            "opening",
            session,
            json!({}),
        )
        .await;
    assert_eq!(code, 200);
    assert!(application.is_live());
    timeout(Duration::from_secs(2), assert_closed(&mut socket, None))
        .await
        .expect("retired proof must wake the idle socket before its 60s deadline");
    server.stop().await;
}

async fn open_idle_socket(server: &Server, opening: &Value) -> Client {
    let mut socket = server.socket(&server.ticket(opening).await).await;
    assert_eq!(
        call(&mut socket, "ping", "ping", json!({})).await["ok"],
        true
    );
    socket
}
