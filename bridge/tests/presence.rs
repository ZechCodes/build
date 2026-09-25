//! The presence reporter against a mock api: a beat goes out immediately and
//! every interval after, each one signed with the device identity key, and an
//! api that refuses one does not end the loop.
//!
//! `fixtures/presence_challenge.txt` is the cross-language contract — the api's
//! `skriftapp/buildapp/test_presence.py` reads the same file, so the two
//! challenge builders cannot drift apart.
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use build_bridge::presence::{heartbeat_challenge, HeartbeatRequest, PresenceReporter};
use build_bridge::reachability::Reachability;
use build_bridge::relay::DeviceIdentity;
use build_bridge::transport;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const HEARTBEAT_PATH: &str = "/api/devices/heartbeat";

fn challenge_fixture() -> HashMap<String, String> {
    let raw = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/presence_challenge.txt"),
    )
    .expect("the shared challenge fixture");
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

async fn received(server: &MockServer, at_least: usize) -> Vec<HeartbeatRequest> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let beats: Vec<HeartbeatRequest> = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r: &&Request| r.url.path() == HEARTBEAT_PATH)
            .map(|r| serde_json::from_slice(&r.body).expect("a heartbeat body"))
            .collect();
        if beats.len() >= at_least || tokio::time::Instant::now() > deadline {
            return beats;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// A device a relay has authenticated: the beat only goes out for one of
/// those, so every test about the posting itself starts from one.
fn reached() -> Reachability {
    let reachable = Reachability::unreachable();
    reachable.reached();
    reachable
}

fn identity_for(device_id: &str) -> (DeviceIdentity, String) {
    let keypair = transport::generate_identity_keypair();
    (
        DeviceIdentity {
            device_id: device_id.to_string(),
            identity_private_key_b64: keypair.private_key_b64,
        },
        keypair.public_key_b64,
    )
}

/// The api verifies this exact string; it has no shared code with the bridge,
/// so the fixture both suites read is the contract.
#[test]
fn the_challenge_is_the_string_the_api_rebuilds() {
    let fixture = challenge_fixture();
    let timestamp: i64 = fixture["timestamp"].parse().expect("a unix timestamp");
    assert_eq!(
        heartbeat_challenge(&fixture["device_id"], timestamp),
        fixture["challenge"]
    );
}

#[tokio::test]
async fn a_beat_goes_out_at_once_and_keeps_going() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let (identity, public_key_b64) = identity_for("dev-1");

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reached(), Duration::from_millis(40));
    let beats = received(&api, 3).await;
    beating.abort();

    assert!(
        beats.len() >= 3,
        "one beat on start and one per interval after: {beats:?}"
    );
    for beat in &beats {
        assert_eq!(beat.device_id, "dev-1");
        assert!(
            transport::verify_message_b64(
                &public_key_b64,
                heartbeat_challenge(&beat.device_id, beat.timestamp).as_bytes(),
                &beat.signature_b64,
            )
            .is_ok(),
            "the api can verify {beat:?} against the device identity key"
        );
    }
}

#[tokio::test]
async fn a_refused_beat_does_not_end_the_loop() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reached(), Duration::from_millis(40));
    let beats = received(&api, 3).await;
    beating.abort();

    assert!(
        beats.len() >= 3,
        "the 500 is logged and the next beat still goes: {beats:?}"
    );
}

#[tokio::test]
async fn an_unreachable_api_does_not_end_the_loop() {
    // Port 1 on loopback: nothing listens, so every post is a connection error.
    let (identity, _) = identity_for("dev-1");
    let beating = PresenceReporter::start_every(
        "http://127.0.0.1:1",
        &identity,
        &reached(),
        Duration::from_millis(20),
    );
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !beating.is_finished(),
        "an api that cannot be reached is logged, never fatal"
    );
    beating.abort();
}

/// The failure a refusal and a connection error do not cover: an api that
/// accepts the connection and then never answers. Without a request timeout the
/// beat loop parks inside one `send()` forever — presence goes stale while the
/// bridge is healthy, and nothing short of a restart recovers it.
#[tokio::test]
async fn a_hung_api_does_not_stop_the_beats() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        // Longer than this test could ever wait: the beat must be abandoned,
        // not waited on.
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .up_to_n_times(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reached(), Duration::from_millis(40));
    let beats = received(&api, 3).await;
    beating.abort();

    assert!(
        beats.len() >= 3,
        "a beat that hangs past the interval is dropped and the next one goes: {beats:?}"
    );
}

