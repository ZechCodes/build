use super::*;

const SENTINEL: &str = "SYNTHETIC_SECRET_NAME_DO_NOT_LOG";

fn stage() -> StageContext {
    StageContext {
        owner_id: SENTINEL.into(),
        conversation_id: SENTINEL.into(),
        generation: SENTINEL.into(),
        draft_id: Some(SENTINEL.into()),
    }
}

fn context() -> SecretContext {
    SecretContext {
        id: format!("sec_{}", uuid::Uuid::new_v4()),
        device_id: SENTINEL.into(),
        creator: SENTINEL.into(),
        name: SENTINEL.into(),
        ttl_seconds: None,
        grant: GrantScope::Project {
            project_id: SENTINEL.into(),
        },
        stage: stage(),
    }
}

#[test]
fn stage_debug_does_not_format_arbitrary_staging_context() {
    let debug = format!("{:?}", stage());
    assert!(debug.contains("StageContext"));
    assert!(!debug.contains(SENTINEL));
}

#[test]
fn context_debug_redacts_names_and_nested_metadata() {
    let context = context();
    let context_debug = format!("{context:?}");
    assert!(context_debug.contains("SecretContext"));
    assert!(!context_debug.contains(SENTINEL));
}

#[test]
fn nested_record_debug_redacts_names_and_arbitrary_record_metadata() {
    let record = SecretRecord {
        context: context(),
        created_at: 1000,
        attached_at: None,
        expires_at: None,
        revoked_at: None,
        revocation_reason: Some(SENTINEL.into()),
        revision: 1,
        message_operation_id: Some(SENTINEL.into()),
        upload_request_id: SENTINEL.into(),
        upload_digest: SENTINEL.into(),
        blob_present: true,
    };
    let record_debug = format!("{record:?}");
    assert!(record_debug.contains("SecretRecord"));
    assert!(!record_debug.contains(SENTINEL));
    assert!(!format!("{record:#?}").contains(SENTINEL));
}
