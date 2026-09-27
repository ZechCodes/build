//! Content-free push notifications — bridge → api → web push.
//!
//! A push follows the unread counter (#191): it fires when something adds to
//! the badge the inbox wears, and at no other time. Two things add to it — an
//! attention-class item on a watched agent's conversation, and news on a
//! watched, unfinished task — and each has its kind. The bridge POSTs a
//! signed, timestamped notify to the api's `/api/push/notify`, which fans a push
//! out to the owner's browsers. E2EE invariant: the request names the device,
//! the **opaque entity id**, and a **generic kind** (`agent`/`task`) — never a
//! message, a title or any other content. The browser renders the kind's copy
//! and deep-links by the id; the real state loads only over the E2EE channel
//! once the app opens.
//!
//! Authentication is the registration scheme reused: an Ed25519 signature over
//! [`notify_challenge`], which mirrors `skriftapp/buildapp/web_push.py`
//! byte-for-byte; the timestamp bounds replay of a captured request.
//!
//! Throttling: [`NotifyThrottle`] fires **at most one notify per entity per
//! [`NOTIFY_DEBOUNCE_SECONDS`]**. News arrives in bursts — an agent's reply and
//! the report behind it are one piece of news — so the window is what keeps a
//! phone quiet.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::transport;

/// Notify kinds (contract #6). A kind is a generic label, never content; the
/// service worker renders its copy and the api picks the deep link by it.
///
/// An agent said something that needs the human; the id is the entity (the
/// workspace's or project's conversation owner) the agent is on.
pub const AGENT_KIND: &str = "agent";
/// A watched task has news; the id is the task's. User-facing copy calls a
/// task a task (#190).
pub const TASK_KIND: &str = "task";

/// Now, in unix seconds — the clock [`NotifyThrottle`] debounces against.
pub fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// The payload POSTed to `/api/push/notify`. Carries the opaque `task_id` and the
/// generic `kind` (both allowed metadata under the E2EE contract — never goals or
/// plan text) so the api can build a deep-linked, kind-specific push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyRequest {
    pub device_id: String,
    pub task_id: String,
    pub kind: String,
    /// Unix seconds; the api rejects requests outside its freshness window.
    pub timestamp: i64,
    /// Ed25519 signature (padded b64) over [`notify_challenge`].
    pub signature_b64: String,
}

/// The canonical message signed for a notify. Binds the device, the task, the
/// kind, and the timestamp so a captured signature cannot be replayed onto a
/// different notification. Mirrors `buildapp.web_push.notify_challenge` exactly.
pub fn notify_challenge(device_id: &str, task_id: &str, kind: &str, timestamp: i64) -> String {
    format!("notify.{device_id}.{task_id}.{kind}.{timestamp}")
}

