//! Content-free attention notifications — bridge → api → web push.
//!
//! When a task transitions into a state that needs the human (`plan_review`,
//! `review`, `blocked`, `failed`, `idle_unreported`), the bridge POSTs a signed,
//! timestamped notify to the api's `/api/push/notify`, which fans a **generic**
//! push out to the owner's browsers. E2EE invariant: the request names the
//! device and the kind (`"attention"`) — never the task, the goal, or any
//! content. The browser learns *what* needs attention only over the E2EE
//! channel once the app opens.
//!
//! Authentication is the registration scheme reused: an Ed25519 signature over
//! [`notify_challenge`], which mirrors `skriftapp/buildapp/web_push.py`
//! byte-for-byte; the timestamp bounds replay of a captured request.
//!
//! Throttling: [`NotifyThrottle`] fires **at most one notify per task-state
//! change** — repeated mutations that leave a task in the same state (or in a
//! state that doesn't need the human) push nothing.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::task::TaskState;
use crate::transport;

/// The only notify kind: "a task needs your attention". Content-free by contract.
pub const ATTENTION_KIND: &str = "attention";

/// The payload POSTed to `/api/push/notify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyRequest {
    pub device_id: String,
    pub kind: String,
    /// Unix seconds; the api rejects requests outside its freshness window.
    pub timestamp: i64,
    /// Ed25519 signature (padded b64) over [`notify_challenge`].
    pub signature_b64: String,
}

/// The canonical message signed for a notify. Binds the device, the kind, and
/// the timestamp so a captured signature cannot be replayed onto a different
/// notification. Mirrors `buildapp.web_push.notify_challenge` exactly.
pub fn notify_challenge(device_id: &str, kind: &str, timestamp: i64) -> String {
    format!("notify.{device_id}.{kind}.{timestamp}")
}

