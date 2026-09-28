//! One notify end to end against a mock api: what leaves the bridge is ids
//! and ciphertext, a key that cannot be sealed to never stops the notify, and
//! the keys the api reports unknown are forgotten.

use super::*;
use crate::notify::content::task_url;
use crate::notify::seal::{open, Binding};
use crate::notify::{notify_challenge_for, NotifyRequest, TASK_KIND};
use crate::transport;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::SecretKey;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TITLE: &str = "#12 Banner fades too early";
const BODY: &str = "Rail scroll: the fade now waits for the copy";
const ENTITY: &str = "task-01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn sid(n: u8) -> String {
    URL_SAFE_NO_PAD.encode([n; 32])
}

fn recipient() -> (SecretKey, String) {
    let secret = SecretKey::random(&mut aes_gcm::aead::OsRng);
    let public = URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(false).as_bytes());
    (secret, public)
}

fn content() -> PushContent {
    PushContent::new(TITLE, BODY, task_url(ENTITY)).unwrap()
}

fn delivery(content: Option<PushContent>) -> Delivery {
    Delivery {
        entity_id: ENTITY.to_string(),
        kind: TASK_KIND,
        content,
    }
}

struct Api {
    server: MockServer,
    identity: transport::KeyPairB64,
    notifier: Notifier,
}

async fn api(reply: ResponseTemplate) -> Api {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/push/notify"))
        .respond_with(reply)
        .expect(1)
        .mount(&server)
        .await;
    let identity = transport::generate_identity_keypair();
    let notifier = Notifier::new(&server.uri(), "dev-1", &identity.private_key_b64);
    Api {
        server,
        identity,
        notifier,
    }
}

fn accepted() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "delivered": 1,
        "pruned": 0,
        "unknown_subscriptions": [],
    }))
}

async fn posted(api: &Api) -> (String, NotifyRequest) {
    let requests = api.server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let raw = String::from_utf8(requests[0].body.clone()).unwrap();
    let request = serde_json::from_str(&raw).unwrap();
    (raw, request)
}

fn verify(api: &Api, request: &NotifyRequest) {
    let challenge = notify_challenge_for(
        &request.device_id,
        &request.task_id,
        &request.kind,
        request.timestamp,
        &request.sealed,
    );
    transport::verify_message_b64(
        &api.identity.public_key_b64,
        challenge.as_bytes(),
        &request.signature_b64,
    )
    .expect("the posted signature verifies");
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    (dir, store)
}

fn stored_sids(store: &Store) -> Vec<String> {
    let mut sids: Vec<String> = store
        .list_push_keys()
        .unwrap()
        .into_iter()
        .map(|key| key.subscription_id)
        .collect();
    sids.sort();
    sids
}

/// Control 1: the api sees sids and ciphertext. The title, the body and the
/// deep link are nowhere in the request, each blob opens to them for its own
/// subscription, and the signature covers the entries (v2).
#[tokio::test]
async fn sealed_entries_carry_the_words_and_the_request_carries_none() {
    let (_dir, store) = store();
    let (first_secret, first_public) = recipient();
    let (second_secret, second_public) = recipient();
    store.upsert_push_key(&sid(1), &first_public, 1).unwrap();
    store.upsert_push_key(&sid(2), &second_public, 2).unwrap();
    let api = api(accepted()).await;

    api.notifier
        .deliver(delivery(Some(content())), Some(store.clone()))
        .await
        .expect("delivered");

    let (raw, request) = posted(&api).await;
    for words in [TITLE, BODY, "Banner", "fade", "/app/#/", "tasks/"] {
        assert!(!raw.contains(words), "{words:?} left the bridge: {raw}");
    }
    assert!(!raw.contains(&first_public) && !raw.contains(&second_public));
    verify(&api, &request);
    assert_eq!(request.sealed.len(), 2);
    for (secret, entry) in [
        (&second_secret, &request.sealed[0]),
        (&first_secret, &request.sealed[1]),
    ] {
        let binding = Binding {
            subscription_id: &entry.subscription_id,
            kind: TASK_KIND,
            entity_id: ENTITY,
        };
        let opened: serde_json::Value =
            serde_json::from_slice(&open(secret, binding, &entry.blob).unwrap()).unwrap();
        assert_eq!(opened["title"], TITLE);
        assert_eq!(opened["body"], BODY);
        assert_eq!(opened["url"], task_url(ENTITY));
        assert!(opened["iat"].as_i64().unwrap() >= request.timestamp - 5);
    }
}

