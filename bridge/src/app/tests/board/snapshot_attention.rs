//! Attention writes must update the immutable agent roster before replying.
use super::*;

struct Fixture {
    _directory: tempfile::TempDir,
    state: AppState,
    owner: String,
    agents: Vec<String>,
    sequences: Vec<u64>,
}

fn fixture() -> Fixture {
    let (directory, repository) = init_repo();
    let mut state = qa_state(&repository, directory.path());
    let owner = adopted_run(
        &mut state,
        &repository,
        directory.path(),
        "attention-snapshot",
    );
    let added = state.handle(req(
        "agent.add",
        json!({"entity_id": owner, "name": "Second"}),
    ));
    assert_eq!(added["ok"], true, "{added}");
    let mut active = state.take_run(&owner).unwrap();
    let mut agents = Vec::new();
    let mut sequences = Vec::new();
    for agent in active.agents.iter_mut() {
        agent
            .thread
            .post_agent("Please review this", None, now_rfc3339());
        agents.push(agent.id.clone());
        sequences.push(agent.thread.last_sequence());
    }
    state.finish_run_mutation(owner.clone(), active).unwrap();
    state.initialize_snapshot_projection();
    state.publish_all_snapshot_projection();
    Fixture {
        _directory: directory,
        state,
        owner,
        agents,
        sequences,
    }
}

fn digest(state: &mut AppState, owner: &str, agent: &str) -> Value {
    assert!(state.snapshot_projection_initialized());
    let answer = state.handle(req("agent.list", json!({"entity_id": owner})));
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == agent)
        .unwrap()
        .clone()
}

fn assert_unread(row: Value, count: u64, cursor: u64) {
    assert_eq!(row["unread_count"], count, "{row}");
    assert_eq!(row["read_through_sequence"], cursor, "{row}");
    let reason = if count == 0 {
        Value::Null
    } else {
        json!("agent_message")
    };
    assert_eq!(row["unread_reason"], reason, "{row}");
}

fn mutate(state: &mut AppState, method: &str, params: Value) {
    let answer = state.handle(req(method, params));
    assert_eq!(answer["ok"], true, "{answer}");
}

#[test]
fn seen_named_agent_immediately_publishes_its_cursor_and_preserves_sibling_unread() {
    let mut f = fixture();
    assert_unread(digest(&mut f.state, &f.owner, &f.agents[0]), 1, 0);
    mutate(
        &mut f.state,
        "entity.seen",
        json!({"entity_id": f.owner, "agent_id": f.agents[0]}),
    );
    assert_unread(
        digest(&mut f.state, &f.owner, &f.agents[0]),
        0,
        f.sequences[0],
    );
    assert_unread(digest(&mut f.state, &f.owner, &f.agents[1]), 1, 0);
}

#[test]
fn seen_whole_owner_immediately_publishes_every_agent_cursor() {
    let mut f = fixture();
    mutate(&mut f.state, "entity.seen", json!({"entity_id": f.owner}));
    for (agent, sequence) in f.agents.iter().zip(&f.sequences) {
        assert_unread(digest(&mut f.state, &f.owner, agent), 0, *sequence);
    }
}

#[test]
fn mute_and_unmute_immediately_publish_unread_without_advancing_cursors() {
    let mut f = fixture();
    for (muted, count) in [(true, 0), (false, 1)] {
        mutate(
            &mut f.state,
            "entity.mute",
            json!({"entity_id": f.owner, "muted": muted}),
        );
        for agent in &f.agents {
            assert_unread(digest(&mut f.state, &f.owner, agent), count, 0);
        }
    }
}

#[test]
fn dismiss_immediately_publishes_every_agent_read_cursor() {
    let mut f = fixture();
    mutate(
        &mut f.state,
        "entity.dismiss",
        json!({"entity_id": f.owner}),
    );
    for (agent, sequence) in f.agents.iter().zip(&f.sequences) {
        assert_unread(digest(&mut f.state, &f.owner, agent), 0, *sequence);
    }
}

fn alias_fixture() -> Fixture {
    let mut f = fixture();
    let canonical = f.agents[0].clone();
    f.state.edit_agent_record(
        "bind attention fixture alias",
        &f.owner,
        &f.agents[1],
        |agent| {
            agent.bind_conversation(&canonical);
        },
    );
    f.state.publish_all_snapshot_projection();
    f
}

#[test]
fn seen_conversation_alias_publishes_addressed_cursor_without_reading_canonical_agent() {
    let mut f = alias_fixture();
    mutate(
        &mut f.state,
        "entity.seen",
        json!({"entity_id": f.owner, "agent_id": f.agents[1], "conversation_id": f.agents[0]}),
    );
    assert_unread(
        digest(&mut f.state, &f.owner, &f.agents[1]),
        0,
        f.sequences[0],
    );
    assert_unread(digest(&mut f.state, &f.owner, &f.agents[0]), 1, 0);
}

#[test]
fn muted_conversation_aliases_publish_and_restore_their_unread() {
    let mut f = alias_fixture();
    for (muted, count) in [(true, 0), (false, 1)] {
        mutate(
            &mut f.state,
            "entity.mute",
            json!({"entity_id": f.owner, "muted": muted}),
        );
        for agent in &f.agents {
            assert_unread(digest(&mut f.state, &f.owner, agent), count, 0);
        }
    }
}

#[test]
fn dismissed_conversation_aliases_publish_both_read_cursors() {
    let mut f = alias_fixture();
    mutate(
        &mut f.state,
        "entity.dismiss",
        json!({"entity_id": f.owner}),
    );
    for agent in &f.agents {
        assert_unread(digest(&mut f.state, &f.owner, agent), 0, f.sequences[0]);
    }
}
