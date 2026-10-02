//! Integration tests for the pairing HTTP client against a mock api (wiremock).

use std::time::Duration;

use build_bridge::identity;
use build_bridge::pairing::{
    self, build_register_request, fetch_status, poll_until_approved, register, RetireWhen,
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

// --- a stored approval the api no longer honours (#317) -------------------------

async fn api_answering_status(body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    server
}

/// An identity saved in `dir`: approved by `approved_by` when there is one,
/// else still pending.
fn stored(
    dir: &tempfile::TempDir,
    approved_by: Option<&str>,
) -> (std::path::PathBuf, identity::StoredIdentity) {
    let path = dir.path().join("identity.json");
    let mut id = identity::generate("my-box");
    id.approved = approved_by.is_some();
    id.approved_by = approved_by.map(str::to_string);
    identity::save(&path, &id).unwrap();
    (path, id)
}

#[tokio::test]
async fn fetch_status_reads_the_state_and_tolerates_an_api_without_one() {
    let with_state = api_answering_status(serde_json::json!({
        "approved": false, "owner_user_id": null, "state": "revoked"
    }))
    .await;
    let without = api_answering_status(serde_json::json!({"approved": false})).await;
    let client = reqwest::Client::new();
    let revoked = fetch_status(&client, &with_state.uri(), "dev-1")
        .await
        .unwrap();
    let old_api = fetch_status(&client, &without.uri(), "dev-1")
        .await
        .unwrap();
    assert_eq!(revoked.lapse(), Some(pairing::Lapse::Revoked));
    assert_eq!(old_api.lapse(), Some(pairing::Lapse::NotApproved));
}

#[tokio::test]
async fn a_revoked_approval_is_retired_so_the_next_pairing_mints_a_new_identity() {
    let server = api_answering_status(serde_json::json!({
        "approved": false, "owner_user_id": null, "state": "revoked"
    }))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some(&server.uri()));

    let retired = pairing::retire_lapsed_approval(
        &reqwest::Client::new(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await
    .unwrap()
    .expect("a revoked approval is retired");

    assert_eq!(retired.lapse, pairing::Lapse::Revoked);
    assert!(
        identity::load(&path).unwrap().is_none(),
        "nothing left to load"
    );
    assert_eq!(identity::load(&retired.kept_at).unwrap(), Some(id));
    let said = words(&retired.notice(dir.path()));
    assert!(said.contains("no longer valid"), "{said}");
    assert!(said.contains("kept in ~."), "{said}");
    assert!(!said.contains("already paired"), "{said}");
}

#[tokio::test]
async fn an_approval_the_api_never_heard_of_is_retired_too() {
    let server = api_answering_status(serde_json::json!({
        "approved": false, "owner_user_id": null, "state": "unknown"
    }))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = stored(&dir, Some(&server.uri()));
    let retired = pairing::retire_lapsed_approval(
        &reqwest::Client::new(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await
    .unwrap()
    .expect("an unknown device is retired");
    assert_eq!(retired.lapse, pairing::Lapse::Unknown);
}

#[tokio::test]
async fn an_approval_the_api_still_honours_is_left_alone() {
    let server = api_answering_status(serde_json::json!({
        "approved": true, "owner_user_id": "u1", "state": "approved"
    }))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some(&server.uri()));
    let retired = pairing::retire_lapsed_approval(
        &reqwest::Client::new(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await
    .unwrap();
    assert!(retired.is_none());
    assert_eq!(identity::load(&path).unwrap(), Some(id));
}

/// A pending identity re-registers with a fresh code on its own, and no
/// identity has nothing to retire: neither asks the api anything.
#[tokio::test]
async fn only_a_stored_approval_is_checked() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("none.json");
    let client = reqwest::Client::new();
    assert!(pairing::retire_lapsed_approval(
        &client,
        &server.uri(),
        &missing,
        RetireWhen::ApproverSays
    )
    .await
    .unwrap()
    .is_none());
    let (pending, id) = stored(&dir, None);
    assert!(pairing::retire_lapsed_approval(
        &client,
        &server.uri(),
        &pending,
        RetireWhen::ApproverSays
    )
    .await
    .unwrap()
    .is_none());
    assert_eq!(identity::load(&pending).unwrap(), Some(id));
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

/// An api that cannot answer is not an api that said no: the identity stays.
#[tokio::test]
async fn an_unreachable_api_retires_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some(&server.uri()));
    assert!(pairing::retire_lapsed_approval(
        &reqwest::Client::new(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays
    )
    .await
    .is_err());
    assert_eq!(identity::load(&path).unwrap(), Some(id));
}

/// A hung api ends in an error inside the status client's bound, never a hang,
/// and a stored approval it could not confirm stays where it is.
#[tokio::test]
async fn a_hung_api_times_out_and_retires_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"approved": false, "state": "revoked"}))
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some(&server.uri()));
    let client =
        pairing::status_client_with(Duration::from_millis(200), Duration::from_millis(300));

    let started = std::time::Instant::now();
    let outcome =
        pairing::retire_lapsed_approval(&client, &server.uri(), &path, RetireWhen::ApproverSays)
            .await;

    assert!(outcome.is_err(), "a timeout is not an answer");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(identity::load(&path).unwrap(), Some(id));
}

