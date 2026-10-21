//! Producer controls for the private immediate authority, not performance claims.
use super::*;
use crate::app::runtime::sessions::SessionRegistry;

fn coding(thread: &str) -> McpIssuer {
    McpIssuer::Coding(IssuedCodingAddress {
        owner: "owner-a".into(),
        agent: "agent-a".into(),
        canonical_owner: "canonical-owner".into(),
        conversation: "canonical-agent".into(),
        thread_id: thread.into(),
        generation: u64::from(thread != "thread-old"),
    })
}

#[test]
fn captured_authority_observes_install_rotation_and_revoke_without_board_publication() {
    let mut registry = SessionRegistry::new();
    let authority = registry.mcp_read_authority();
    assert!(authority
        .load()
        .authenticate_token("agent-a", "old")
        .is_none());
    registry.install_mcp_token("agent-a".into(), "old".into(), coding("thread-old"));
    let old = authority.load();
    assert!(old.authenticate_token("agent-a", "old").is_some());
    registry.install_mcp_token("agent-a".into(), "new".into(), coding("thread-new"));
    let current = authority.load();
    assert!(current.authenticate_token("agent-a", "old").is_none());
    assert!(current
        .authenticate_token("agent-a", "new")
        .is_some_and(|issuer| issuer == coding("thread-new")));
    registry.revoke_mcp_token("agent-a");
    assert!(authority
        .load()
        .authenticate_token("agent-a", "new")
        .is_none());
    assert!(
        old.authenticate_token("agent-a", "old").is_some(),
        "retained immutable root changed"
    );
}

#[test]
fn capability_comparison_retains_exact_length_and_byte_behavior() {
    let mut registry = SessionRegistry::new();
    registry.install_mcp_token("agent-a".into(), "same-token".into(), coding("thread-old"));
    let authority = registry.mcp_read_authority();
    assert!(authority
        .load()
        .authenticate_token("agent-a", "same-token")
        .is_some());
    for supplied in ["same-tokeN", "same-token\0", "same-toke", "", "\0"] {
        assert!(authority
            .load()
            .authenticate_token("agent-a", supplied)
            .is_none());
    }
    assert!(authority
        .load()
        .authenticate_token("missing-agent", "same-token")
        .is_none());
}

#[test]
fn a_token_map_key_cannot_authorize_a_different_issued_agent() {
    let mut registry = SessionRegistry::new();
    registry.install_mcp_token(
        "different-agent".into(),
        "valid-bytes".into(),
        coding("thread-old"),
    );
    assert!(registry.token_matches("different-agent", "valid-bytes"));
    assert!(registry
        .mcp_read_authority()
        .load()
        .authenticate_token("different-agent", "valid-bytes")
        .is_none());
}

#[test]
fn conditional_revoke_preserves_rotation_and_retains_same_byte_lease_semantics() {
    let mut registry = SessionRegistry::new();
    let authority = registry.mcp_read_authority();
    let old = registry.install_mcp_token("agent-a".into(), "old".into(), coding("thread-old"));
    let new = registry.install_mcp_token("agent-a".into(), "new".into(), coding("thread-new"));
    registry.revoke_mcp_token_if_current(&old);
    assert!(authority
        .load()
        .authenticate_token("agent-a", "new")
        .is_some());
    registry.revoke_mcp_token_if_current(&new);
    assert!(authority
        .load()
        .authenticate_token("agent-a", "new")
        .is_none());
    let same = registry.install_mcp_token("agent-a".into(), "same".into(), coding("thread-old"));
    registry.install_mcp_token("agent-a".into(), "same".into(), coding("thread-new"));
    registry.revoke_mcp_token_if_current(&same);
    assert!(
        authority
            .load()
            .authenticate_token("agent-a", "same")
            .is_none(),
        "legacy conditional revoke compares token bytes, not a new install epoch"
    );
}

#[test]
fn router_authority_is_live_before_child_registration_and_removal_is_immediate() {
    let mut registry = SessionRegistry::new();
    let authority = registry.mcp_read_authority();
    let router = registry.register_mcp_router("capture-a".into(), "router-a".into());
    registry.install_mcp_token(
        "router-a".into(),
        "router-token".into(),
        McpIssuer::Router(router.clone()),
    );
    let old = authority.load();
    assert!(old.authenticate_token("router-a", "router-token").is_some());
    registry.remove_mcp_router("capture-a", "router-a");
    assert!(
        registry.token_matches("router-a", "router-token"),
        "the runtime endpoint fence must act before the later revoke"
    );
    assert!(authority
        .load()
        .authenticate_token("router-a", "router-token")
        .is_none());
    let replacement = registry.register_mcp_router("capture-a".into(), "router-a".into());
    assert!(
        replacement != router,
        "same text cannot reuse an old runtime incarnation"
    );
    assert!(authority
        .load()
        .authenticate_token("router-a", "router-token")
        .is_none());
    registry.install_mcp_token(
        "router-a".into(),
        "replacement".into(),
        McpIssuer::Router(replacement),
    );
    assert!(authority
        .load()
        .authenticate_token("router-a", "replacement")
        .is_some());
    assert!(
        old.router_is_current(&router),
        "retained runtime root changed"
    );
}

#[test]
fn reset_reservation_overlay_updates_captured_handles_independently_of_owner_roots() {
    let mut registry = SessionRegistry::new();
    let authority = registry.mcp_read_authority();
    registry.install_mcp_token("agent-a".into(), "same".into(), coding("thread-old"));
    let old = authority.load();
    registry.set_mcp_resetting("canonical-agent", true);
    let reserved = authority.load();
    assert!(reserved.authenticate_token("agent-a", "same").is_some());
    assert!(reserved.is_resetting("canonical-agent"));
    assert!(!old.is_resetting("canonical-agent"));
    registry.install_mcp_token("agent-a".into(), "same".into(), coding("thread-new"));
    assert!(
        authority.load().is_resetting("canonical-agent"),
        "reinstall must not erase reservation"
    );
    registry.set_mcp_resetting("canonical-agent", false);
    assert!(!authority.load().is_resetting("canonical-agent"));
    assert!(
        reserved.is_resetting("canonical-agent"),
        "retained reservation root changed"
    );
}