/// The api reports the sealed subscriptions it no longer has; the bridge
/// forgets exactly those of the keys it sealed to, and nothing else.
#[tokio::test]
async fn the_keys_the_api_reports_unknown_are_forgotten() {
    let (_dir, store) = store();
    let (_, public) = recipient();
    store.upsert_push_key(&sid(1), &public, 1).unwrap();
    store.upsert_push_key(&sid(2), &public, 2).unwrap();
    store.upsert_push_key(&sid(3), "not-a-key", 3).unwrap();
    let api = api(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "delivered": 1,
        "pruned": 1,
        "unknown_subscriptions": [sid(1), sid(3), sid(9)],
    })))
    .await;

    api.notifier
        .deliver(delivery(Some(content())), Some(store.clone()))
        .await
        .unwrap();

    assert_eq!(
        stored_sids(&store),
        vec![sid(2), sid(3)],
        "sid 1 forgotten; sid 3 was never sealed to, so the api cannot speak for it"
    );
}

/// Control 8: a key that cannot be sealed to is skipped, and a bridge whose
/// every key is bad still sends the notify, generic.
#[tokio::test]
async fn a_bad_stored_key_falls_back_to_generic_and_the_notify_still_posts() {
    let (_dir, store) = store();
    store.upsert_push_key(&sid(1), "not-a-key", 1).unwrap();
    let api = api(accepted()).await;

    api.notifier
        .deliver(delivery(Some(content())), Some(store.clone()))
        .await
        .expect("the notify still goes out");

    let (raw, request) = posted(&api).await;
    assert!(request.sealed.is_empty());
    assert!(!raw.contains("sealed"), "{raw}");
    verify(&api, &request);
}

#[tokio::test]
async fn a_bad_key_beside_a_good_one_costs_only_its_own_entry() {
    let (_dir, store) = store();
    let (_, public) = recipient();
    store.upsert_push_key(&sid(1), "not-a-key", 1).unwrap();
    store.upsert_push_key(&sid(2), &public, 2).unwrap();
    let api = api(accepted()).await;

    api.notifier
        .deliver(delivery(Some(content())), Some(store.clone()))
        .await
        .unwrap();

    let (_, request) = posted(&api).await;
    let sealed: Vec<&str> = request
        .sealed
        .iter()
        .map(|entry| entry.subscription_id.as_str())
        .collect();
    assert_eq!(sealed, vec![sid(2)]);
}

/// No keys, no store, or no content: the request is the #191 one exactly —
/// five fields, signed over the #191 challenge.
#[tokio::test]
async fn without_keys_or_content_the_request_is_the_191_shape() {
    let (_dir, empty) = store();
    let (_keyed_dir, keyed) = store();
    let (_, public) = recipient();
    keyed.upsert_push_key(&sid(1), &public, 1).unwrap();
    for (content, keys) in [
        (Some(content()), Some(empty.clone())),
        (Some(content()), None),
        (None, Some(keyed.clone())),
    ] {
        let api = api(accepted()).await;
        api.notifier.deliver(delivery(content), keys).await.unwrap();
        let (raw, request) = posted(&api).await;
        let body: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let fields: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            ["device_id", "kind", "signature_b64", "task_id", "timestamp"]
        );
        transport::verify_message_b64(
            &api.identity.public_key_b64,
            crate::notify::notify_challenge("dev-1", ENTITY, TASK_KIND, request.timestamp)
                .as_bytes(),
            &request.signature_b64,
        )
        .expect("the #191 challenge verifies");
    }
}

/// A refused notify forgets nothing and says only why it was refused.
#[tokio::test]
async fn a_refused_notify_forgets_no_key() {
    let (_dir, store) = store();
    let (_, public) = recipient();
    store.upsert_push_key(&sid(1), &public, 1).unwrap();
    let api = api(ResponseTemplate::new(401)).await;

    let error = api
        .notifier
        .deliver(delivery(Some(content())), Some(store.clone()))
        .await
        .unwrap_err();

    assert!(error.contains("401"), "{error}");
    assert!(!error.contains(TITLE) && !error.contains(BODY));
    assert_eq!(stored_sids(&store), vec![sid(1)]);
}

/// The api refuses a sid listed twice, so a key list that names one twice
/// seals to it once.
#[test]
fn a_subscription_is_sealed_to_once() {
    let (_, public) = recipient();
    let key = crate::store::push_keys::PushKey {
        subscription_id: sid(1),
        public_key: public,
    };
    let sealed = seal_to_keys(&[key.clone(), key], ENTITY, TASK_KIND, &content(), 1);
    assert_eq!(sealed.len(), 1);
}