/// The installer's status client is bounded on both connect and the whole call.
#[test]
fn the_status_client_bounds_are_bounded() {
    assert!(pairing::STATUS_CONNECT_TIMEOUT <= Duration::from_secs(15));
    assert!(pairing::STATUS_TIMEOUT <= Duration::from_secs(60));
    assert!(pairing::STATUS_CONNECT_TIMEOUT <= pairing::STATUS_TIMEOUT);
    let _client = pairing::status_client();
}

/// Every way the api can fail to say "not approved" leaves the identity alone.
async fn assert_retires_nothing(api_url: &str) {
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some(api_url));
    let outcome = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        api_url,
        &path,
        RetireWhen::ApproverSays,
    )
    .await;
    assert!(outcome.is_err(), "{api_url}: {outcome:?}");
    assert_eq!(identity::load(&path).unwrap(), Some(id), "{api_url}");
}

#[tokio::test]
async fn a_refused_connection_retires_nothing() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    assert_retires_nothing(&format!("http://127.0.0.1:{port}")).await;
}

#[tokio::test]
async fn a_body_that_is_not_json_retires_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/devices/.+/status$"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not approved</html>"))
        .mount(&server)
        .await;
    assert_retires_nothing(&server.uri()).await;
}

/// `approved` is required: if it ever gained a serde default, a body without
/// it would read as "not approved" and retire a good identity.
#[tokio::test]
async fn a_body_without_approved_retires_nothing() {
    let server =
        api_answering_status(serde_json::json!({"owner_user_id": null, "state": "revoked"})).await;
    assert_retires_nothing(&server.uri()).await;
}

/// Pairing against the wrong `BRIDGE_API_URL` retires a good identity, so the
/// message names an api that is not the default and where the old identity is
/// kept; putting it back is the README's to say (#319).
#[tokio::test]
async fn the_retire_message_names_the_api_and_where_the_old_identity_is() {
    let server =
        api_answering_status(serde_json::json!({"approved": false, "state": "unknown"})).await;
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = stored(&dir, Some(&server.uri()));
    let retired = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await
    .unwrap()
    .unwrap();
    let said = words(&retired.notice(std::path::Path::new("/nowhere")));
    assert!(said.contains(&server.uri()), "{said}");
    assert!(
        said.contains(&format!("kept in {}.", dir.path().display())),
        "{said}"
    );
    assert!(!said.contains("mv "), "{said}");
}

/// What `pair` pairs after a retire is a new device: new id, new keys.
#[tokio::test]
async fn the_identity_minted_after_a_retire_is_a_new_device() {
    let server =
        api_answering_status(serde_json::json!({"approved": false, "state": "revoked"})).await;
    let dir = tempfile::tempdir().unwrap();
    let (path, old) = stored(&dir, Some(&server.uri()));
    pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await
    .unwrap()
    .unwrap();
    let new = identity::load_or_generate(&path, "my-box").unwrap();
    assert_ne!(new.device_id, old.device_id);
    assert_ne!(new.identity_public_key_b64, old.identity_public_key_b64);
    assert_ne!(new.transport.public_key_b64, old.transport.public_key_b64);
    assert!(!new.approved);
    assert_eq!(
        identity::load(&path).unwrap(),
        Some(new),
        "the new identity is saved"
    );
}