/// Build a fully-signed attention [`NotifyRequest`] for `timestamp` (unix
/// seconds). Pure given its inputs; does not mutate them.
pub fn build_notify_request(
    device_id: &str,
    identity_private_key_b64: &str,
    timestamp: i64,
) -> Result<NotifyRequest, String> {
    let challenge = notify_challenge(device_id, ATTENTION_KIND, timestamp);
    let signature_b64 = transport::sign_message_b64(identity_private_key_b64, challenge.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(NotifyRequest {
        device_id: device_id.to_string(),
        kind: ATTENTION_KIND.to_string(),
        timestamp,
        signature_b64,
    })
}

/// The push-worthy subset of [`TaskState::needs_attention`]: states an agent (or
/// its silence) put the task in, where only the human moves it forward.
/// `interrupted` is excluded — it's raised by boot recovery after a daemon
/// restart, where the operator is already at the machine.
pub fn state_needs_push(state: &TaskState) -> bool {
    matches!(
        state,
        TaskState::PlanReview
            | TaskState::Review
            | TaskState::Blocked(_)
            | TaskState::Failed(_)
            | TaskState::IdleUnreported(_)
    )
}

/// At most one notify per task-state change: remembers the last state observed
/// per task and fires only when the state actually changed into a push-worthy
/// one. Owns no I/O — the caller sends the push.
#[derive(Debug, Default)]
pub struct NotifyThrottle {
    last_state: HashMap<String, String>,
}

impl NotifyThrottle {
    /// Record `state` for `task_id`; `true` iff this observation is a *change*
    /// into a state that needs the human.
    pub fn should_notify(&mut self, task_id: &str, state: &TaskState) -> bool {
        let state_repr = format!("{state:?}");
        let unchanged = self
            .last_state
            .get(task_id)
            .is_some_and(|previous| previous == &state_repr);
        self.last_state.insert(task_id.to_string(), state_repr);
        !unchanged && state_needs_push(state)
    }
}

/// Sends signed attention notifies to the api. Cheap to clone (the reqwest
/// client is an `Arc` internally) so callers can fire-and-forget from a spawned
/// task without holding any app lock.
#[derive(Clone)]
pub struct Notifier {
    api_url: String,
    device_id: String,
    identity_private_key_b64: String,
    client: reqwest::Client,
}

impl Notifier {
    pub fn new(api_url: &str, device_id: &str, identity_private_key_b64: &str) -> Self {
        Notifier {
            api_url: api_url.trim_end_matches('/').to_string(),
            device_id: device_id.to_string(),
            identity_private_key_b64: identity_private_key_b64.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// POST one signed, freshly-timestamped attention notify.
    pub async fn notify_attention(&self) -> Result<(), String> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;
        let request =
            build_notify_request(&self.device_id, &self.identity_private_key_b64, timestamp)?;
        let url = format!("{}/api/push/notify", self.api_url);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("api rejected notify: {}", response.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Phase;

    #[test]
    fn challenge_matches_the_api_contract_and_binds_all_fields() {
        // The exact string buildapp.web_push.notify_challenge produces.
        let base = notify_challenge("dev-1", "attention", 1_750_000_000);
        assert_eq!(base, "notify.dev-1.attention.1750000000");
        assert_ne!(base, notify_challenge("dev-2", "attention", 1_750_000_000));
        assert_ne!(base, notify_challenge("dev-1", "other", 1_750_000_000));
        assert_ne!(base, notify_challenge("dev-1", "attention", 1_750_000_001));
    }

    #[test]
    fn notify_request_signature_verifies_and_binds_the_timestamp() {
        let identity = transport::generate_identity_keypair();
        let request = build_notify_request("dev-1", &identity.private_key_b64, 1_750_000_000)
            .expect("signable");
        assert_eq!(request.kind, ATTENTION_KIND);
        let challenge = notify_challenge(&request.device_id, &request.kind, request.timestamp);
        transport::verify_message_b64(
            &identity.public_key_b64,
            challenge.as_bytes(),
            &request.signature_b64,
        )
        .expect("notify signature verifies against the device identity key");

        // A replayed signature on a different timestamp must fail.
        let forged = notify_challenge(&request.device_id, &request.kind, 1_750_009_999);
        assert!(transport::verify_message_b64(
            &identity.public_key_b64,
            forged.as_bytes(),
            &request.signature_b64,
        )
        .is_err());
    }

    #[test]
    fn notify_request_carries_no_task_content() {
        let identity = transport::generate_identity_keypair();
        let request =
            build_notify_request("dev-1", &identity.private_key_b64, 1_750_000_000).unwrap();
        let json = serde_json::to_value(&request).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["device_id", "kind", "signature_b64", "timestamp"]);
    }

    #[test]
    fn push_worthy_states_are_exactly_the_agent_raised_attention_states() {
        for state in [
            TaskState::PlanReview,
            TaskState::Review,
            TaskState::Blocked(Phase::Plan),
            TaskState::Failed(Phase::Build),
            TaskState::IdleUnreported(Phase::Build),
        ] {
            assert!(state_needs_push(&state), "{state:?} should push");
        }
        for state in [
            TaskState::Created,
            TaskState::Planning,
            TaskState::Building,
            TaskState::Interrupted(Phase::Build),
            TaskState::Merged,
            TaskState::Abandoned,
        ] {
            assert!(!state_needs_push(&state), "{state:?} should not push");
        }
    }

    #[test]
    fn throttle_fires_once_per_state_change() {
        let mut throttle = NotifyThrottle::default();
        assert!(throttle.should_notify("t1", &TaskState::PlanReview));
        // The same state observed again (e.g. a re-persist) pushes nothing.
        assert!(!throttle.should_notify("t1", &TaskState::PlanReview));
        // Leaving for a working state pushes nothing…
        assert!(!throttle.should_notify("t1", &TaskState::Planning));
        // …but a *fresh* transition back into plan_review (revision round) does.
        assert!(throttle.should_notify("t1", &TaskState::PlanReview));
        // Distinct attention states in sequence each fire once.
        assert!(throttle.should_notify("t1", &TaskState::Blocked(Phase::Plan)));
        assert!(!throttle.should_notify("t1", &TaskState::Blocked(Phase::Plan)));
    }

    #[test]
    fn throttle_tracks_tasks_independently() {
        let mut throttle = NotifyThrottle::default();
        assert!(throttle.should_notify("t1", &TaskState::Review));
        assert!(throttle.should_notify("t2", &TaskState::Review));
        assert!(!throttle.should_notify("t1", &TaskState::Review));
    }

    #[tokio::test]
    async fn notifier_posts_a_verifiable_signed_notify() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/push/notify"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let identity = transport::generate_identity_keypair();
        let notifier = Notifier::new(&server.uri(), "dev-1", &identity.private_key_b64);
        notifier.notify_attention().await.expect("notify accepted");

        let requests = server.received_requests().await.unwrap();
        let body: NotifyRequest = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body.device_id, "dev-1");
        assert_eq!(body.kind, ATTENTION_KIND);
        let challenge = notify_challenge(&body.device_id, &body.kind, body.timestamp);
        transport::verify_message_b64(
            &identity.public_key_b64,
            challenge.as_bytes(),
            &body.signature_b64,
        )
        .expect("posted signature verifies");
    }

    #[tokio::test]
    async fn notifier_surfaces_api_rejection() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/push/notify"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let identity = transport::generate_identity_keypair();
        let notifier = Notifier::new(&server.uri(), "dev-1", &identity.private_key_b64);
        let err = notifier.notify_attention().await.unwrap_err();
        assert!(err.contains("401"), "{err}");
    }
}
