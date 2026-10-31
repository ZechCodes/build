//! Mounted SSE subscriptions: the bridge half of the desktop/SPA event stream.
use super::*;
use futures_util::StreamExt;

type Frames =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, axum::Error>> + Send>>;

fn pushing_execution() -> Arc<ExecutionService> {
    ExecutionService::new(
        FrameHandler::new(FrameClock::new(), |context, frame, _| {
            let events = context.has_events();
            if events {
                context.push(json!({"type": "probe.pushed"}));
            }
            json!({"id": frame.payload["id"], "ok": true, "result": {"events": events}})
        })
        .with_greeting_reads(
            |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
        ),
    )
}

fn verifier(authority: VerifiedAuthority) -> Arc<dyn GrantVerifier> {
    Arc::new(WitnessVerifier {
        calls: Arc::new(AtomicUsize::new(0)),
        authority: Some(authority),
    })
}

async fn opened(app: &Router) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn lifecycle(method: &str, uri: &str, session_id: &str, body: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(AUTHORIZATION, "Bearer account-bound-token")
        .header("X-Build-Session", session_id);
    if body.is_some() {
        request = request.header(CONTENT_TYPE, "application/json");
    }
    let mut request = request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_owned())))
        .unwrap();
    request
        .extensions_mut()
        .insert(super::super::super::listener::OutputAuthority::default());
    request
}