// --- only the api that approved an identity retires it (#320) ------------------

/// An api pointed at by a stray `BRIDGE_API_URL` — a local mock, say — is not
/// the api that approved this machine, and its "not approved" retires nothing.
#[tokio::test]
async fn another_apis_answer_retires_nothing() {
    let server =
        api_answering_status(serde_json::json!({"approved": false, "state": "unknown"})).await;
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some("https://getbuild.ing"));

    let outcome = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await;

    assert!(
        matches!(outcome, Err(pairing::PairingError::ApprovedElsewhere)),
        "{outcome:?}"
    );
    assert_eq!(identity::load(&path).unwrap(), Some(id));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// An identity approved before the approver was recorded was approved by the
/// default api, the only one the installer pairs with: any other api's
/// answer leaves it alone.
#[tokio::test]
async fn an_approval_from_before_the_approver_was_recorded_belongs_to_the_default_api() {
    let server =
        api_answering_status(serde_json::json!({"approved": false, "state": "revoked"})).await;
    let dir = tempfile::tempdir().unwrap();
    let (path, mut id) = stored(&dir, Some("unused"));
    id.approved_by = None;
    identity::save(&path, &id).unwrap();

    let outcome = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await;

    assert!(
        matches!(outcome, Err(pairing::PairingError::ApprovedElsewhere)),
        "{outcome:?}"
    );
    assert_eq!(identity::load(&path).unwrap(), Some(id));
}

/// The approver is the same api with or without a trailing slash.
#[tokio::test]
async fn the_approver_is_matched_without_its_trailing_slash() {
    let server =
        api_answering_status(serde_json::json!({"approved": false, "state": "revoked"})).await;
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = stored(&dir, Some(&format!("{}/", server.uri())));
    let retired = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await
    .unwrap();
    assert!(retired.is_some());
}

/// `pair --retire` takes any api's word for it.
#[tokio::test]
async fn retire_takes_any_apis_answer() {
    let server =
        api_answering_status(serde_json::json!({"approved": false, "state": "unknown"})).await;
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some("https://getbuild.ing"));

    let retired = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &server.uri(),
        &path,
        RetireWhen::AnyApiSays,
    )
    .await
    .unwrap()
    .expect("retired on the operator's say-so");

    assert_eq!(identity::load(&retired.kept_at).unwrap(), Some(id));
}

/// An api that still approves the identity is no reason to refuse, whoever
/// approved it.
#[tokio::test]
async fn another_api_that_still_approves_is_no_refusal() {
    let server = api_answering_status(serde_json::json!({
        "approved": true, "owner_user_id": "u1", "state": "approved"
    }))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = stored(&dir, Some("https://getbuild.ing"));
    let retired = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &server.uri(),
        &path,
        RetireWhen::ApproverSays,
    )
    .await
    .unwrap();
    assert!(retired.is_none());
    assert_eq!(identity::load(&path).unwrap(), Some(id));
}

/// The approval `ensure_paired` saves names the api that gave it.
#[tokio::test]
async fn a_new_approval_records_the_api_that_gave_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/devices/register"))
        .respond_with(ResponseTemplate::new(201))
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

    let api = format!("{}/", server.uri());
    let out = pairing::ensure_paired(
        &reqwest::Client::new(),
        &api,
        &server.uri(),
        &path,
        identity::generate("my-box"),
        Duration::from_millis(10),
        None,
    )
    .await
    .unwrap();

    assert_eq!(out.approved_by.as_deref(), Some(server.uri().as_str()));
    assert_eq!(identity::load(&path).unwrap(), Some(out));
}

/// `text` with its line breaks and indents read as single spaces, so an
/// assertion holds however the notice wraps.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