/// Build a fully-signed [`NotifyRequest`] for `task_id`/`kind` at `timestamp`
/// (unix seconds). Pure given its inputs; does not mutate them.
pub fn build_notify_request(
    device_id: &str,
    identity_private_key_b64: &str,
    task_id: &str,
    kind: &str,
    timestamp: i64,
) -> Result<NotifyRequest, String> {
    let challenge = notify_challenge(device_id, task_id, kind, timestamp);
    let signature_b64 = transport::sign_message_b64(identity_private_key_b64, challenge.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(NotifyRequest {
        device_id: device_id.to_string(),
        task_id: task_id.to_string(),
        kind: kind.to_string(),
        timestamp,
        signature_b64,
    })
}

/// How long one entity stays quiet after a push, in seconds.
pub const NOTIFY_DEBOUNCE_SECONDS: i64 = 60;

/// At most one notify per entity per [`NOTIFY_DEBOUNCE_SECONDS`]. Owns no I/O
/// and no clock — the caller sends the push and says what time it is, so the
/// window is testable without sleeping through it.
#[derive(Debug, Default)]
pub struct NotifyThrottle {
    last_push_at: HashMap<String, i64>,
}

impl NotifyThrottle {
    /// Whether `entity_id` may push at `now` (unix seconds), recording the push
    /// when it may. Plan, run and task ids are disjoint (`plan-…` / `run-…` /
    /// `task-…`), so every entity shares the one map.
    ///
    /// A clock that stepped backwards fires and re-anchors the window rather
    /// than staying silent until it catches up.
    pub fn should_notify(&mut self, entity_id: &str, now: i64) -> bool {
        let within_window = self
            .last_push_at
            .get(entity_id)
            .is_some_and(|last| (0..NOTIFY_DEBOUNCE_SECONDS).contains(&(now - last)));
        if within_window {
            return false;
        }
        self.last_push_at.insert(entity_id.to_string(), now);
        true
    }
}

/// Sends signed notifies to the api. Cheap to clone (the reqwest
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

    /// POST one signed, freshly-timestamped notify for `task_id`/`kind`.
    pub async fn notify(&self, task_id: &str, kind: &str) -> Result<(), String> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;
        let request = build_notify_request(
            &self.device_id,
            &self.identity_private_key_b64,
            task_id,
            kind,
            timestamp,
        )?;
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

    #[test]
    fn challenge_matches_the_api_contract_and_binds_all_fields() {
        // The exact string buildapp.web_push.notify_challenge produces.
        let base = notify_challenge("dev-1", "task-1", "attention", 1_750_000_000);
        assert_eq!(base, "notify.dev-1.task-1.attention.1750000000");
        assert_ne!(
            base,
            notify_challenge("dev-2", "task-1", "attention", 1_750_000_000)
        );
        assert_ne!(
            base,
            notify_challenge("dev-1", "task-2", "attention", 1_750_000_000)
        );
        assert_ne!(
            base,
            notify_challenge("dev-1", "task-1", "other", 1_750_000_000)
        );
        assert_ne!(
            base,
            notify_challenge("dev-1", "task-1", "attention", 1_750_000_001)
        );
    }

    #[test]
    fn notify_request_signature_verifies_and_binds_the_timestamp() {
        let identity = transport::generate_identity_keypair();
        let request = build_notify_request(
            "dev-1",
            &identity.private_key_b64,
            "task-1",
            AGENT_KIND,
            1_750_000_000,
        )
        .expect("signable");
        assert_eq!(request.kind, AGENT_KIND);
        assert_eq!(request.task_id, "task-1");
        let challenge = notify_challenge(
            &request.device_id,
            &request.task_id,
            &request.kind,
            request.timestamp,
        );
        transport::verify_message_b64(
            &identity.public_key_b64,
            challenge.as_bytes(),
            &request.signature_b64,
        )
        .expect("notify signature verifies against the device identity key");

        // A replayed signature on a different timestamp must fail.
        let forged = notify_challenge(
            &request.device_id,
            &request.task_id,
            &request.kind,
            1_750_009_999,
        );
        assert!(transport::verify_message_b64(
            &identity.public_key_b64,
            forged.as_bytes(),
            &request.signature_b64,
        )
        .is_err());
    }

    #[test]
    fn notify_request_carries_only_id_and_kind_never_task_content() {
        // task_id + kind are allowed metadata; goals/plan text are not — the body
        // has exactly these five fields and nothing goal-shaped.
        let identity = transport::generate_identity_keypair();
        let request = build_notify_request(
            "dev-1",
            &identity.private_key_b64,
            "task-1",
            TASK_KIND,
            1_750_000_000,
        )
        .unwrap();
        let json = serde_json::to_value(&request).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["device_id", "kind", "signature_b64", "task_id", "timestamp"]
        );
    }

    /// A burst of attention events is one piece of news. The window opens
    /// again a minute later, so a genuinely new one still reaches the phone.
    #[test]
    fn throttle_pushes_once_per_entity_per_minute() {
        let mut throttle = NotifyThrottle::default();
        assert!(throttle.should_notify("run-1", 1_750_000_000));
        assert!(!throttle.should_notify("run-1", 1_750_000_001));
        assert!(!throttle.should_notify("run-1", 1_750_000_000 + NOTIFY_DEBOUNCE_SECONDS - 1));
        assert!(throttle.should_notify("run-1", 1_750_000_000 + NOTIFY_DEBOUNCE_SECONDS));
        // The window re-anchors on each push, rather than on the first one.
        assert!(!throttle.should_notify("run-1", 1_750_000_000 + NOTIFY_DEBOUNCE_SECONDS + 1));
    }

    #[test]
    fn throttle_tracks_entities_independently() {
        let mut throttle = NotifyThrottle::default();
        assert!(throttle.should_notify("run-1", 1_750_000_000));
        assert!(throttle.should_notify("run-2", 1_750_000_000));
        assert!(!throttle.should_notify("run-1", 1_750_000_000));
    }

    /// A clock that steps backwards must not silence an entity until it
    /// catches up — silence is the one failure the human cannot see.
    #[test]
    fn a_backwards_clock_fires_instead_of_going_quiet() {
        let mut throttle = NotifyThrottle::default();
        assert!(throttle.should_notify("run-1", 1_750_000_000));
        assert!(throttle.should_notify("run-1", 1_740_000_000));
        assert!(!throttle.should_notify("run-1", 1_740_000_010));
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
        notifier
            .notify("task-1", AGENT_KIND)
            .await
            .expect("notify accepted");

        let requests = server.received_requests().await.unwrap();
        let body: NotifyRequest = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body.device_id, "dev-1");
        assert_eq!(body.task_id, "task-1");
        assert_eq!(body.kind, AGENT_KIND);
        let challenge =
            notify_challenge(&body.device_id, &body.task_id, &body.kind, body.timestamp);
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
        let err = notifier.notify("task-1", AGENT_KIND).await.unwrap_err();
        assert!(err.contains("401"), "{err}");
    }
}
