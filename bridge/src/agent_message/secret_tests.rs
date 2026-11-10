use crate::models::AgentProvider;
use crate::operation::OperationPayload;

fn payload() -> OperationPayload {
    serde_json::from_value(serde_json::json!({
        "start_sequence": 1,
        "end_sequence": 1,
        "messages": [{
            "id": "message-1", "sequence": 1, "role": "user", "body": "deploy",
            "created_at": "2026-11-04T00:00:00Z",
            "secrets": [{"name": "TEST_TOKEN", "secret_id": "sec_test", "scope": "project", "ttl_seconds": null, "expires_at": null}]
        }],
        "prior_context": "",
        "secret_environment": "dev"
    })).unwrap()
}

#[test]
fn secret_metadata_block_uses_exact_revision_words_and_harness_wrapper() {
    let block = "Attached secrets (values are not included):\n\
{\"name\":\"TEST_TOKEN\",\"secret_id\":\"sec_test\",\"scope\":\"project\",\"ttl_seconds\":null,\"expires_at\":null}\n\
Read a value with: buildctl --env dev secrets get --id <secret_id>\n\
Access depends on each secret's scope (environment, project or agent), its expiry and the current conversation's authorization.\n\
Printing, logging, or committing secret values is dangerous. A CLI read prints the value and may expose it in tool output, transcripts or terminal history. Retrieve only when needed and avoid unnecessary disclosure.";
    for provider in super::golden::HARNESSES.map(|(provider, _)| provider) {
        let prompt = payload().delivery_prompt("op-test", false, provider);
        let wrapped = crate::harness::harness_for(provider).wrap_attachment(block);
        assert!(prompt.contains(&wrapped), "{provider:?}: {prompt}");
    }
}

#[test]
fn missing_environment_never_assumes_production_for_secret_delivery() {
    let mut value = serde_json::to_value(payload()).unwrap();
    value.as_object_mut().unwrap().remove("secret_environment");
    let payload: OperationPayload = serde_json::from_value(value).unwrap();
    let prompt = payload.delivery_prompt("op-test", false, AgentProvider::Codex);
    assert!(!prompt.contains("buildctl --env prod"));
}

#[test]
fn production_channel_is_frozen_in_the_delivery_and_survives_roundtrip() {
    let mut payload = payload();
    payload.secret_environment = Some(crate::thread::SecretEnvironment::Prod);
    let saved = serde_json::to_string(&payload).unwrap();
    let restored: OperationPayload = serde_json::from_str(&saved).unwrap();
    let prompt = restored.delivery_prompt("op-test", true, AgentProvider::Claude);
    assert!(prompt.contains("buildctl --env prod secrets get --id <secret_id>"));
    assert!(!prompt.contains("buildctl --env dev"));
}

#[test]
fn descriptors_are_json_escaped_without_rescanning_template_placeholders() {
    let mut payload = payload();
    payload.messages[0].secrets[0].name = "SYNTHETIC_\"\n{environment}".into();
    let prompt = payload.delivery_prompt("op-test", false, AgentProvider::Pi);
    assert!(prompt.contains(r#"{"name":"SYNTHETIC_\"\n{environment}","secret_id":"sec_test","scope":"project","ttl_seconds":null,"expires_at":null}"#));
}

#[test]
fn secret_history_descriptors_refuse_plaintext_and_upload_fields() {
    for key in ["value", "sealed_payload", "path", "content_b64"] {
        let mut descriptor = serde_json::to_value(&payload().messages[0].secrets[0]).unwrap();
        descriptor[key] = serde_json::json!("synthetic-sensitive-data");
        assert!(serde_json::from_value::<crate::thread::MessageSecret>(descriptor).is_err());
    }
}

#[test]
fn expiry_before_delivery_preserves_the_frozen_descriptor_for_every_harness() {
    let mut payload = payload();
    let descriptor = &mut payload.messages[0].secrets[0];
    descriptor.ttl_seconds = Some(60);
    descriptor.expires_at = Some("2020-01-01T00:01:00Z".into());
    let frozen = serde_json::to_string(&payload).unwrap();
    let expected_row = serde_json::to_string(&payload.messages[0].secrets[0]).unwrap();
    for provider in super::golden::HARNESSES.map(|(provider, _)| provider) {
        let restored: OperationPayload = serde_json::from_str(&frozen).unwrap();
        for cold in [false, true] {
            let delivered = restored.delivery_prompt("op-delayed", cold, provider);
            assert!(
                delivered.contains(&expected_row),
                "{provider:?}: {delivered}"
            );
            assert!(delivered.contains("A CLI read prints the value and may expose it"));
            assert_eq!(serde_json::to_string(&restored).unwrap(), frozen);
        }
    }
}
