use crate::agent::AgentRoster;
use crate::attention::Attention;
use crate::capture::Capture;
use crate::models::ModelChoice;
use crate::run::RunState;
use crate::session_summary::UserSession;
use crate::store::{DeliveryIntent, PersistedRun, Store};
use crate::tracker::{Actor, Task};
use std::collections::{HashMap, HashSet};

const NOW: i64 = 1_800_000_000_000;
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

async fn device_rows(store: &Store) -> Vec<serde_json::Value> {
    let lease = store
        .writer()
        .admit_device_lease("producer-test", "epoch", NOW)
        .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let answer = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW + 1))
        .unwrap()
        .unwrap();
    answer["records"].as_array().unwrap().clone()
}

#[tokio::test]
async fn task_capture_and_user_session_writers_publish_device_rows_and_deletions() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut task = Task::drafted(
        "/device-producers",
        "watched task",
        Actor::User,
        "2026-01-01T00:00:00Z",
    );
    task.watched = true;
    let mut task = store.create_tracker_task(task, &[]).unwrap();
    let capture = Capture::new(
        "capture-one",
        "private body must not enter the device row",
        "2026-01-01T00:00:00Z",
    );
    store.save_capture(&capture).unwrap();
    let session = UserSession::default().updated(NOW);
    store.save_user_session(&session).unwrap();

    let rows = device_rows(&store).await;
    let kinds = rows
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["inbox_capture", "inbox_task", "user_session"]);
    let capture = rows
        .iter()
        .find(|row| row["kind"] == "inbox_capture")
        .unwrap();
    assert!(capture["row"].get("text").is_none());
    assert_eq!(capture["row"]["id"], "capture-one");
    let task_row = rows.iter().find(|row| row["kind"] == "inbox_task").unwrap();
    assert_eq!(task_row["row"]["id"], task.id);
    assert_eq!(task_row["row"]["selection"]["pending_present"], false);

    store
        .accept_delivery_intent(
            &task,
            &[],
            DeliveryIntent {
                id: String::new(),
                task_id: task.id.clone(),
                project_path: task.project_path.clone(),
                generation: 0,
                target: serde_json::json!({"kind":"agent","agent_id":"Actual\u{0}suffix"}),
                actor: Actor::User,
                sender: None,
                note: None,
                operation_id: String::new(),
                state: "queued".into(),
                reservation: serde_json::json!(null),
                admitted: None,
                error: None,
            },
        )
        .unwrap();
    let rows = device_rows(&store).await;
    let task_row = rows.iter().find(|row| row["kind"] == "inbox_task").unwrap();
    assert_eq!(task_row["row"]["selection"]["pending_present"], true);
    assert_eq!(
        task_row["row"]["deferred_assignment"]["target"]["agent_id"],
        "Actual\u{0}suffix"
    );

    task.watched = false;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    store.delete_capture("capture-one").unwrap();
    let rows = device_rows(&store).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "user_session");

    let device_deletes: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT COUNT(*) FROM sync_journal j JOIN sync_routes r ON r.seq=j.seq
             WHERE r.stream_kind='device' AND j.operation='delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(device_deletes, 2);
}

