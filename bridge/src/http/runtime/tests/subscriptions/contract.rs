//! The subscription contract the desktop and SPA consumers are tested against.
//!
//! `fixtures/api/http/subscription-contract.json` is this bridge's own output
//! for one subscription's life, with per-run ids replaced by placeholders.
//! Regenerate it with `BUILD_UPDATE_FIXTURES=1 cargo test --lib
//! subscription_contract`; never edit it by hand.
use super::*;

const FIXTURE: &str = "fixtures/api/http/subscription-contract.json";

fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(FIXTURE)
}

async fn recorded(response: Response) -> serde_json::Value {
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    json!({"status": status, "body": body})
}

async fn send(app: &Router, request: Request<Body>) -> serde_json::Value {
    recorded(app.clone().oneshot(request).await.unwrap()).await
}

fn greeting(session_id: &str, subscription: Option<&str>) -> Request<Body> {
    let mut request = lifecycle("POST", "/v1/session-greetings", session_id, Some("{}"));
    if let Some(subscription) = subscription {
        request
            .headers_mut()
            .insert("X-Build-Subscription", subscription.parse().unwrap());
    }
    request
}

async fn next_frame(frames: &mut Frames) -> Option<String> {
    // The clock is paused, so waiting past the heartbeat interval is instant.
    let wait = crate::http::sse::HEARTBEAT_INTERVAL + std::time::Duration::from_secs(1);
    let frame = tokio::time::timeout(wait, frames.next())
        .await
        .expect("a frame or the end arrives")?
        .ok()?;
    Some(String::from_utf8(frame.to_vec()).unwrap())
}

/// One subscription's life: refused without a stream, created, streamed
/// (hello, a push, a heartbeat), deleted, then refused as replaced.
async fn subscription_life() -> serde_json::Value {
    let app = router_with(pushing_execution(), verifier(authority()));
    let opening = opened(&app).await;
    let session_id = opening["session_id"].as_str().unwrap();
    let stream_required = send(&app, greeting(session_id, None)).await;
    let create = send(
        &app,
        lifecycle(
            "POST",
            "/v1/subscriptions",
            session_id,
            Some(r#"{"kind":"application"}"#),
        ),
    )
    .await;
    let subscription = create["body"]["subscription_id"].as_str().unwrap();

    let response = events(&app, session_id, subscription).await;
    let status = response.status().as_u16();
    let headers = json!({
        "content-type": response.headers()[CONTENT_TYPE].to_str().unwrap(),
        "cache-control": response.headers()["cache-control"].to_str().unwrap(),
    });
    let mut frames: Frames = Box::pin(response.into_body().into_data_stream());
    let mut body = vec![next_frame(&mut frames).await.expect("hello")];
    let pushed = send(&app, greeting(session_id, Some(subscription))).await;
    assert_eq!(pushed["status"], 200, "{pushed}");
    body.push(next_frame(&mut frames).await.expect("the push"));
    body.push(next_frame(&mut frames).await.expect("a heartbeat"));
    let delete = send(
        &app,
        lifecycle(
            "DELETE",
            &format!("/v1/subscriptions/{subscription}"),
            session_id,
            None,
        ),
    )
    .await;
    while let Some(frame) = next_frame(&mut frames).await {
        body.push(frame);
    }
    let stream_replaced = send(&app, greeting(session_id, Some(subscription))).await;

    let contract = json!({
        "generated_by": "bridge/src/http/runtime/tests/subscriptions/contract.rs",
        "stream_required": stream_required,
        "create": create,
        "events": {"status": status, "headers": headers, "frames": body},
        "delete": delete,
        "stream_replaced": stream_replaced,
    });
    placeholders(
        serde_json::to_string_pretty(&contract).unwrap(),
        &[
            ("session_id", session_id),
            ("subscription_id", subscription),
            (
                "opening_generation",
                opening["opening_generation"].as_str().unwrap(),
            ),
            (
                "bridge_epoch",
                opening["greeting"]["bridge_epoch"].as_str().unwrap(),
            ),
        ],
    )
}

fn placeholders(text: String, ids: &[(&str, &str)]) -> serde_json::Value {
    let text = ids.iter().fold(text, |text, (name, id)| {
        assert!(
            uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == *id),
            "masked {name} is not a UUID: {id}"
        );
        text.replace(id, &format!("{{{{{name}}}}}"))
    });
    serde_json::from_str(&text).unwrap()
}

/// Whether this run rewrites the fixture. CI never does: a rewrite there
/// would compare the fixture with itself and always pass.
fn fixture_update_requested(
    update: Option<std::ffi::OsString>,
    ci: Option<std::ffi::OsString>,
) -> bool {
    assert!(
        update.is_none() || ci.is_none(),
        "BUILD_UPDATE_FIXTURES is refused under CI; regenerate {FIXTURE} locally"
    );
    update.is_some()
}

#[test]
fn a_local_run_may_update_the_fixture_only_when_asked() {
    assert!(fixture_update_requested(Some("1".into()), None));
    assert!(!fixture_update_requested(None, None));
    assert!(!fixture_update_requested(None, Some("true".into())));
}

#[test]
#[should_panic(expected = "BUILD_UPDATE_FIXTURES is refused under CI")]
fn ci_refuses_to_update_the_fixture() {
    fixture_update_requested(Some("1".into()), Some("true".into()));
}

#[test]
#[should_panic(expected = "masked session_id is not a UUID")]
fn only_uuid_shaped_ids_are_masked() {
    placeholders(r#"{"id":"1"}"#.into(), &[("session_id", "1")]);
}

#[tokio::test(start_paused = true)]
async fn subscription_contract_fixture_is_the_bridges_own_output() {
    let contract = subscription_life().await;
    let generated = format!("{}\n", serde_json::to_string_pretty(&contract).unwrap());
    if fixture_update_requested(
        std::env::var_os("BUILD_UPDATE_FIXTURES"),
        std::env::var_os("CI"),
    ) {
        std::fs::write(fixture_path(), &generated).unwrap();
    }
    let committed = std::fs::read_to_string(fixture_path()).unwrap_or_default();
    assert_eq!(
        committed, generated,
        "{FIXTURE} differs from the bridge; regenerate it with BUILD_UPDATE_FIXTURES=1"
    );
}
