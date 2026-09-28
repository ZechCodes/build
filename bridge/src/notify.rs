//! Push notifications — bridge → api → web push, content sealed end to end.
//!
//! A push follows the unread counter (#191): it fires when something adds to
//! the badge the inbox wears, and at no other time. Two things add to it — an
//! attention-class item on a watched agent's conversation, and news on a
//! watched, unfinished task — and each has its kind. The bridge POSTs a
//! signed, timestamped notify to the api's `/api/push/notify`, which fans a push
//! out to the owner's browsers.
//!
//! What the api sees (#200): the device, the **opaque entity id**, the
//! **generic kind** (`agent`/`task`), and one `sealed` entry per registered
//! notification key — a subscription id and a blob. The words a push shows (a
//! task's title and news, an agent's name and the first line it said, the deep
//! link) exist outside this bridge only inside those blobs, sealed to each
//! browser's notification key ([`seal`], the scheme in
//! `planning/v2/Push Content Security Checklist.md`). No title, body or url
//! text is ever sent in the clear or written to a log; errors name only the
//! kind of failure.
//!
//! The seal is ECIES with an ephemeral sender key, so it is not
//! sender-authenticated: anyone holding a subscription's notification public
//! key can seal content that opens. Forgery by the api or the push service is
//! prevented only because that public key stays secret — it is generated in
//! the browser and travels only over the E2EE session (`push.registerKey`),
//! never through the api.
//!
//! Content is best effort and never the notify's condition: a key that cannot
//! be sealed to is skipped, content that cannot be built means no blobs, and
//! either way the notify goes out on time and the browser shows the #191
//! generic copy.
//!
//! Authentication is the registration scheme reused: an Ed25519 signature over
//! [`notify_challenge_for`], which mirrors `skriftapp/buildapp/web_push.py`
//! byte-for-byte and, with `sealed` present, covers every entry through a
//! digest; the timestamp bounds replay of a captured request.
//!
//! Throttling: [`NotifyThrottle`] fires **at most one notify per entity per
//! [`NOTIFY_DEBOUNCE_SECONDS`]**. News arrives in bursts — an agent's reply and
//! the report behind it are one piece of news — so the window is what keeps a
//! phone quiet.

pub mod content;
mod delivery;
pub mod seal;

pub use delivery::Delivery;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

/// One subscription's sealed content: which subscription, and the blob only
/// its browser can open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedEntry {
    pub subscription_id: String,
    pub blob: String,
}

/// The payload POSTed to `/api/push/notify`. Carries the opaque `task_id` and the
/// generic `kind` (both allowed metadata under the E2EE contract) so the api can
/// build a deep-linked, kind-specific push, and the sealed content beside them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyRequest {
    pub device_id: String,
    pub task_id: String,
    pub kind: String,
    /// Unix seconds; the api rejects requests outside its freshness window.
    pub timestamp: i64,
    /// Ed25519 signature (padded b64) over [`notify_challenge_for`].
    pub signature_b64: String,
    /// One entry per key sealed to. Left off the wire when empty: the api
    /// refuses `null`, and a bridge with no keys sends the #191 shape exactly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sealed: Vec<SealedEntry>,
}

/// The #191 message signed for a notify. Binds the device, the task, the
/// kind, and the timestamp so a captured signature cannot be replayed onto a
/// different notification. Mirrors `buildapp.web_push.notify_challenge` exactly.
pub fn notify_challenge(device_id: &str, task_id: &str, kind: &str, timestamp: i64) -> String {
    format!("notify.{device_id}.{task_id}.{kind}.{timestamp}")
}

/// The message signed for a notify carrying `sealed`: the #191 challenge,
/// then the lowercase hex SHA-256 of every `"{subscription_id}:{blob}\n"` in
/// request order. Without entries it is the #191 challenge unchanged.
pub fn notify_challenge_for(
    device_id: &str,
    task_id: &str,
    kind: &str,
    timestamp: i64,
    sealed: &[SealedEntry],
) -> String {
    let base = notify_challenge(device_id, task_id, kind, timestamp);
    if sealed.is_empty() {
        return base;
    }
    format!("{base}.{}", sealed_digest(sealed))
}

