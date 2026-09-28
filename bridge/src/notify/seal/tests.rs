//! The seal held to the cross-language vector (`fixtures/push/sealed-v1.json`)
//! and to the scheme's bindings and caps.

use super::*;
use serde_json::Value;

fn vector() -> Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/push/sealed-v1.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn secret_from_jwk(jwk: &Value) -> SecretKey {
    let d = URL_SAFE_NO_PAD.decode(jwk["d"].as_str().unwrap()).unwrap();
    SecretKey::from_slice(&d).unwrap()
}

fn fixture_binding(vector: &Value) -> Binding<'_> {
    Binding {
        subscription_id: vector["subscription_id"].as_str().unwrap(),
        kind: vector["kind"].as_str().unwrap(),
        entity_id: vector["entity_id"].as_str().unwrap(),
    }
}

fn fixture_content(vector: &Value) -> (PushContent, i64) {
    let plaintext: Value = serde_json::from_str(vector["plaintext"].as_str().unwrap()).unwrap();
    let content = PushContent {
        title: plaintext["title"].as_str().unwrap().into(),
        body: plaintext["body"].as_str().unwrap().into(),
        url: plaintext["url"].as_str().unwrap().into(),
    };
    (content, plaintext["iat"].as_i64().unwrap())
}

fn fixture_nonce(vector: &Value) -> [u8; NONCE_LEN] {
    URL_SAFE_NO_PAD
        .decode(vector["nonce"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}

fn random_recipient() -> (SecretKey, String) {
    let secret = SecretKey::random(&mut OsRng);
    let public = URL_SAFE_NO_PAD.encode(uncompressed(&secret.public_key()));
    (secret, public)
}

fn content() -> PushContent {
    PushContent::new(
        "#12 Fix the banner",
        "Rail scroll: ready for review",
        "/app/#/tasks/task-1".into(),
    )
    .unwrap()
}

const SID: &str = "fktz0HaQPrTBONQORgaYdrkQLm41LU5PVktHaqho6To";

fn binding() -> Binding<'static> {
    Binding {
        subscription_id: SID,
        kind: "task",
        entity_id: "task-1",
    }
}

/// Rust reproduces Python's blob byte for byte from the same ephemeral key
/// and nonce: the three implementations agree on the scheme (control 3).
#[test]
fn the_seal_reproduces_the_fixture_blob_byte_for_byte() {
    let vector = vector();
    let (content, iat) = fixture_content(&vector);
    let blob = seal_with(
        vector["recipient_public_key"].as_str().unwrap(),
        fixture_binding(&vector),
        &content,
        iat,
        &secret_from_jwk(&vector["ephemeral_private_jwk"]),
        fixture_nonce(&vector),
    )
    .unwrap();
    assert_eq!(blob, vector["blob"].as_str().unwrap());
}

#[test]
fn the_fixture_blob_opens_to_the_fixture_plaintext() {
    let vector = vector();
    let opened = open(
        &secret_from_jwk(&vector["recipient_private_jwk"]),
        fixture_binding(&vector),
        vector["blob"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(opened).unwrap(),
        vector["plaintext"].as_str().unwrap()
    );
}

/// A blob moved to another subscription, or pasted beside another entity or
/// kind, fails the tag (control 4).
#[test]
fn changing_any_one_of_sid_kind_or_entity_fails_to_open() {
    let vector = vector();
    let recipient = secret_from_jwk(&vector["recipient_private_jwk"]);
    let blob = vector["blob"].as_str().unwrap();
    let bound = fixture_binding(&vector);
    let other_sid = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
    for moved in [
        Binding {
            subscription_id: other_sid,
            ..bound
        },
        Binding {
            kind: "task",
            ..bound
        },
        Binding {
            entity_id: "run-other",
            ..bound
        },
    ] {
        assert_eq!(
            open(&recipient, moved, blob),
            Err(SealError::Cipher),
            "{moved:?}"
        );
    }
    open(&recipient, bound, blob).expect("the untouched binding still opens");
}

#[test]
fn a_random_seal_round_trips_and_is_fresh_every_time() {
    let (secret, public) = random_recipient();
    let first = seal(&public, binding(), &content(), 1_790_000_000).unwrap();
    let second = seal(&public, binding(), &content(), 1_790_000_000).unwrap();
    assert_ne!(first, second, "a fresh ephemeral key and nonce per blob");
    let opened: Value = serde_json::from_slice(&open(&secret, binding(), &first).unwrap()).unwrap();
    assert_eq!(
        opened,
        serde_json::json!({
            "v": 1,
            "title": "#12 Fix the banner",
            "body": "Rail scroll: ready for review",
            "url": "/app/#/tasks/task-1",
            "iat": 1_790_000_000,
        })
    );
    let (other, _) = random_recipient();
    assert!(
        open(&other, binding(), &first).is_err(),
        "another key cannot open it"
    );
}

/// The largest content the caps allow — every character four bytes — still
/// fits the plaintext and blob caps.
#[test]
fn content_at_its_caps_fits_the_plaintext_and_the_blob() {
    let (_, public) = random_recipient();
    let wide = PushContent::new(
        &"🚀".repeat(200),
        &"🚀".repeat(400),
        crate::notify::content::task_url("task-01ARZ3NDEKTSV4RRFFQ69G5FAV"),
    )
    .unwrap();
    let blob = seal(&public, binding(), &wide, 1_790_000_000).unwrap();
    assert!(blob.len() <= BLOB_MAX_CHARS, "{}", blob.len());
}

#[test]
fn plaintext_over_its_cap_is_refused() {
    let (_, public) = random_recipient();
    let oversized = PushContent {
        title: "t".into(),
        body: "b".into(),
        url: format!("/app/#/{}", "x".repeat(PLAINTEXT_MAX_BYTES)),
    };
    assert_eq!(
        seal(&public, binding(), &oversized, 1),
        Err(SealError::PlaintextTooLarge)
    );
}

#[test]
fn a_key_that_is_not_an_uncompressed_p256_point_is_refused() {
    let (secret, public) = random_recipient();
    let compressed = URL_SAFE_NO_PAD.encode(secret.public_key().to_encoded_point(true).as_bytes());
    let mut off_curve = URL_SAFE_NO_PAD.decode(&public).unwrap();
    off_curve[64] ^= 1;
    for bad in [
        "not base64!".to_string(),
        compressed,
        URL_SAFE_NO_PAD.encode(off_curve),
        URL_SAFE_NO_PAD.encode([4u8; 65]),
        String::new(),
    ] {
        assert_eq!(
            parse_public_key(&bad).err(),
            Some(SealError::BadKey),
            "{bad}"
        );
        assert_eq!(seal(&bad, binding(), &content(), 1), Err(SealError::BadKey));
    }
    parse_public_key(&public).expect("a real uncompressed point");
}

#[test]
fn a_subscription_id_is_43_base64url_characters_of_32_bytes() {
    assert!(is_subscription_id(SID));
    assert!(!is_subscription_id(&SID[..42]));
    assert!(!is_subscription_id(&format!("{SID}A")));
    assert!(!is_subscription_id(
        "fktz0HaQPrTBONQORgaYdrkQLm41LU5PVktHaqho6T+"
    ));
    assert!(!is_subscription_id(""));
}

#[test]
fn errors_name_the_failure_and_nothing_else() {
    for error in [
        SealError::BadKey,
        SealError::PlaintextTooLarge,
        SealError::BlobTooLarge,
        SealError::Cipher,
    ] {
        let said = error.to_string();
        assert!(!said.contains("banner") && !said.contains(SID), "{said}");
    }
}
