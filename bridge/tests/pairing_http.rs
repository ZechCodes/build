//! Integration tests for the pairing HTTP client against a mock api (wiremock).

use std::time::Duration;

use build_bridge::identity;
use build_bridge::pairing::{
    self, build_register_request, fetch_status, poll_until_approved, register,
};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn register_posts_signed_payload_and_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/devices/register"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;

    let id = identity::generate("my-box");
    let req = build_register_request(&id, "WXYZ-4F2K").unwrap();
    let client = reqwest::Client::new();
    register(&client, &server.uri(), &req)
        .await
        .expect("registration succeeds on 2xx");

    // The mock's .expect(1) is verified on drop — confirms exactly one POST landed.
}

#[tokio::test]
async fn register_conflict_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/devices/register"))
        .respond_with(ResponseTemplate::new(409).set_body_string("device_id exists"))
        .mount(&server)
        .await;

    let id = identity::generate("my-box");
    let req = build_register_request(&id, "WXYZ-4F2K").unwrap();
    let client = reqwest::Client::new();
    let err = register(&client, &server.uri(), &req)
        .await
        .expect_err("409 is an error");
    assert!(matches!(err, pairing::PairingError::Rejected(_)));
}

#[tokio::test]
async fn fetch_status_parses_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"approved": true, "owner_user_id": "u1"})),
        )
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let status = fetch_status(&client, &server.uri(), "dev-1").await.unwrap();
    assert!(status.approved);
    assert_eq!(status.owner_user_id.as_deref(), Some("u1"));
}

#[tokio::test]
async fn poll_until_approved_waits_then_returns_owner() {
    let server = MockServer::start().await;
    // First two polls: pending. Third: approved. wiremock serves mounts in order
    // using `up_to_n_times` + scoped ordering.
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"approved": false})),
        )
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"approved": true, "owner_user_id": "owner-9"})),
        )
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let owner = poll_until_approved(&client, &server.uri(), "dev-1", Duration::from_millis(10))
        .await
        .unwrap();
    assert_eq!(owner, "owner-9");
}

#[tokio::test]
async fn ensure_paired_uses_the_injected_pairing_code() {
    // Compose/dev automation injects a known code (BRIDGE_PAIRING_CODE) so a
    // scripted approver can complete the real pairing flow. The register payload
    // must carry the hash of exactly that code.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/devices/register"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"approved": true, "owner_user_id": "u1"})),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.json");
    let id = identity::generate("my-box");

    let client = reqwest::Client::new();
    let out = pairing::ensure_paired(
        &client,
        &server.uri(),
        &server.uri(),
        &path,
        id,
        Duration::from_millis(10),
        Some("FIXED-CODE"),
    )
    .await
    .expect("pairing with an injected code succeeds");
    assert!(out.approved);

    let requests = server.received_requests().await.unwrap();
    let register = requests
        .iter()
        .find(|r| r.url.path() == "/api/devices/register")
        .expect("a register call landed");
    let body: serde_json::Value = serde_json::from_slice(&register.body).unwrap();
    assert_eq!(
        body["pairing_code_hash"],
        serde_json::json!(pairing::hash_pairing_code("FIXED-CODE")),
        "register must carry the injected code's hash"
    );
}

#[tokio::test]
async fn ensure_paired_short_circuits_when_already_approved() {
    // A pre-approved identity must do zero network calls. Point at a server with no
    // mounts so any request would 404 → error; success proves nothing was sent.
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("identity.json");

    let mut id = identity::generate("my-box");
    id.approved = true;

    let client = reqwest::Client::new();
    let out = pairing::ensure_paired(
        &client,
        &server.uri(),
        &server.uri(),
        &path,
        id.clone(),
        Duration::from_millis(10),
        None,
    )
    .await
    .expect("already-approved short-circuits");
    assert_eq!(out, id);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}