async fn subscribe(
    app: &Router,
    session_id: &str,
    predecessor: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut request = lifecycle(
        "POST",
        "/v1/subscriptions",
        session_id,
        Some(r#"{"kind":"application"}"#),
    );
    if let Some(predecessor) = predecessor {
        request
            .headers_mut()
            .insert("X-Build-Subscription", predecessor.parse().unwrap());
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn subscription(app: &Router, session_id: &str) -> String {
    let (status, created) = subscribe(app, session_id, None).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["subscription_id"].as_str().unwrap().to_owned()
}

async fn events(app: &Router, session_id: &str, subscription: &str) -> Response {
    app.clone()
        .oneshot(lifecycle(
            "GET",
            &format!("/v1/subscriptions/{subscription}/events"),
            session_id,
            None,
        ))
        .await
        .unwrap()
}

async fn stream(app: &Router, session_id: &str, subscription: &str) -> Frames {
    let response = events(app, session_id, subscription).await;
    assert_eq!(response.status(), StatusCode::OK);
    Box::pin(response.into_body().into_data_stream())
}

async fn next_event(frames: &mut Frames) -> (String, serde_json::Value) {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
        .await
        .expect("an SSE frame arrives")
        .expect("the stream is still open")
        .expect("the frame is readable");
    let text = std::str::from_utf8(&frame).unwrap();
    let (name, data) = text
        .strip_prefix("event: ")
        .and_then(|rest| rest.strip_suffix("\n\n"))
        .and_then(|rest| rest.split_once("\ndata: "))
        .unwrap_or_else(|| panic!("one complete SSE event per frame: {text:?}"));
    (name.to_owned(), serde_json::from_str(data).unwrap())
}

async fn assert_ended(frames: &mut Frames) {
    let next = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
        .await
        .expect("the stream ends promptly");
    assert!(
        matches!(next, None | Some(Err(_))),
        "the stream carries no more application frames"
    );
}

async fn greeting_request(
    app: &Router,
    session_id: &str,
    subscription: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut request = lifecycle("POST", "/v1/session-greetings", session_id, Some("{}"));
    if let Some(subscription) = subscription {
        request
            .headers_mut()
            .insert("X-Build-Subscription", subscription.parse().unwrap());
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn subscription_creation_names_the_opening_and_its_events_path() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let opening = opened(&app).await;
    let session_id = opening["session_id"].as_str().unwrap();

    let (status, created) = subscribe(&app, session_id, None).await;

    assert_eq!(status, StatusCode::CREATED);
    let subscription = created["subscription_id"].as_str().unwrap();
    assert!(!subscription.is_empty() && subscription.len() <= 128);
    assert_eq!(created["opening_generation"], opening["opening_generation"]);
    assert_eq!(
        created["events_path"],
        format!("/v1/subscriptions/{subscription}/events")
    );
    assert_eq!(created["durable"], false);
}

#[tokio::test]
async fn subscription_creation_refuses_other_kinds_and_foreign_openings() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for body in [r#"{"kind":"live"}"#, r#"{"kind":"application","scope":{}}"#] {
        let response = app
            .clone()
            .oneshot(lifecycle(
                "POST",
                "/v1/subscriptions",
                &session_id,
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    let response = app
        .clone()
        .oneshot(lifecycle(
            "POST",
            "/v1/subscriptions",
            "not-an-opening",
            Some(r#"{"kind":"application"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn event_stream_starts_with_hello_and_carries_application_pushes() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let opening = opened(&app).await;
    let session_id = opening["session_id"].as_str().unwrap();
    let subscription = subscription(&app, session_id).await;

    let response = events(&app, session_id, &subscription).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[CONTENT_TYPE],
        "text/event-stream; charset=utf-8"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let mut frames: Frames = Box::pin(response.into_body().into_data_stream());

    let (name, hello) = next_event(&mut frames).await;
    assert_eq!(name, "hello");
    assert_eq!(
        hello,
        json!({
            "durable": false,
            "recovery": "current_state_refresh",
            "subscription_id": subscription,
            "opening_generation": opening["opening_generation"],
            "bridge_epoch": opening["greeting"]["bridge_epoch"],
        })
    );

    let (status, reply) = greeting_request(&app, session_id, Some(&subscription)).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["result"]["events"], true);
    let (name, pushed) = next_event(&mut frames).await;
    assert_eq!(name, "event");
    assert_eq!(pushed["type"], "probe.pushed");
    assert_eq!(pushed["bridge_epoch"], opening["greeting"]["bridge_epoch"]);
}

#[tokio::test]
async fn event_stream_refuses_a_resume_cursor_and_a_second_get() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let subscription = subscription(&app, &session_id).await;

    let mut resumed = lifecycle(
        "GET",
        &format!("/v1/subscriptions/{subscription}/events"),
        &session_id,
        None,
    );
    resumed
        .headers_mut()
        .insert("Last-Event-ID", "7".parse().unwrap());
    let response = app.clone().oneshot(resumed).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let error: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error["error_code"], "cursor_unsupported");

    let _live = stream(&app, &session_id, &subscription).await;
    let second = events(&app, &session_id, &subscription).await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn stream_bound_resources_require_the_current_subscription() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (status, missing) = greeting_request(&app, &session_id, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(missing["error_code"], "stream_required");

    let first = subscription(&app, &session_id).await;
    let (status, unbound) = greeting_request(&app, &session_id, Some(&first)).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an unbound handle has no stream"
    );
    assert_eq!(unbound["error_code"], "stream_replaced");

    let (status, replacement) = subscribe(&app, &session_id, Some(&first)).await;
    assert_eq!(status, StatusCode::CREATED);
    let second = replacement["subscription_id"].as_str().unwrap();
    let _live = stream(&app, &session_id, second).await;
    let (status, stale) = greeting_request(&app, &session_id, Some(&first)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["error_code"], "stream_replaced");
    let (status, _) = greeting_request(&app, &session_id, Some(second)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn replacement_names_the_last_handle_and_ends_the_old_stream() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = subscription(&app, &session_id).await;
    let mut old = stream(&app, &session_id, &first).await;
    assert_eq!(next_event(&mut old).await.0, "hello");

    let (status, unnamed) = subscribe(&app, &session_id, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(unnamed["error_code"], "stream_replaced");
    let (status, _) = subscribe(&app, &session_id, Some("someone-else")).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, replacement) = subscribe(&app, &session_id, Some(&first)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ended(&mut old).await;
    let second = replacement["subscription_id"].as_str().unwrap();
    let mut new = stream(&app, &session_id, second).await;
    assert_eq!(next_event(&mut new).await.0, "hello");
}

#[tokio::test]
async fn deleting_the_subscription_ends_its_stream() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let subscription = subscription(&app, &session_id).await;
    let mut frames = stream(&app, &session_id, &subscription).await;
    assert_eq!(next_event(&mut frames).await.0, "hello");

    let response = app
        .clone()
        .oneshot(lifecycle(
            "DELETE",
            &format!("/v1/subscriptions/{subscription}"),
            &session_id,
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_ended(&mut frames).await;
    let (status, _) = greeting_request(&app, &session_id, Some(&subscription)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn closing_the_opening_ends_its_stream() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let subscription = subscription(&app, &session_id).await;
    let mut frames = stream(&app, &session_id, &subscription).await;
    assert_eq!(next_event(&mut frames).await.0, "hello");

    let response = app
        .clone()
        .oneshot(close_request(&session_id))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_ended(&mut frames).await;
}

#[tokio::test]
async fn revoking_the_grant_ends_the_stream_before_another_push() {
    let live = Arc::new(AtomicBool::new(true));
    let app = router_with(
        pushing_execution(),
        verifier(authority_with(
            live.clone(),
            std::time::Duration::from_secs(60),
        )),
    );
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let subscription = subscription(&app, &session_id).await;
    let mut frames = stream(&app, &session_id, &subscription).await;
    assert_eq!(next_event(&mut frames).await.0, "hello");

    live.store(false, Ordering::SeqCst);

    let next = tokio::time::timeout(std::time::Duration::from_secs(20), frames.next())
        .await
        .expect("revocation is observed no later than the next heartbeat");
    assert!(matches!(next, None | Some(Err(_))));
}

#[tokio::test]
async fn opening_advertises_stream_bound_resources_once_streams_are_mounted() {
    let app = router_with(pushing_execution(), verifier(authority()));
    let opening = opened(&app).await;
    let expected = super::super::super::routes::manifest()
        .operations
        .iter()
        .map(|operation| operation.operation_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(opening["transport"]["operation_ids"], json!(expected));
}

#[tokio::test]
async fn client_disconnect_releases_the_subscription_and_its_stream_reservation() {
    let runtime = runtime_with(pushing_execution(), verifier(authority()));
    let app = runtime.router();
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let subscription = subscription(&app, &session_id).await;
    let idle = runtime.state.buffers.available();
    let mut frames = stream(&app, &session_id, &subscription).await;
    assert_eq!(next_event(&mut frames).await.0, "hello");
    assert!(runtime.state.buffers.available() < idle);

    // Dropping the body is the client going away mid-stream.
    drop(frames);

    assert_eq!(runtime.state.buffers.available(), idle);
    let (status, released) = greeting_request(&app, &session_id, Some(&subscription)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{released}");
    assert_eq!(released["error_code"], "stream_replaced");
    let (status, replacement) = subscribe(&app, &session_id, Some(&subscription)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the released handle is still the last issued"
    );
    let successor = replacement["subscription_id"].as_str().unwrap();
    let mut frames = stream(&app, &session_id, successor).await;
    assert_eq!(next_event(&mut frames).await.0, "hello");
    let (status, _) = greeting_request(&app, &session_id, Some(successor)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next_event(&mut frames).await.0, "event");
}

#[tokio::test]
async fn delete_racing_an_unread_stream_ends_it_without_harming_the_successor() {
    let runtime = runtime_with(pushing_execution(), verifier(authority()));
    let app = runtime.router();
    let session_id = opened(&app).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let idle = runtime.state.buffers.available();
    let first = subscription(&app, &session_id).await;
    // hello and a push are queued but never polled before the DELETE lands.
    let mut old = stream(&app, &session_id, &first).await;
    let (status, _) = greeting_request(&app, &session_id, Some(&first)).await;
    assert_eq!(status, StatusCode::OK);

    let deleted = app
        .clone()
        .oneshot(lifecycle(
            "DELETE",
            &format!("/v1/subscriptions/{first}"),
            &session_id,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let again = app
        .clone()
        .oneshot(lifecycle(
            "DELETE",
            &format!("/v1/subscriptions/{first}"),
            &session_id,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        again.status(),
        StatusCode::NO_CONTENT,
        "DELETE is idempotent on the retired tombstone"
    );

    let (status, replacement) = subscribe(&app, &session_id, Some(&first)).await;
    assert_eq!(status, StatusCode::CREATED);
    let second = replacement["subscription_id"].as_str().unwrap();
    let mut new = stream(&app, &session_id, second).await;
    assert_eq!(next_event(&mut new).await.0, "hello");

    // The old body drains whatever it still holds, then ends; its late
    // release names the deleted handle and cannot touch the successor.
    let mut drained = 0;
    loop {
        let next = tokio::time::timeout(std::time::Duration::from_secs(2), old.next())
            .await
            .expect("the deleted stream settles");
        match next {
            Some(Ok(_)) => drained += 1,
            None | Some(Err(_)) => break,
        }
        assert!(drained <= 2, "only the frames queued before DELETE");
    }
    drop(old);

    let (status, _) = greeting_request(&app, &session_id, Some(second)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the successor survives the old release"
    );
    let (name, pushed) = next_event(&mut new).await;
    assert_eq!(
        (name.as_str(), &pushed["type"]),
        ("event", &json!("probe.pushed"))
    );
    drop(new);
    assert_eq!(runtime.state.buffers.available(), idle);
}

struct MountedTls {
    _dir: tempfile::TempDir,
    base: String,
    client: reqwest::Client,
    stop: tokio::sync::watch::Sender<bool>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

async fn mounted_tls(runtime: HttpRuntime) -> MountedTls {
    let dir = tempfile::tempdir().unwrap();
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let tls = super::super::super::tls::load(&cert_path, &key_path).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let policy = super::super::super::policy::HttpPolicy::new(
        &format!("https://localhost:{port}"),
        &["https://app.example".to_owned()],
    )
    .unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let server =
        tokio::spawn(async move { serve_mounted(listener, tls, policy, runtime, stopped).await });
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
        .build()
        .unwrap();
    MountedTls {
        _dir: dir,
        base: format!("https://localhost:{port}"),
        client,
        stop,
        server,
    }
}

impl MountedTls {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("origin", "https://app.example")
            .bearer_auth("account-bound-token")
    }

    async fn shutdown(self) {
        self.stop.send(true).unwrap();
        self.server.await.unwrap().unwrap();
    }
}

/// Reads whole SSE events from a live production socket body.
struct SocketEvents {
    response: reqwest::Response,
    buffered: String,
}

impl SocketEvents {
    async fn next(&mut self) -> Option<(String, serde_json::Value)> {
        loop {
            if let Some(end) = self.buffered.find("\n\n") {
                let event: String = self.buffered.drain(..end + 2).collect();
                if event.starts_with(':') {
                    continue;
                }
                let (name, data) = event
                    .trim_end()
                    .strip_prefix("event: ")
                    .and_then(|rest| rest.split_once("\ndata: "))
                    .unwrap_or_else(|| panic!("one complete SSE event: {event:?}"));
                return Some((name.to_owned(), serde_json::from_str(data).unwrap()));
            }
            // Shorter than the 15 s heartbeat, so a stream kept open only by
            // heartbeats fails instead of passing at the grant deadline.
            let chunk =
                tokio::time::timeout(std::time::Duration::from_secs(5), self.response.chunk())
                    .await
                    .expect("the socket stream settles before the next heartbeat");
            match chunk {
                Ok(Some(bytes)) => self.buffered.push_str(std::str::from_utf8(&bytes).unwrap()),
                Ok(None) | Err(_) => return None,
            }
        }
    }
}

#[tokio::test]
async fn production_tls_listener_carries_the_subscription_stream_until_revocation() {
    let live = Arc::new(AtomicBool::new(true));
    let runtime = runtime_with(
        pushing_execution(),
        verifier(authority_with(
            live.clone(),
            std::time::Duration::from_secs(60),
        )),
    );
    let mounted = mounted_tls(runtime).await;
    let opening: serde_json::Value = mounted
        .request(reqwest::Method::POST, "/v1/sessions")
        .json(&json!({"api_range": ">=3 <4"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = opening["session_id"].as_str().unwrap().to_owned();

    let created = mounted
        .request(reqwest::Method::POST, "/v1/subscriptions")
        .header("X-Build-Session", &session_id)
        .json(&json!({"kind": "application"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value = created.json().await.unwrap();
    let subscription = created["subscription_id"].as_str().unwrap().to_owned();

    let stream = mounted
        .request(
            reqwest::Method::GET,
            created["events_path"].as_str().unwrap(),
        )
        .header("X-Build-Session", &session_id)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(
        stream.headers()[CONTENT_TYPE],
        "text/event-stream; charset=utf-8"
    );
    let mut events = SocketEvents {
        response: stream,
        buffered: String::new(),
    };
    let (name, hello) = events.next().await.expect("hello arrives over TLS");
    assert_eq!(name, "hello");
    assert_eq!(hello["subscription_id"], subscription);

    let greeted = mounted
        .request(reqwest::Method::POST, "/v1/session-greetings")
        .header("X-Build-Session", &session_id)
        .header("X-Build-Subscription", &subscription)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(greeted.status(), StatusCode::OK);
    let (name, pushed) = events.next().await.expect("the push arrives over TLS");
    assert_eq!(name, "event");
    assert_eq!(pushed["type"], "probe.pushed");

    live.store(false, Ordering::SeqCst);
    assert!(
        events.next().await.is_none(),
        "revocation ends the production socket stream"
    );

    mounted.shutdown().await;
}

#[tokio::test]
async fn production_tls_listener_deletes_a_subscription_and_ends_its_stream() {
    let runtime = runtime_with(pushing_execution(), verifier(authority()));
    let mounted = mounted_tls(runtime).await;
    let opening: serde_json::Value = mounted
        .request(reqwest::Method::POST, "/v1/sessions")
        .json(&json!({"api_range": ">=3 <4"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = opening["session_id"].as_str().unwrap().to_owned();
    let created: serde_json::Value = mounted
        .request(reqwest::Method::POST, "/v1/subscriptions")
        .header("X-Build-Session", &session_id)
        .json(&json!({"kind": "application"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let subscription = created["subscription_id"].as_str().unwrap();
    let mut events = SocketEvents {
        response: mounted
            .request(
                reqwest::Method::GET,
                created["events_path"].as_str().unwrap(),
            )
            .header("X-Build-Session", &session_id)
            .send()
            .await
            .unwrap(),
        buffered: String::new(),
    };
    assert_eq!(events.next().await.unwrap().0, "hello");

    let deleted = mounted
        .request(
            reqwest::Method::DELETE,
            &format!("/v1/subscriptions/{subscription}"),
        )
        .header("X-Build-Session", &session_id)
        .send()
        .await
        .unwrap();

    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(events.next().await.is_none());
    mounted.shutdown().await;
}

mod contract;
mod renewal;
