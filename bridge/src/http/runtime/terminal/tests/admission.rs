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
    let (status_code, refusal) = server.issue(&extra).await;
    assert_eq!(status_code, 429);
    assert_eq!(refusal["error_code"], "overloaded");
    assert_eq!(refusal["retryable"], true);
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

#[tokio::test]
async fn mounted_tls_restart_refuses_old_opening_and_unspent_ticket() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    // A child paused before exec can still hold a CLOEXEC listener descriptor.
    #[cfg(target_os = "linux")]
    let _inherited_listener = std::os::fd::AsFd::as_fd(&listener)
        .try_clone_to_owned()
        .unwrap();
    let first = Server::mount_on(fake_handler(), TIMEOUT * 6, true, listener).await;
    let port = first.port;
    let old = first.opening().await;
    let stale = first.ticket(&old).await;
    first.stop().await;

    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let fresh = Server::mount_on(fake_handler(), TIMEOUT * 6, true, listener).await;
    assert_eq!(fresh.issue(&old).await.0, 401);
    assert_eq!(
        status(&fresh.upgrade(&fresh.handshake(&stale)).await.0),
        401
    );
    let opening = fresh.opening().await;
    let mut replacement = open_idle_socket(&fresh, &opening).await;
    fresh.stop().await;
    assert_closed(&mut replacement, None).await;
}

#[tokio::test]
async fn mounted_tls_concurrent_redemption_has_exactly_one_winner() {
    let server = Server::start(fake_handler()).await;
    let opening = server.opening().await;
    let ticket = server.ticket(&opening).await;
    let headers = server.handshake(&ticket);
    let (first, second) = tokio::join!(server.upgrade(&headers), server.upgrade(&headers));
    let mut statuses = [status(&first.0), status(&second.0)];
    statuses.sort_unstable();
    assert_eq!(statuses, [101, 401]);
    drop((first, second));
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_ticket_refuses_a_changed_device_in_the_same_registry() {
    let server = Server::start(fake_handler()).await;
    let opening = server.opening().await;
    let session = case_text(&opening["session_id"]);
    let ticket = server.ticket(&opening).await;
    // Leave the registry, ticket, Opening, generation and origin intact.
    let device = {
        let mut openings = server.state.openings.lock().unwrap();
        let mounted = openings.get_mut(session).unwrap();
        std::mem::replace(&mut mounted.identity.device_id, "other-device".into())
    };
    let (head, mut connection) = server.upgrade(&server.handshake(&ticket)).await;
    assert_eq!(status(&head), 401);
    let refusal = read_json_body(&mut connection, &head).await;
    assert_eq!(refusal["error_code"], "invalid_credential");
    server
        .state
        .openings
        .lock()
        .unwrap()
        .get_mut(session)
        .unwrap()
        .identity
        .device_id = device;
    // Redeem the same ticket to prove it was present and the refusal did not spend it.
    let socket = server.socket(&ticket).await;
    drop(socket);
    server.stop().await;
}

#[tokio::test]
async fn mounted_tls_cookie_without_ticket_cannot_upgrade() {
    let server = Server::start(fake_handler()).await;
    let mut headers = server.handshake(&"A".repeat(43));
    let cookie = "session=qualification-cookie-sentinel";
    headers.insert(header::COOKIE, cookie.parse().unwrap());
    headers.insert(header::SEC_WEBSOCKET_PROTOCOL, PROTOCOL.parse().unwrap());
    let (head, mut connection) = server.upgrade(&headers).await;
    assert_eq!(status(&head), 401);
    let refusal = read_json_body(&mut connection, &head).await;
    assert_eq!(refusal["error_code"], "invalid_credential");
    assert!(!refusal.to_string().contains(cookie));
    server.stop().await;
}