#[tokio::test]
async fn agent_and_conversation_writers_publish_current_rows_and_roster_deletions() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut run = run_record("run-one");
    let agent_id = run.agents[0].id.clone();
    store.save_run(&run).unwrap();

    let rows = device_rows(&store).await;
    assert_eq!(
        rows.iter()
            .map(|row| row["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["agent", "identity", "inbox_conversation"]
    );
    let agent = rows.iter().find(|row| row["kind"] == "agent").unwrap();
    assert_eq!(agent["key"], agent_id);
    assert_eq!(agent["row"]["owner_id"], "run-one");
    assert_eq!(agent["row"]["project_id"], "/device-producers");
    for ephemeral in ["active_model", "state", "working", "available"] {
        assert!(
            agent["row"].get(ephemeral).is_none(),
            "process-lifetime field escaped: {ephemeral}"
        );
    }
    let identity = rows.iter().find(|row| row["kind"] == "identity").unwrap();
    assert!(identity["row"].get("available").is_none());
    let conversation = rows
        .iter()
        .find(|row| row["kind"] == "inbox_conversation")
        .unwrap();
    assert_eq!(conversation["row"]["last_sequence"], "0");

    run.agents[0].thread.post_user("hello", None, AT);
    store.save_run(&run).unwrap();
    let rows = device_rows(&store).await;
    let conversation = rows
        .iter()
        .find(|row| row["kind"] == "inbox_conversation")
        .unwrap();
    assert_eq!(conversation["row"]["last_sequence"], "1");

    let mut attention = Attention::default();
    attention.read_through(&agent_id, 1);
    store
        .save_attention(
            &HashMap::from([("run-one".into(), attention)]),
            &HashSet::from(["run-one".into()]),
        )
        .unwrap();
    let rows = device_rows(&store).await;
    let conversation = rows
        .iter()
        .find(|row| row["kind"] == "inbox_conversation")
        .unwrap();
    assert_eq!(conversation["row"]["read_through_sequence"], "1");

    run.agents.clear();
    store.save_run(&run).unwrap();
    assert!(device_rows(&store).await.is_empty());
    let deletes: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT COUNT(*) FROM sync_journal j JOIN sync_routes r ON r.seq=j.seq
             WHERE r.stream_kind='device' AND j.operation='delete'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deletes, 3);
}

#[test]
fn committed_conversation_items_have_linear_or_better_journal_amplification() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut run = run_record("run-amplification");
    store.save_run(&run).unwrap();
    let baseline: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| row.get(0))
        .unwrap();

    store.save_run(&run).unwrap();
    assert_eq!(
        store
            .writer_for_test()
            .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        baseline,
        "an unchanged roster save must add no device entries"
    );

    const COMMITTED: i64 = 32;
    for index in 0..COMMITTED {
        run.agents[0]
            .thread
            .post_user(format!("committed {index}"), None, AT);
        store.save_run(&run).unwrap();
    }
    let added: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
        - baseline;
    assert!(
        added <= 2 * COMMITTED,
        "{COMMITTED} committed items added {added} journal entries"
    );

    let mut batched = run_record("run-batched-amplification");
    store.save_run(&batched).unwrap();
    let before_batch: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| row.get(0))
        .unwrap();
    for index in 0..COMMITTED {
        batched.agents[0]
            .thread
            .post_user(format!("batched {index}"), None, AT);
    }
    store.save_run(&batched).unwrap();
    let batch_added: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap()
        - before_batch;
    assert!(
        batch_added <= 2,
        "one transaction with {COMMITTED} items added {batch_added} entries"
    );
}

#[test]
fn device_sql_source_and_projection_share_rollback_and_identical_writes_are_quiet() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    store
        .writer_for_test()
        .execute_batch(
            "CREATE TRIGGER reject_device_publication BEFORE INSERT ON sync_journal
             BEGIN SELECT RAISE(ABORT,'synthetic device publication failure'); END;",
        )
        .unwrap();
    let capture = Capture::new("capture-rollback", "private", AT);
    assert!(store.save_capture(&capture).is_err());
    let conn = store.writer_for_test();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM captures WHERE id='capture-rollback'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_projection", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER reject_device_publication")
        .unwrap();
    drop(conn);

    let session = UserSession::default().updated(NOW);
    store.save_user_session(&session).unwrap();
    store.save_user_session(&session).unwrap();
    assert_eq!(
        store
            .writer_for_test()
            .query_row("SELECT COUNT(*) FROM sync_journal", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1,
        "an identical committed source row must not advance the device stream"
    );
}

#[test]
fn implemented_device_sql_writers_have_common_transactional_hooks() {
    let entities = include_str!("../entities.rs");
    for owner in [
        "touch_device_agent(tx, gone)",
        "touch_device_agent(tx, &agent.id)",
        "touch_device_owner(tx, run_id)",
        "Entity::new(\"inbox_capture\"",
    ] {
        assert!(entities.contains(owner), "missing device owner: {owner}");
    }
    let session = include_str!("../user_session.rs");
    assert!(session.contains("Entity::new(\"user_session\", \"current\")"));
    let task = include_str!("task_publication.rs");
    assert!(task.contains("Entity::new(\"inbox_task\", &id)"));
}
