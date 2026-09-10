use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::harness::{AgentSession, AgentStatus, HarnessError, Turn};
use crate::models::AgentProvider;
use crate::store::now_rfc3339;
use crate::thread::SessionInstance;

use super::*;

#[derive(Default)]
struct TestSession(AtomicBool);

impl AgentSession for TestSession {
    fn send_turn(&self, _: &Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> AgentStatus {
        if self.0.load(Ordering::Relaxed) {
            AgentStatus::Ended { code: Some(0) }
        } else {
            AgentStatus::Working
        }
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _: Duration) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn backdate_last_output(&self, _: Duration) {}
    fn end(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn instance(root: &Path, owner: &str, agent: &str, conversation: &str) -> SessionInstance {
    SessionInstance {
        id: format!("session-{owner}-{agent}"),
        entity_id: owner.into(),
        agent_id: agent.into(),
        conversation_id: conversation.into(),
        checkout: root.display().to_string(),
    }
}

fn agent_tab(
    root: &Path,
    owner: &str,
    agent: &str,
    conversation: &str,
) -> (Tab, Arc<dyn AgentSession>, SessionInstance) {
    let session: Arc<dyn AgentSession> = Arc::new(TestSession::default());
    let row = instance(root, owner, agent, conversation);
    (
        Tab {
            tab_id: format!("agent:{agent}"),
            root: root.into(),
            role: TabRole::Agent {
                owner: owner.into(),
                agent_id: agent.into(),
                provider: AgentProvider::default(),
            },
            created_at: now_rfc3339(),
            session: Arc::clone(&session),
            session_instance: Some(row.clone()),
            screen: None,
            live: true,
            call_sequences: HashMap::new(),
            last_delivered_at: None,
        },
        session,
        row,
    )
}

fn insert_agent(
    registry: &mut SessionRegistry,
    root: &Path,
    owner: &str,
    agent: &str,
    conversation: &str,
) -> (TabKey, Arc<dyn AgentSession>, SessionInstance) {
    let key = TabKey::agent(root, agent);
    let (tab, session, row) = agent_tab(root, owner, agent, conversation);
    registry.test_insert_tab(key.clone(), tab);
    (key, session, row)
}

fn diagnostic<'a>() -> LifecycleDiagnostic<'a> {
    LifecycleDiagnostic {
        event: "shutdown_requested",
        origin: "registry_test",
        reason: Some("reaped"),
        operation_id: None,
        provider_thread_id: None,
        caller: None,
    }
}

#[test]
fn token_comparison_preserves_exact_length_and_byte_behavior() {
    assert!(constant_time_token_eq("same-token", "same-token"));
    assert!(!constant_time_token_eq("same-token", "same-tokeN"));
    assert!(!constant_time_token_eq("same-token", "same-token\0"));
    assert!(!constant_time_token_eq("", "\0"));
}

#[test]
fn terminal_ids_are_monotonic_and_start_at_one() {
    let mut registry = SessionRegistry::new();
    assert_eq!(registry.next_terminal_id(), "term-1");
    assert_eq!(registry.next_terminal_id(), "term-2");
}

#[test]
fn claim_take_and_settle_are_infallible() {
    let mut registry = SessionRegistry::new();
    let key = TabKey::agent(Path::new("/canonical/repo"), "a");
    let claim = registry.take_spawn_claim(key.clone());
    assert!(registry.claim_is_held(&key));
    registry.settle_spawn_claim(claim);
    assert!(!registry.claim_is_held(&key));
}

#[test]
fn conditional_token_revoke_preserves_a_newer_rotation() {
    let mut registry = SessionRegistry::new();
    let old = registry.install_mcp_token("agent-a".into(), "old".into());
    let _new = registry.install_mcp_token("agent-a".into(), "new".into());
    registry.revoke_mcp_token_if_current(&old);
    assert!(registry.token_matches("agent-a", "new"));
    assert!(!registry.token_matches("agent-a", "old"));
}

#[test]
fn availability_requires_the_expected_owner_and_agent() {
    let root = PathBuf::from("/canonical/repo");
    let mut registry = SessionRegistry::new();
    let (key, _, _) = insert_agent(&mut registry, &root, "owner-a", "agent-a", "conversation-a");
    assert!(matches!(
        registry.spawn_availability(&key, "owner-a", "agent-a", false),
        SpawnAvailability::Live(_)
    ));
    assert!(matches!(
        registry.spawn_availability(&key, "owner-b", "agent-a", false),
        SpawnAvailability::Available
    ));
    assert!(matches!(
        registry.spawn_availability(&key, "owner-a", "agent-a", true),
        SpawnAvailability::Available
    ));
}

#[test]
fn exact_instance_fence_rejects_a_replaced_session() {
    let root = PathBuf::from("/canonical/repo");
    let mut registry = SessionRegistry::new();
    let (key, old_session, old_row) =
        insert_agent(&mut registry, &root, "owner-a", "agent-a", "conversation-a");
    let (replacement, _, _) = agent_tab(&root, "owner-a", "agent-a", "conversation-b");
    registry.test_insert_tab(key.clone(), replacement);
    assert!(!registry.agent_pump_matches(&key, &old_session, &old_row));
}

#[test]
fn waiting_screen_attach_reuses_the_existing_entry() {
    let mut registry = SessionRegistry::new();
    let key = TabKey::agent(Path::new("/canonical/repo"), "agent-a");
    let first = registry.waiting_screen_for_attach(key.clone(), "agent:agent-a", 120, 40);
    assert!(first.feed(b"one"));
    let second = registry.waiting_screen_for_attach(key, "agent:agent-a", 80, 24);
    assert_eq!(registry.test_counts().waiting_screens, 1);
    assert_eq!(first.snapshot().cursor, second.snapshot().cursor);
}

#[test]
fn attachment_preserves_unknown_and_no_terminal_errors() {
    let root = PathBuf::from("/canonical/repo");
    let mut registry = SessionRegistry::new();
    let missing = TabKey::agent(&root, "missing");
    assert_eq!(
        registry.attachment(&missing).err().unwrap(),
        "unknown term_id"
    );
    let (key, _, _) = insert_agent(&mut registry, &root, "owner-a", "agent-a", "conversation-a");
    assert!(registry
        .attachment(&key)
        .err()
        .unwrap()
        .contains("has no terminal"));
}

#[test]
fn retirement_returns_the_instance_and_removes_only_the_named_tab() {
    let root = PathBuf::from("/vanished/repo");
    let mut registry = SessionRegistry::new();
    let (first, _, _) = insert_agent(&mut registry, &root, "owner-a", "agent-a", "a");
    let (second, _, _) = insert_agent(&mut registry, &root, "owner-b", "agent-b", "b");
    let removed = registry
        .retire_tab(&first, "reaped", diagnostic())
        .expect("the selected tab is removed");
    assert_eq!(removed.instance.unwrap().agent_id, "agent-a");
    assert!(!registry.contains(&first));
    assert!(
        registry.contains(&second),
        "another tab is not swept implicitly"
    );
}
