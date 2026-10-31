//! Renewal end to end: the production verifier, a hosted introspection
//! endpoint and the production TLS listener. Evidence here is fresh for
//! `WINDOW` instead of production's 30 s, so real time crosses a grant.
use super::*;
use std::time::Duration;
use tokio::time::Instant;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WINDOW: Duration = Duration::from_secs(6);
const OPENING_BEARER: &str = "account-bound-token";
const OTHER_BEARER: &str = "other-family-token";
const INTROSPECT: &str = "/api/bridge-http/introspect";

fn active(family: &str) -> ResponseTemplate {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    ResponseTemplate::new(200).set_body_json(json!({
        "active": true, "issuer": "https://getbuild.ing", "account_id": "account-a",
        "device_id": "device-a", "origin": "https://bridge.example", "endpoint_epoch": "epoch-a",
        "authorization_epoch": 1, "family_id": family, "token_id": format!("{family}-token"),
        "scope": "bridge:read bridge:write", "exp": exp,
    }))
}

/// Hosted answers `bearer` with `answer`, `times` times or for good.
async fn hosted_answers(
    hosted: &MockServer,
    bearer: &str,
    answer: ResponseTemplate,
    times: Option<u64>,
) {
    let mock = Mock::given(method("POST"))
        .and(path(INTROSPECT))
        .and(body_partial_json(json!({ "token": bearer })))
        .respond_with(answer);
    match times {
        Some(times) => mock.up_to_n_times(times),
        None => mock,
    }
    .mount(hosted)
    .await;
}

/// A mounted opening streaming over TLS, verified by hosted once; hosted
/// answers its renewal with `renewal`.
struct HostedOpening {
    mounted: MountedTls,
    hosted: MockServer,
    opened_at: Instant,
    session_id: String,
    subscription: String,
    events: SocketEvents,
}

async fn hosted_opening(renewal: ResponseTemplate) -> HostedOpening {
    let hosted = MockServer::start().await;
    hosted_answers(&hosted, OPENING_BEARER, active("family-a"), Some(1)).await;
    hosted_answers(&hosted, OPENING_BEARER, renewal, None).await;
    let verifier = crate::http::auth::HttpAuthVerifier::hosted_for_tests(
        format!("{}{INTROSPECT}", hosted.uri()),
        WINDOW,
    );
    let mounted = mounted_tls(runtime_with(pushing_execution(), Arc::new(verifier))).await;
    let opened_at = Instant::now();
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
    assert_eq!(events.next().await.expect("hello").0, "hello");
    let opening = HostedOpening {
        mounted,
        hosted,
        opened_at,
        session_id,
        subscription: created["subscription_id"].as_str().unwrap().to_owned(),
        events,
    };
    assert_eq!(opening.introspections().await, 1, "one lookup, then cached");
    opening
}

impl HostedOpening {
    async fn introspections(&self) -> usize {
        self.hosted.received_requests().await.unwrap().len()
    }

    async fn renew_as(&self, bearer: &str) -> (StatusCode, serde_json::Value) {
        let response = self
            .mounted
            .client
            .patch(format!(
                "{}/v1/sessions/{}",
                self.mounted.base, self.session_id
            ))
            .header("origin", "https://app.example")
            .header("X-Build-Session", &self.session_id)
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap();
        (response.status(), response.json().await.unwrap())
    }

    async fn push(&self) -> StatusCode {
        self.mounted
            .request(reqwest::Method::POST, "/v1/session-greetings")
            .header("X-Build-Session", &self.session_id)
            .header("X-Build-Subscription", &self.subscription)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn assert_delivers(&mut self, why: &str) {
        assert_eq!(self.push().await, StatusCode::OK, "{why}");
        let (name, pushed) = self.events.next().await.unwrap_or_else(|| panic!("{why}"));
        assert_eq!(
            (name.as_str(), &pushed["type"]),
            ("event", &json!("probe.pushed"))
        );
    }

    /// The stream ends, and returns when it did.
    async fn ended_at(&mut self) -> Instant {
        assert!(
            self.events.next().await.is_none(),
            "no frame after authority ends"
        );
        Instant::now()
    }

    async fn sleep_past_first_grant(&self) {
        tokio::time::sleep_until(self.opened_at + WINDOW + Duration::from_secs(1)).await;
    }
}

#[tokio::test]
async fn a_hosted_renewal_carries_the_tls_stream_past_its_first_grant() {
    let mut opening = hosted_opening(active("family-a")).await;
    tokio::time::sleep_until(opening.opened_at + WINDOW * 2 / 3).await;

    let (status, renewed) = opening.renew_as(OPENING_BEARER).await;

    assert_eq!(status, StatusCode::OK, "{renewed}");
    assert_eq!(
        opening.introspections().await,
        2,
        "renewal asks hosted afresh, never the cache"
    );
    opening.sleep_past_first_grant().await;
    opening
        .assert_delivers("the renewed stream outlives its first grant")
        .await;
    opening.mounted.shutdown().await;
}

#[tokio::test]
async fn hosted_inactive_at_renewal_ends_the_tls_stream_at_once() {
    let mut opening = hosted_opening(ResponseTemplate::new(200).set_body_json(json!({
        "active": false
    })))
    .await;

    let (status, refused) = opening.renew_as(OPENING_BEARER).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    assert_eq!(refused["error_code"], "invalid_credential");
    let ended_at = opening.ended_at().await;
    assert!(
        ended_at < opening.opened_at + WINDOW,
        "revocation, not the grant deadline, ended the stream"
    );
    assert_eq!(opening.push().await, StatusCode::UNAUTHORIZED);
    opening.mounted.shutdown().await;
}

#[tokio::test]
async fn hosted_unavailable_at_renewal_leaves_only_the_first_deadline() {
    let mut opening = hosted_opening(ResponseTemplate::new(503)).await;
    tokio::time::sleep_until(opening.opened_at + WINDOW * 2 / 3).await;

    let (status, refused) = opening.renew_as(OPENING_BEARER).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
    assert_eq!(refused["retryable"], true);
    assert_eq!(opening.introspections().await, 2);
    opening
        .assert_delivers("open until the first grant's deadline")
        .await;
    let ended_at = opening.ended_at().await;
    assert!(
        ended_at >= opening.opened_at + WINDOW,
        "the stream ran to its first deadline"
    );
    opening.mounted.shutdown().await;
}

#[tokio::test]
async fn another_bindings_bearer_cannot_renew_the_tls_opening() {
    let mut opening = hosted_opening(active("family-a")).await;
    hosted_answers(&opening.hosted, OTHER_BEARER, active("family-b"), None).await;
    tokio::time::sleep_until(opening.opened_at + WINDOW * 2 / 3).await;

    let (status, refused) = opening.renew_as(OTHER_BEARER).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["error_code"], "not_found");
    opening
        .assert_delivers("the 404 leaves the original stream untouched")
        .await;
    let ended_at = opening.ended_at().await;
    assert!(
        ended_at >= opening.opened_at + WINDOW,
        "the stream kept its own deadline"
    );
    opening.mounted.shutdown().await;
}
