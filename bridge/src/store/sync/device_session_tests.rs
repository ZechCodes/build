//! Wire 3.34 pooled directory session facts.
use crate::agent::AgentRoster;
use crate::models::ModelChoice;
use crate::run::RunState;
use crate::store::{PersistedRun, Store};
use serde_json::Value;
const AT: &str = "2026-01-01T00:00:00Z";
fn run_record(id: &str) -> PersistedRun {
    PersistedRun {
        id: id.into(),
        plan_id: None,
        goal: "synthetic run".into(),
        project_path: "/device-producers".into(),
        base_branch: "main".into(),
        state: RunState::Building,
        branch: format!("build/{id}"),
        worktree_name: id.into(),
        worktree_path: format!("/worktrees/{id}"),
        base_sha: None,
        stages: vec![],
        current_stage_id: None,
        revising_stage_id: None,
        auto_advance: false,
        adopted: false,
        publication_attempt: None,
        provider: Default::default(),
        model: None,
        effort: None,
        agents: AgentRoster::with_first(id, ModelChoice::default(), AT)
            .agents()
            .to_vec(),
        legacy_thread: Default::default(),
        pinned_agent_id: None,
        last_summary: None,
        last_error: None,
        created_at: AT.into(),
        updated_at: AT.into(),
        state_changed_at: None,
    }
}

fn projection(store: &Store, kind: &str, key: &str) -> Value {
    let raw: String = store
        .writer_for_test()
        .query_row(
            "SELECT record FROM sync_projection WHERE kind=?1 AND key=?2 AND deleted=0",
            rusqlite::params![kind, key],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}
fn fixture() -> (tempfile::TempDir, Store, PersistedRun) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    store
        .writer_for_test()
        .execute(
            "INSERT INTO device_project_ids VALUES('/device-producers','project-session-334')",
            [],
        )
        .unwrap();
    (directory, store, run_record("owner-session-334"))
}
#[test]
fn device_334_sessions_unwatched_activity_advances_pooled_owner_and_project() {
    let (_directory, store, mut run) = fixture();
    run.agents[0].watched = false;
    run.agents[0].thread.post_user("first", None, AT);
    store.save_run(&run).unwrap();
    let first = crate::session_summary::message_millis(AT).unwrap();
    for (kind, key) in [
        ("owner_session", run.id.as_str()),
        ("project_session", "project-session-334"),
    ] {
        let row = projection(&store, kind, key);
        assert_eq!(row["session_started_ms"], first, "{kind}");
        assert_eq!(row["last_activity_ms"], first, "{kind}");
    }
    run.agents[0]
        .thread
        .post_agent("unwatched answer", None, "2026-01-01T07:00:00Z");
    store.save_run(&run).unwrap();
    let last = crate::session_summary::message_millis("2026-01-01T07:00:00Z").unwrap();
    for (kind, key) in [
        ("owner_session", run.id.as_str()),
        ("project_session", "project-session-334"),
    ] {
        let row = projection(&store, kind, key);
        assert_eq!(row["session_started_ms"], first, "{kind}");
        assert_eq!(row["last_activity_ms"], last, "{kind}");
    }
}
#[test]
fn device_334_sessions_pool_interleaved_conversations_and_replay_exactly() {
    let (directory, store, mut run) = fixture();
    let mut second = run_record("second-owner-334");
    for (owner, at) in [(&mut run, AT), (&mut second, "2026-01-01T07:00:00Z")] {
        owner.agents[0].thread.post_user("pooled first", None, at);
        store.save_run(owner).unwrap();
    }
    run.agents[0]
        .thread
        .post_user("pooled continuation", None, "2026-01-01T14:00:00Z");
    store.save_run(&run).unwrap();
    let first = crate::session_summary::message_millis(AT).unwrap();
    assert_eq!(
        projection(&store, "project_session", "project-session-334")["session_started_ms"],
        first,
        "pooling per-conversation starts would incorrectly split this session"
    );
    let before = projection(&store, "project_session", "project-session-334");
    drop(store);
    let reopened = Store::new(directory.path()).unwrap();
    assert_eq!(
        projection(&reopened, "project_session", "project-session-334"),
        before
    );
}
#[test]
fn device_334_sessions_unchanged_message_rewrite_does_not_republish_pool() {
    let (_directory, store, mut run) = fixture();
    run.agents[0].thread.post_user("same", None, AT);
    store.save_run(&run).unwrap();
    let before: i64 = store
        .writer_for_test()
        .query_row("SELECT head FROM sync_state", [], |row| row.get(0))
        .unwrap();
    store.save_run(&run).unwrap();
    let events:i64=store.writer_for_test().query_row("SELECT COUNT(*) FROM sync_journal WHERE seq>?1 AND entity_kind IN ('owner_session','project_session')",[before],|row|row.get(0)).unwrap();
    assert_eq!(events, 0);
}

#[test]
fn device_334_sessions_mapping_joins_retained_and_live_pools_exactly() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut run = run_record("mapped-owner-334");
    run.agents[0]
        .thread
        .post_user("live after retained", None, "2026-01-01T07:00:00Z");
    store.save_run(&run).unwrap();
    let first = crate::session_summary::message_millis(AT).unwrap();
    store
        .writer_for_test()
        .execute(
            "INSERT INTO inbox_retained_messages VALUES('mapped-project-334',?1)",
            [first],
        )
        .unwrap();
    // A pre-device store has no registry mapping when SQL bootstrap runs.
    store.writer_for_test().execute_batch("DELETE FROM meta WHERE key='device_sessions_334_position'; DELETE FROM device_pooled_sessions;").unwrap();
    drop(store);
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let generation = writer
        .mark_sync_dirty("registry", "lifecycle", 1_800_000_000_000)
        .unwrap();
    let markers = writer.device_registry_markers().unwrap();
    writer
        .reconcile_device_registry(
            generation,
            &Default::default(),
            &std::collections::BTreeMap::from([(
                "/device-producers".into(),
                "mapped-project-334".into(),
            )]),
            &markers,
        )
        .unwrap();
    assert_eq!(
        projection(&store, "project_session", "mapped-project-334")["session_started_ms"],
        first
    );
}

#[test]
fn device_334_sessions_reset_recomputes_owner_and_project_from_remaining_history() {
    let (_directory, store, mut first) = fixture();
    first.agents[0].thread.post_user("reset history", None, AT);
    store.save_run(&first).unwrap();
    let mut second = run_record("surviving-owner-334");
    second.agents[0]
        .thread
        .post_user("surviving history", None, "2026-01-01T07:00:00Z");
    store.save_run(&second).unwrap();
    let id = first.agents[0].id.clone();
    first.agents[0].thread.items.clear();
    store
        .reset_conversation(
            &id,
            &first.agents,
            &Default::default(),
            &std::collections::HashSet::from([first.id.clone()]),
        )
        .unwrap();
    let owner = projection(&store, "owner_session", &first.id);
    assert!(owner["session_started_ms"].is_null());
    assert!(owner["last_activity_ms"].is_null());
    let at = crate::session_summary::message_millis("2026-01-01T07:00:00Z").unwrap();
    let project = projection(&store, "project_session", "project-session-334");
    assert_eq!(project["session_started_ms"], at);
    assert_eq!(project["last_activity_ms"], at);
}
