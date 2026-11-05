use super::*;
use crate::secrets::{GrantScope, StageContext};
use dryoc::classic::crypto_box::crypto_box_seal;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

fn context() -> UploadContext {
    UploadContext {
        device_id: "device-test".into(),
        creator: "user-test".into(),
        request_id: "557ab895-85e0-4c94-82ad-f16fcb0dd11c".into(),
        name: "TEST_TOKEN".into(),
        ttl_seconds: Some(3600),
        grant: GrantScope::Project {
            project_id: "project-test".into(),
        },
        stage: StageContext {
            owner_id: "owner-test".into(),
            conversation_id: "conversation-test".into(),
            generation: "generation-test".into(),
            draft_id: None,
        },
    }
}

fn opener() -> SealedUploadOpener {
    SealedUploadOpener::new(&BASE64.encode([7u8; 32])).unwrap()
}

fn envelope() -> Value {
    json!({"domain": "build.secrets.upload", "version": 1, "context": context(), "value_b64": BASE64.encode("synthetic-secret-value")})
}

fn sealed_plain(plain: &[u8]) -> String {
    let secret = [7u8; 32];
    let mut public = [0; 32];
    crypto_scalarmult_base(&mut public, &secret);
    let mut ciphertext = vec![0; plain.len() + SEAL_BYTES];
    crypto_box_seal(&mut ciphertext, plain, &public).unwrap();
    BASE64.encode(ciphertext)
}

fn padded(value: &Value) -> Vec<u8> {
    let encoded = serde_json::to_vec(value).unwrap();
    let mut padded = vec![0; PADDED_BYTES];
    padded[..4].copy_from_slice(&(encoded.len() as u32).to_be_bytes());
    padded[4..4 + encoded.len()].copy_from_slice(&encoded);
    padded
}

fn open_javascript(value: &str) -> SecretValue {
    let mut public = [0; 32];
    crypto_scalarmult_base(&mut public, &[7u8; 32]);
    let input = json!({"deviceTransportPublicKeyB64": BASE64.encode(public), "context": context(), "value": value});
    let mut child = Command::new("node")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../transport/js/testing/secrets-upload.mjs"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&input).unwrap().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "JS sealing helper failed");
    let sealed = String::from_utf8(output.stdout).unwrap();
    opener().open(&sealed, &context()).unwrap()
}

#[test]
fn javascript_sealing_opens_exact_utf8_value() {
    let value = " synthetic-é-test\n ";
    assert_eq!(open_javascript(value).as_bytes(), value.as_bytes());
}

#[test]
fn javascript_maximum_size_newline_and_control_values_round_trip() {
    for value in ["\n".repeat(16 * 1024), "\u{1}".repeat(16 * 1024)] {
        assert_eq!(open_javascript(&value).as_bytes(), value.as_bytes());
    }
}

#[test]
fn every_bound_context_field_is_compared_with_authorized_outer_context() {
    let ciphertext = sealed_plain(&padded(&envelope()));
    let original = serde_json::to_value(context()).unwrap();
    let changes = [
        ("/device_id", json!("other-device")),
        ("/creator", json!("other-user")),
        ("/request_id", json!("557ab895-85e0-4c94-82ad-f16fcb0dd11d")),
        ("/name", json!("OTHER_TOKEN")),
        ("/ttl_seconds", Value::Null),
        ("/grant/project_id", json!("other-project")),
        (
            "/grant",
            json!({"scope":"agent", "agent_id":"agent", "conversation_id":"conversation", "generation":"generation"}),
        ),
        ("/stage/owner_id", json!("other-owner")),
        ("/stage/conversation_id", json!("other-conversation")),
        ("/stage/generation", json!("other-generation")),
        ("/stage/draft_id", json!("draft-test")),
    ];
    for (pointer, value) in changes {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let expected: UploadContext = serde_json::from_value(changed).unwrap();
        assert!(
            opener().open(&ciphertext, &expected).is_err(),
            "accepted changed {pointer}"
        );
    }
}

