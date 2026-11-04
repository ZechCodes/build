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

#[test]
fn device_334_sessions_project_path_lookup_uses_an_indexed_seek() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let plan: String = store
        .writer_for_test()
        .query_row(
            "EXPLAIN QUERY PLAN SELECT path FROM device_project_ids WHERE project_id=?1 LIMIT 1",
            ["synthetic-project-334"],
            |row| row.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("SEARCH") && !plan.contains("SCAN"),
        "hot project-session reverse lookup must seek: {plan}"
    );
}

#[test]
fn device_334_sessions_cleanup_is_bounded_and_resumes_before_exposing_facts() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let canonical = crate::session_summary::UserSession {
        session_started_ms: Some(1),
        last_activity_ms: Some(2),
        previous_session_ended_ms: None,
    };
    store.save_user_session(&canonical).unwrap();
    let original: String = store
        .writer_for_test()
        .query_row(
            "SELECT value FROM meta WHERE key='device_backfill_position'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    {
        let conn = store.writer_for_test();
        for index in 0..257 {
            conn.execute(
                "INSERT INTO device_owner_pins VALUES(?1,NULL)",
                [format!("obsolete-{index:03}")],
            )
            .unwrap();
        }
        for index in 0..256 {
            conn.execute("INSERT INTO sync_projection(kind,key,from_seq,deleted,record,record_bytes) VALUES('owner_session',?1,0,1,NULL,0)",[format!("prefix-{index:03}")]).unwrap();
        }
        conn.execute(
            "UPDATE meta SET value='sync-journal-2' WHERE key='sync_schema'",
            [],
        )
        .unwrap();
        conn.execute_batch("CREATE TRIGGER stop_pin_cleanup AFTER DELETE ON device_owner_pins WHEN (SELECT COUNT(*) FROM device_owner_pins)<129 BEGIN SELECT RAISE(ABORT,'synthetic cleanup interruption'); END;").unwrap();
    }
    drop(store);
    assert!(
        Store::new(directory.path()).is_err(),
        "an interrupted cleanup must not expose StoreWriter or ReadPool"
    );
    let conn = rusqlite::Connection::open(directory.path().join("build.db")).unwrap();
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM device_owner_pins", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 129, "only the first128-row chunk may commit");
    let pending: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='device_334_cleanup_position'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&pending).unwrap()["phase"], 0);
    let flags:i64=conn.query_row("SELECT COUNT(*) FROM meta WHERE key IN ('device_owner_facts_334_position','device_sessions_334_position')",[],|row|row.get(0)).unwrap();
    assert_eq!(
        flags, 0,
        "coverage flags must be invalidated before cleanup starts"
    );
    let saved: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='device_backfill_position'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        saved, original,
        "unrelated backfill coverage must remain intact"
    );
    conn.execute_batch("DROP TRIGGER stop_pin_cleanup; CREATE TRIGGER stop_projection_cleanup BEFORE UPDATE OF value ON meta WHEN NEW.key='device_334_cleanup_position' AND json_extract(NEW.value,'$.phase')=4 AND json_extract(NEW.value,'$.key')>'prefix-127' BEGIN SELECT RAISE(ABORT,'synthetic projection cleanup interruption'); END;").unwrap();
    assert!(Store::new(directory.path()).is_err());
    let pending: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='device_334_cleanup_position'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let position: Value = serde_json::from_str(&pending).unwrap();
    assert_eq!(position["phase"], 4);
    assert_eq!(
        position["key"], "prefix-127",
        "deleted candidates must advance the bounded source cursor before filtering"
    );
    conn.execute_batch("DROP TRIGGER stop_projection_cleanup")
        .unwrap();
    drop(conn);
    let reopened = Store::new(directory.path()).unwrap();
    assert_eq!(reopened.load_user_session().unwrap(), Some(canonical));
    let saved: String = reopened
        .writer_for_test()
        .query_row(
            "SELECT value FROM meta WHERE key='device_backfill_position'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, original);
    let pending: i64 = reopened
        .writer_for_test()
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key='device_334_cleanup_position'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 0);
}
