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

    let beating = PresenceReporter::start_every(&api.uri(), &identity, Duration::from_millis(40));
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

    let beating = PresenceReporter::start_every(&api.uri(), &identity, Duration::from_millis(40));
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
    let beating =
        PresenceReporter::start_every("http://127.0.0.1:1", &identity, Duration::from_millis(20));
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

    let beating = PresenceReporter::start_every(&api.uri(), &identity, Duration::from_millis(40));
    let beats = received(&api, 3).await;
    beating.abort();

    assert!(
        beats.len() >= 3,
        "a beat that hangs past the interval is dropped and the next one goes: {beats:?}"
    );
}