#[test]
fn ciphertext_tamper_wrong_key_and_wrong_size_are_refused() {
    let sealed = sealed_plain(&padded(&envelope()));
    let mut bytes = BASE64.decode(&sealed).unwrap();
    bytes[80] ^= 1;
    assert!(opener().open(&BASE64.encode(bytes), &context()).is_err());
    let wrong_key = SealedUploadOpener::new(&BASE64.encode([8u8; 32])).unwrap();
    assert!(wrong_key.open(&sealed, &context()).is_err());
    for invalid in [String::new(), "!".repeat(50000), BASE64.encode([0u8; 64])] {
        assert!(opener().open(&invalid, &context()).is_err());
    }
}

#[test]
fn domain_version_unknown_fields_and_value_limits_are_strict() {
    for (field, change) in [
        ("domain", json!("other.domain")),
        ("version", json!(2)),
        ("extra", json!(true)),
        ("value_b64", json!("")),
        ("value_b64", json!(BASE64.encode(b"has\0nul"))),
        ("value_b64", json!(BASE64.encode(vec![b'x'; 16385]))),
        ("value_b64", json!("malformed!")),
        ("value_b64", json!(BASE64.encode([0xff]))),
        ("value", json!("previous-unadvertised-format")),
    ] {
        let mut body = envelope();
        body[field] = change;
        assert!(
            opener()
                .open(&sealed_plain(&padded(&body)), &context())
                .is_err(),
            "accepted {field}"
        );
    }
}

#[test]
fn padding_must_have_exact_size_length_and_zero_tail() {
    let good = padded(&envelope());
    let mut nonzero = good.clone();
    nonzero[PADDED_BYTES - 1] = 1;
    let mut wrong_length = good.clone();
    wrong_length[..4].copy_from_slice(&(PADDED_BYTES as u32).to_be_bytes());
    for invalid in [
        nonzero,
        wrong_length,
        good[..100].to_vec(),
        vec![0; PADDED_BYTES],
    ] {
        assert!(opener().open(&sealed_plain(&invalid), &context()).is_err());
    }
}

#[test]
fn invalid_private_key_is_refused() {
    assert!(SealedUploadOpener::new("bad").is_err());
    assert!(SealedUploadOpener::new(&BASE64.encode([0; 31])).is_err());
}

#[test]
fn nullable_ttl_agent_grant_and_provisional_draft_round_trip() {
    let mut expected = context();
    expected.ttl_seconds = None;
    expected.stage.draft_id = Some("draft-synthetic".into());
    expected.grant = GrantScope::Agent {
        agent_id: "agent-synthetic".into(),
        conversation_id: expected.stage.conversation_id.clone(),
        generation: expected.stage.generation.clone(),
    };
    let mut body = envelope();
    body["context"] = serde_json::to_value(&expected).unwrap();
    body["value_b64"] = json!(BASE64.encode(vec![b'x'; 16 * 1024]));
    let value = opener()
        .open(&sealed_plain(&padded(&body)), &expected)
        .unwrap();
    assert_eq!(value.as_bytes().len(), 16 * 1024);
}

#[test]
fn upload_context_enforces_name_uuid_and_ttl_limits() {
    let original = serde_json::to_value(context()).unwrap();
    for (field, changed) in [
        ("name", json!("invalid name")),
        ("name", json!("x".repeat(65))),
        ("request_id", json!("not-a-uuid")),
        ("ttl_seconds", json!(0)),
        ("ttl_seconds", json!(59)),
        ("ttl_seconds", json!(2_592_001)),
    ] {
        let mut body = original.clone();
        body[field] = changed;
        let invalid: UploadContext = serde_json::from_value(body).unwrap();
        assert!(invalid.validate().is_err(), "accepted invalid {field}");
    }
}