/// The beat says this device can be REACHED, not that its process is running.
/// A daemon that can still reach the api but holds no relay socket has no way
/// in, so it says nothing and the api lets it go a window later — the
/// forty-three minutes b02b5ba1 spent listed online with no socket on
/// 2026-09-19 is what a beat that ignored this cost.
#[tokio::test]
async fn a_device_no_relay_has_authenticated_does_not_beat() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");

    let beating = PresenceReporter::start_every(
        &api.uri(),
        &identity,
        &Reachability::unreachable(),
        Duration::from_millis(40),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let beats = received(&api, 0).await;
    beating.abort();

    assert!(
        beats.is_empty(),
        "a device nothing can route to must not report itself online: {beats:?}"
    );
}

/// And the two follow each other: the socket going takes the beats with it, and
/// the socket coming back brings them straight back, with no restart in
/// between.
#[tokio::test]
async fn the_beats_follow_the_relay_socket_away_and_back() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");
    let reachable = reached();

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reachable, Duration::from_millis(40));
    let while_connected = received(&api, 2).await.len();
    assert!(while_connected >= 2, "the device was beating to begin with");

    reachable.lost();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after_the_socket_went = received(&api, 0).await.len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        received(&api, 0).await.len(),
        after_the_socket_went,
        "nothing is posted while the device cannot be reached"
    );

    reachable.reached();
    let back = received(&api, after_the_socket_went + 2).await.len();
    beating.abort();

    assert!(
        back > after_the_socket_went,
        "the socket coming back is the device coming back, with no restart in between"
    );
}

/// Every deploy of the api leaves its one pod replaced with nothing behind the
/// ingress for half a minute, and the ingress answers that gap with a 404
/// (#131: the 211 refused beats of one evening, each some seconds after a
/// rollout). A beat refused for a reason that passes — the ingress's 404, a
/// 429, a 5xx, no answer at all — is tried again a sixth of an interval
/// later, not a whole interval: at 30 s, a device stays online through the
/// gap rather than spending a third of its 90 s window on one lost beat.
#[tokio::test]
async fn a_beat_refused_in_passing_is_tried_again_soon() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(404))
        .up_to_n_times(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reached(), Duration::from_secs(3));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let beats = received(&api, 0).await;
    beating.abort();

    assert_eq!(
        beats.len(),
        2,
        "the 404 was followed well inside the 3 s interval: {beats:?}"
    );
}

/// A failure that does not pass soon is not asked about every few seconds
/// for as long as it lasts: the retries come less soon each time, up to the
/// interval, so ten devices behind one address cannot spend the api's whole
/// budget for it between them (#131 review). At a 3 s interval the beats
/// come at 0 s, then about 0.5, 1.5, 3.5 and 6.5 s — against twelve in 6 s
/// at a fixed half second.
#[tokio::test]
async fn a_failure_that_goes_on_is_retried_less_soon_each_time() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reached(), Duration::from_secs(3));
    tokio::time::sleep(Duration::from_millis(6000)).await;
    let beats = received(&api, 0).await;
    beating.abort();

    assert!(
        (4..=5).contains(&beats.len()),
        "{} beats in 6 s of 503s",
        beats.len()
    );
}

/// A 429 that says when to come back is not asked before then, although a
/// first retry would otherwise come half a second later; and once a beat
/// lands, the next waits the interval.
#[tokio::test]
async fn a_rate_limited_beat_waits_out_the_retry_after_and_recovers() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "2"))
        .up_to_n_times(1)
        .mount(&api)
        .await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reached(), Duration::from_secs(3));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let waiting = received(&api, 0).await.len();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after_it = received(&api, 0).await.len();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let then = received(&api, 0).await.len();
    beating.abort();

    assert_eq!(waiting, 1, "nothing before the Retry-After's 2 s");
    assert_eq!(after_it, 2, "asked again once it passed");
    assert_eq!(then, 2, "landed, so the next waits the 3 s interval");
}

/// A beat the api refuses on its merits — a device it does not know, a
/// signature it will not take — is not asked again early: the answer would
/// be the same.
#[tokio::test]
async fn a_beat_refused_on_its_merits_waits_the_interval() {
    let api = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(HEARTBEAT_PATH))
        .respond_with(ResponseTemplate::new(401))
        .mount(&api)
        .await;
    let (identity, _) = identity_for("dev-1");

    let beating =
        PresenceReporter::start_every(&api.uri(), &identity, &reached(), Duration::from_secs(3));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let beats = received(&api, 0).await;
    beating.abort();

    assert_eq!(beats.len(), 1, "one beat, then the interval: {beats:?}");
}