fn sealed_digest(sealed: &[SealedEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in sealed {
        hasher.update(format!("{}:{}\n", entry.subscription_id, entry.blob));
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Build a fully-signed [`NotifyRequest`] for `task_id`/`kind` at `timestamp`
/// (unix seconds), carrying `sealed`. Pure given its inputs.
pub fn build_notify_request(
    device_id: &str,
    identity_private_key_b64: &str,
    task_id: &str,
    kind: &str,
    timestamp: i64,
    sealed: Vec<SealedEntry>,
) -> Result<NotifyRequest, String> {
    let challenge = notify_challenge_for(device_id, task_id, kind, timestamp, &sealed);
    let signature_b64 = transport::sign_message_b64(identity_private_key_b64, challenge.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(NotifyRequest {
        device_id: device_id.to_string(),
        task_id: task_id.to_string(),
        kind: kind.to_string(),
        timestamp,
        signature_b64,
        sealed,
    })
}

/// What the api answered a notify with, as far as the bridge needs it: the
/// sealed subscription ids that match none of the owner's subscriptions,
/// whose keys the bridge then forgets.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NotifyReply {
    pub unknown_subscriptions: Vec<String>,
}

impl NotifyReply {
    /// Read leniently: `delivered` and `pruned` are the api's to count, and a
    /// field that is missing, `null` or mistyped means none — the notify was
    /// accepted either way.
    pub fn parse(body: &[u8]) -> NotifyReply {
        let value: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let unknown_subscriptions = value
            .get("unknown_subscriptions")
            .and_then(serde_json::Value::as_array)
            .map(|sids| {
                sids.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        NotifyReply {
            unknown_subscriptions,
        }
    }
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

    /// The device this notifier signs for: the one every deep link names.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// POST one signed, freshly-timestamped notify for `task_id`/`kind`,
    /// carrying `sealed`, and read what the api answered.
    pub async fn notify(
        &self,
        task_id: &str,
        kind: &str,
        sealed: Vec<SealedEntry>,
    ) -> Result<NotifyReply, String> {
        let request = build_notify_request(
            &self.device_id,
            &self.identity_private_key_b64,
            task_id,
            kind,
            unix_seconds(),
            sealed,
        )?;
        let url = format!("{}/api/push/notify", self.api_url);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("api rejected notify: {}", response.status()));
        }
        let body = response.bytes().await.unwrap_or_default();
        Ok(NotifyReply::parse(&body))
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
            Vec::new(),
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
            Vec::new(),
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

    fn challenge_vector() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/push/notify-challenge-v2.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn vector_entries(vector: &serde_json::Value) -> Vec<SealedEntry> {
        serde_json::from_value(vector["sealed"].clone()).unwrap()
    }

    /// With `sealed` present the signature covers every entry, in order,
    /// through the digest the api recomputes (`notify-challenge-v2.json`).
    #[test]
    fn the_v2_challenge_matches_the_fixture() {
        let vector = challenge_vector();
        let challenge = notify_challenge_for(
            vector["device_id"].as_str().unwrap(),
            vector["entity_id"].as_str().unwrap(),
            vector["kind"].as_str().unwrap(),
            vector["timestamp"].as_i64().unwrap(),
            &vector_entries(&vector),
        );
        assert_eq!(challenge, vector["challenge"].as_str().unwrap());
    }

    /// Without `sealed` the #191 challenge is unchanged, and reordering the
    /// entries changes the digest.
    #[test]
    fn the_challenge_without_sealed_is_the_191_one_and_order_is_signed() {
        assert_eq!(
            notify_challenge_for("dev-1", "task-1", "task", 1_790_000_000, &[]),
            notify_challenge("dev-1", "task-1", "task", 1_790_000_000)
        );
        let vector = challenge_vector();
        let mut entries = vector_entries(&vector);
        entries.reverse();
        assert_ne!(
            notify_challenge_for("dev-1", "task-1", "task", 1_790_000_000, &entries),
            vector["challenge"].as_str().unwrap()
        );
    }

    #[test]
    fn a_sealed_request_signs_the_v2_challenge() {
        let identity = transport::generate_identity_keypair();
        let entries = vector_entries(&challenge_vector());
        let request = build_notify_request(
            "dev-1",
            &identity.private_key_b64,
            "task-1",
            TASK_KIND,
            1_790_000_000,
            entries.clone(),
        )
        .unwrap();
        assert_eq!(request.sealed, entries);
        let challenge = notify_challenge_for("dev-1", "task-1", TASK_KIND, 1_790_000_000, &entries);
        transport::verify_message_b64(
            &identity.public_key_b64,
            challenge.as_bytes(),
            &request.signature_b64,
        )
        .expect("the v2 challenge verifies");
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["sealed"][0]["blob"], "AQID");
    }

    /// The reply is read leniently: a field missing, null or mistyped means
    /// none, and a reply that is not JSON at all still counts as delivered.
    #[test]
    fn the_notify_reply_is_read_leniently() {
        let unknown = |body: &str| NotifyReply::parse(body.as_bytes()).unknown_subscriptions;
        assert_eq!(
            unknown(r#"{"delivered":1,"pruned":0,"unknown_subscriptions":["a","b"]}"#),
            vec!["a", "b"]
        );
        assert!(unknown("{}").is_empty());
        assert!(unknown(r#"{"unknown_subscriptions":null}"#).is_empty());
        assert!(unknown(r#"{"unknown_subscriptions":"a"}"#).is_empty());
        assert_eq!(
            unknown(r#"{"unknown_subscriptions":["a",5,null]}"#),
            vec!["a"]
        );
        assert!(unknown("").is_empty());
        assert!(unknown("<html>").is_empty());
    }

    /// Control 1, held in the source: no log line on the push path formats
    /// the content, a key, or anything carrying them. Each log call names a
    /// kind of failure and at most an error value.
    #[test]
    fn no_log_line_on_the_push_path_formats_content_or_keys() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = [
            "notify.rs",
            "notify/content.rs",
            "notify/seal.rs",
            "notify/delivery.rs",
            "app/board/agent_push.rs",
            "app/board/attention.rs",
            "app/tracker/push.rs",
            "app/push_keys.rs",
            "store/push_keys.rs",
        ];
        let forbidden = [
            "content",
            "title",
            "body",
            "url",
            "public_key",
            "key.",
            "blob",
            "plaintext",
            "delivery",
            "said",
            "line",
        ];
        let mut calls = 0;
        for file in files {
            let source = std::fs::read_to_string(root.join(file)).unwrap();
            for (at, _) in source
                .match_indices("eprintln!(")
                .chain(source.match_indices("log::"))
            {
                let call = &source[at..];
                let call = &call[..call.find(");").unwrap_or(call.len())];
                calls += 1;
                let arguments = call.split_once('"').map_or("", |(_, rest)| rest);
                let formatted: Vec<&str> = arguments
                    .split(['{', '}'])
                    .skip(1)
                    .step_by(2)
                    .chain(arguments.rsplit_once('"').map(|(_, tail)| tail))
                    .collect();
                for word in forbidden {
                    assert!(
                        !formatted.iter().any(|part| part.contains(word)),
                        "{file}: a log call formats `{word}`: {call}"
                    );
                }
            }
        }
        assert!(calls > 5, "the scan found the log calls: {calls}");
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
            .notify("task-1", AGENT_KIND, Vec::new())
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
        let err = notifier
            .notify("task-1", AGENT_KIND, Vec::new())
            .await
            .unwrap_err();
        assert!(err.contains("401"), "{err}");
    }
}
