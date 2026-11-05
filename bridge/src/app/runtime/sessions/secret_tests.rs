use super::registry::SessionRegistry;
use crate::secrets::SecretCaller;

pub(super) fn caller() -> SecretCaller {
    SecretCaller {
        device_id: "synthetic-device".into(),
        channel: "dev".into(),
        project_id: "project-a".into(),
        owner_id: "owner-a".into(),
        canonical_owner_id: "owner-a".into(),
        agent_id: "agent-a".into(),
        conversation_id: "agent-a".into(),
        thread_id: "thread-a".into(),
        generation: "thread-a".into(),
        generation_revision: 1,
        session_incarnation: "synthetic-incarnation".into(),
    }
}

#[test]
fn secret_capabilities_rotate_independently_and_revoke_with_the_session() {
    let mut registry = SessionRegistry::new();
    let authority = registry.secret_authority();
    registry.install_secret_token("old-secret".into(), caller());
    assert!(authority.authenticate("agent-a", "old-secret").is_some());
    assert!(authority.authenticate("agent-a", "mcp-token").is_none());
    let mut next = caller();
    next.session_incarnation = "next-incarnation".into();
    registry.install_secret_token("new-secret".into(), next);
    assert!(authority.authenticate("agent-a", "old-secret").is_none());
    assert!(authority.authenticate("agent-a", "new-secret").is_some());
    registry.revoke_mcp_token("agent-a");
    assert!(authority.authenticate("agent-a", "new-secret").is_none());
}

#[test]
fn secret_capabilities_check_exact_agent_and_token_bytes() {
    let mut registry = SessionRegistry::new();
    registry.install_secret_token("synthetic-token".into(), caller());
    let authority = registry.secret_authority();
    for supplied in ["", "synthetic-toke", "synthetic-tokeN", "synthetic-token\0"] {
        assert!(authority.authenticate("agent-a", supplied).is_none());
    }
    assert!(authority
        .authenticate("agent-b", "synthetic-token")
        .is_none());
}
