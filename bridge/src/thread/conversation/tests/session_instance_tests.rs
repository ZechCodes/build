use super::*;

fn start(thread: &mut Thread, agent_id: &str, checkout: &str, now: &str) -> SessionInstance {
    thread.start_agent_session(SessionStart {
        entity_id: "entity-shared",
        agent_id,
        checkout,
        provider: "claude",
        model: None,
        effort: None,
        phase: "build",
        now,
    })
}

#[test]
fn delayed_end_closes_its_exact_instance_not_the_replacement() {
    let mut thread = Thread::for_agent("agent-a");
    let first = start(&mut thread, "agent-a", "/work/a", "2026-09-08T10:00:00Z");
    let replacement = start(&mut thread, "agent-a", "/work/a", "2026-09-08T10:01:00Z");

    assert!(thread.finish_session_instance(&first, "2026-09-08T10:02:00Z"));
    assert!(thread.sessions[0].ended_at.is_some());
    assert!(thread.sessions[1].ended_at.is_none());
    assert_eq!(
        thread.open_session_instance("agent-a"),
        Some(replacement),
        "S1's delayed EOF must not close S2"
    );
}

#[test]
fn shared_history_parents_lineage_within_each_explicit_agent() {
    let mut thread = Thread::for_agent("conversation-owner");
    let first_a = start(
        &mut thread,
        "agent-a",
        "/work/shared",
        "2026-09-08T10:00:00Z",
    );
    let first_b = start(
        &mut thread,
        "agent-b",
        "/work/shared",
        "2026-09-08T10:01:00Z",
    );
    let second_a = start(
        &mut thread,
        "agent-a",
        "/work/shared",
        "2026-09-08T10:02:00Z",
    );

    assert_eq!(thread.sessions[0].agent_id, "agent-a");
    assert_eq!(thread.sessions[0].checkout.as_deref(), Some("/work/shared"));
    assert_eq!(thread.sessions[1].parent_session_id, None);
    assert_eq!(
        thread.sessions[2].parent_session_id.as_deref(),
        Some(first_a.id.as_str()),
        "agent A must not become a child of agent B's newer session"
    );
    assert_ne!(second_a.agent_id, first_b.agent_id);
}
