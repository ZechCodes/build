use super::support::*;
use super::*;

fn tracker_task_records(conn: &Connection) -> Vec<(String, String)> {
    let mut statement = conn
        .prepare("SELECT id,record FROM tracker_tasks ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn create_v16_fixture(dir: &std::path::Path) -> Vec<(String, String)> {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store/tests/schema_v16.sql");
    let schema = std::fs::read_to_string(fixture).expect("dedicated schema 16 fixture exists");
    let conn = Connection::open(dir.join(DB_FILE)).unwrap();
    conn.execute_batch(&schema).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    let delivery_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' \
             AND name IN ('tracker_delivery_intents', 'tracker_delivery_notices')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        delivery_tables, 0,
        "the fixture predates delivery authority"
    );
    conn.execute_batch(
        "INSERT INTO meta(key,value) VALUES ('schema_version','16');
         INSERT INTO tracker_tasks(id,project_key,number,state,status,created_at,updated_at,record)
         VALUES ('task-waiting','/repo',1,'open','ready','t0','t1',
                 '{\"id\":\"task-waiting\",\"project_path\":\"/repo\",\"number\":1,\"title\":\"Waiting\",\"body\":\"kept\",\"state\":\"open\",\"status\":\"ready\",\"assignee\":{\"kind\":\"project_agent\"},\"created_by\":{\"kind\":\"user\"},\"created_at\":\"t0\",\"updated_at\":\"t1\"}'),
                ('task-blocker','/repo',2,'open','ready','t0','t1',
                 '{\"id\":\"task-blocker\",\"project_path\":\"/repo\",\"number\":2,\"title\":\"Blocker\",\"body\":\"kept\",\"state\":\"open\",\"status\":\"ready\",\"created_by\":{\"kind\":\"user\"},\"created_at\":\"t0\",\"updated_at\":\"t1\"}');
         INSERT INTO tracker_task_dependencies(task_id,waiting_on_task_id,position,task_number)
             VALUES ('task-waiting','task-blocker',0,1);
         INSERT INTO tracker_dependency_readiness(task_id,blocked)
             VALUES ('task-waiting',1);
         INSERT INTO tracker_dependency_readiness(task_id) VALUES ('task-blocker');
         INSERT INTO tracker_dependency_reconcile(source_task_id,at)
             VALUES ('task-blocker','t1');",
    )
    .unwrap();
    tracker_task_records(&conn)
}

fn assert_v17_task_preservation(store: &Store, records: &[(String, String)]) {
    use crate::tracker::Assignee;

    let waiting = store.load_tracker_task("task-waiting").unwrap().unwrap();
    assert_eq!(waiting.assignee, Some(Assignee::ProjectAgent));
    assert_eq!(waiting.body, "kept");
    assert!(store
        .current_delivery_intent(&waiting.id)
        .unwrap()
        .is_none());
    assert!(store.delivery_notices_ready(10).unwrap().is_empty());
    let summary = store.waiting_on_summary("/repo", &waiting.id).unwrap();
    assert!(summary.blocked);
    assert_eq!(summary.waiting_on.len(), 1);
    assert_eq!(summary.waiting_on[0].task_id, "task-blocker");
    assert!(!summary.waiting_on[0].resolved);
    let blocker = store.load_tracker_task("task-blocker").unwrap().unwrap();
    assert_eq!(blocker.assignee, None);
    assert!(blocker.labels.is_empty());
    assert!(!blocker.watched);

    let conn = store.connection();
    assert_eq!(tracker_task_records(&conn).as_slice(), records);
}

fn assert_v17_delivery_tables(conn: &Connection) {
    let version: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 17);
    for table in ["tracker_delivery_intents", "tracker_delivery_notices"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "migration grants no authority from history");
    }
    let indexes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name IN \
             ('tracker_deliveries_ready','tracker_deliveries_current','tracker_delivery_notices_ready')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexes, 3);
}

fn assert_v17_dependency_preservation(conn: &Connection) {
    let edge: (String, i64, i64) = conn
        .query_row(
            "SELECT waiting_on_task_id,position,task_number FROM tracker_task_dependencies \
             WHERE task_id='task-waiting'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(edge, ("task-blocker".into(), 0, 1));
    for (id, blocked) in [("task-waiting", 1), ("task-blocker", 0)] {
        let readiness: (i64, String) = conn
            .query_row(
                "SELECT blocked,removed_task_ids FROM tracker_dependency_readiness WHERE task_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(readiness, (blocked, "[]".into()));
    }
    let reconcile: (i64, i64, i64, i64, String) = conn
        .query_row(
            "SELECT after_number,self_done,fanout,resolution_changed,cause_event_ids \
             FROM tracker_dependency_reconcile WHERE source_task_id='task-blocker'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(reconcile, (0, 0, 1, 0, "[]".into()));
    assert!(conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
}

fn assert_v17_delivery_migration(store: &Store, records: &[(String, String)]) {
    assert_v17_task_preservation(store, records);
    let conn = store.connection();
    assert_v17_delivery_tables(&conn);
    assert_v17_dependency_preservation(&conn);
}

#[test]
fn a_v16_database_migrates_to_v17_without_arming_existing_assignments() {
    let dir = tempfile::tempdir().unwrap();
    let records = create_v16_fixture(dir.path());
    let store = Store::new(dir.path()).unwrap();
    assert_v17_delivery_migration(&store, &records);
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_v17_delivery_migration(&reopened, &records);
}

#[test]
fn v5_raw_agent_settings_and_surviving_primary_alias_are_frozen_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = plan_record("task-legacy");
    task.provider = crate::models::AgentProvider::Codex;
    task.model = Some("task-model".to_string());
    store.save_task_plan(&task).unwrap();

    let mut run = run_record("run-legacy", Some("task-legacy"), NOW);
    run.provider = crate::models::AgentProvider::Codex;
    run.model = Some("entity-model".to_string());
    let mut surviving = Agent::new(
        "run-legacy-agent-2",
        "run-legacy",
        ModelChoice {
            provider: crate::models::AgentProvider::Codex,
            model: Some("old-agent-model".to_string()),
            effort: None,
        },
        2,
        NOW,
    );
    surviving.thread.post_user("legacy words", None, NOW);
    run.agents = vec![surviving];
    store.save_run(&run).unwrap();

    let raw_before = {
        let conn = store.connection();
        let raw: String = conn
            .query_row(
                "SELECT record FROM agents WHERE id = 'run-legacy-agent-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("conversation_id");
        object.remove("choice_revision");
        object.remove("settings_version");
        let raw = serde_json::to_string(&value).unwrap();
        conn.execute(
            "UPDATE agents SET record = ?2 WHERE id = ?1",
            rusqlite::params!["run-legacy-agent-2", raw],
        )
        .unwrap();
        raw
    };
    store.pretend_to_be_v5();
    drop(store);

    let migrated = Store::new(dir.path()).unwrap();
    let loaded = reload_run(&migrated, "run-legacy");
    let agent = &loaded.agents[0];
    assert_eq!(agent.ordinal, 2, "migration does not renumber survivors");
    assert_eq!(agent.choice.model.as_deref(), Some("entity-model"));
    assert_eq!(agent.settings_version, CURRENT_SETTINGS_VERSION);
    assert_eq!(
        agent.conversation_id(),
        task.agents[0].id,
        "the legacy current agent keeps the Task alias it effectively used"
    );
    assert_eq!(
        migrated.thread_items("run-legacy-agent-2").unwrap().len(),
        1,
        "migration never copies or drops transcript rows"
    );
    let backup: String = migrated
        .connection()
        .query_row(
            "SELECT record FROM agent_migration_backups \
                 WHERE agent_id = 'run-legacy-agent-2' AND migration_version = 6",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(backup, raw_before);

    drop(migrated);
    let reopened = Store::new(dir.path()).unwrap();
    let stable = reload_run(&reopened, "run-legacy");
    assert_eq!(stable.agents[0].conversation_id(), task.agents[0].id);
    assert_eq!(
        stable.agents[0].choice.model.as_deref(),
        Some("entity-model")
    );
    assert_eq!(
        reopened
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM agent_migration_backups \
                     WHERE agent_id = 'run-legacy-agent-2'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "reopening does not reinterpret or overwrite the migration source"
    );
}

/// A v3 database gains the tool_call column and is classified in place,
/// the way v2 gained message. Nobody's stored conversation has to be
/// rewritten for a page to count the work in it.
#[test]
fn a_v3_database_is_migrated_and_its_tool_calls_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let agent_id;
    {
        let store = Store::new(&root).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        agent_id = record.agents[0].id.clone();
        record.agents[0].thread.post_user("said before", None, NOW);
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some("Read a file".to_string()),
            None,
            None,
            NOW,
        );
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Reasoning,
            Some("thinking".to_string()),
            None,
            None,
            NOW,
        );
        store.save_run(&record).expect("the run saves");
        store.pretend_to_be_v3();
    }
    let migrated = Store::new(&root).expect("a v3 store opens");

    assert_eq!(
        migrated
            .run_census(&agent_id, 1, 3)
            .expect("the census counts"),
        RunCensus {
            tool_calls: 1,
            rows: 2
        },
        "the backfill classified the items already stored"
    );
}

/// A v1 database gains the attention column and is classified in place —
/// the one real installation is a v1 database, so this path is the only one
/// that will ever run on it.
#[test]
fn a_v1_database_is_migrated_and_its_items_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let agent_id;
    let floor;
    {
        let store = Store::new(&root).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        agent_id = record.agents[0].id.clone();
        record.agents[0]
            .thread
            .post_agent("look at this", None, NOW);
        store.save_run(&record).expect("the run saves");
        floor = record.agents[0].thread.last_sequence() + 1;
        // Put it back the way a v1 store looks: no attention column, and a
        // schema version that says so.
        store.pretend_to_be_v1();
    }
    let migrated = Store::new(&root).expect("a v1 store opens");
    assert_eq!(
        migrated
            .unread_attention_between(&agent_id, 0, floor)
            .expect("the count runs"),
        1,
        "the backfill classified the items already stored"
    );
    assert_eq!(
        sequences(
            &migrated
                .thread_message_page(&agent_id, 40)
                .expect("the message page reads")
        ),
        vec![1],
        "a v1 store arrives at v3, so both classifiers ran on it"
    );
    assert_eq!(
        migrated.load_all_runs().expect("runs load")[0].agents[0]
            .thread
            .items
            .len(),
        1,
        "the migration did not disturb the conversation"
    );
}

/// A database written by a newer bridge is refused, and refused WITHOUT
/// being written to — the point of the guard is to leave it untouched.
#[test]
fn a_newer_schema_is_refused_rather_than_downgraded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    {
        let store = Store::new(&root).expect("store opens");
        store.set_schema_version(SCHEMA_VERSION + 1);
    }
    match Store::new(&root) {
        Err(StoreError::SchemaTooNew { found, supported }) => {
            assert_eq!(found, SCHEMA_VERSION + 1);
            assert_eq!(supported, SCHEMA_VERSION);
        }
        Err(other) => panic!("wrong refusal: {other}"),
        Ok(_) => panic!("a store from a newer bridge was opened anyway"),
    }
}

/// The import runs once, imports everything, and leaves the JSON where it
/// was — which is what makes throwing the database away a real recovery.
/// Runs on every `cargo test`: the one-way door is the change's riskiest
/// step, so it cannot be covered only by a test that needs a real store.
#[test]
fn the_json_import_runs_once_and_leaves_the_records_it_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let mut task_record = run_record("run-in-task", Some("plan-1"), NOW);
    task_record.agents[0]
        .thread
        .post_user("carried across", None, NOW);
    std::fs::create_dir_all(root.join("issues/plan-1")).unwrap();
    std::fs::write(
        root.join("issues/plan-1/record.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "task": plan_record("plan-1"),
            "implementations": [task_record],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("runs")).unwrap();
    std::fs::write(
        root.join("runs/run-planless.json"),
        serde_json::to_string_pretty(&run_record("run-planless", None, NOW)).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("attention")).unwrap();
    std::fs::write(
        root.join("attention/map.json"),
        serde_json::to_string(&HashMap::from([(
            "plan-1".to_string(),
            Attention::default(),
        )]))
        .unwrap(),
    )
    .unwrap();

    let store = Store::new(&root).expect("store opens");
    assert_eq!(store.import_json_store().expect("the import runs"), 4);
    assert_eq!(store.load_all_tasks().expect("tasks load").len(), 1);
    assert_eq!(store.load_all_runs().expect("runs load").len(), 2);
    assert!(store.load_attention().contains_key("plan-1"));
    assert_eq!(
        store.load_all_runs().expect("runs load")[0].agents[0]
            .thread
            .items
            .len(),
        1,
        "the imported conversation came with its record"
    );

    assert_eq!(
        store.import_json_store().expect("a second import runs"),
        0,
        "the import is one-way"
    );
    assert!(
        root.join("issues/plan-1/record.json").is_file(),
        "the import moved the records it read"
    );

    // Deleting the database deletes the marker with it, so the untouched
    // JSON rebuilds the store. This is the documented recovery.
    drop(store);
    for sidecar in ["build.db", "build.db-wal", "build.db-shm"] {
        let _ = std::fs::remove_file(root.join(sidecar));
    }
    let rebuilt = Store::new(&root).expect("store reopens");
    assert_eq!(rebuilt.import_json_store().expect("the rebuild imports"), 4);
    assert_eq!(rebuilt.load_all_runs().expect("runs load").len(), 2);

    // An older bridge run against the tree afterwards writes a plan record;
    // the next start sees it and refuses rather than serve the database.
    let record = std::fs::File::options()
        .write(true)
        .open(root.join("issues/plan-1/record.json"))
        .unwrap();
    record
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
        .unwrap();
    assert!(matches!(
        rebuilt.refuse_a_rolled_back_store(),
        Err(StoreError::RolledBack { .. })
    ));
}

/// A store written before the tracker existed gains its three tables on the
/// next open, keeps everything it already held, and can be written to at once.
/// Nothing is backfilled because there is nothing to backfill: a v7 store has
/// no tasks.
#[test]
fn a_v7_store_gains_the_tracker_and_loses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut run = run_record("run-kept", None, NOW);
    run.agents[0]
        .thread
        .post_user("words from before", None, NOW);
    store.save_run(&run).unwrap();
    store.pretend_to_be_v7();
    drop(store);

    let upgraded = Store::new(dir.path()).unwrap();
    let kept = reload_run(&upgraded, "run-kept");
    assert_eq!(
        kept.agents[0].thread.items.len(),
        1,
        "the conversation kept"
    );

    let draft = crate::tracker::Task::drafted(
        "/repo",
        "first after the upgrade",
        crate::tracker::Actor::User,
        NOW,
    );
    let filed = upgraded
        .create_tracker_task(draft, &[])
        .expect("the upgraded store takes a task");
    assert_eq!(filed.number, 1);
    assert_eq!(
        upgraded.load_tracker_task(&filed.id).unwrap().unwrap().id,
        filed.id
    );
}

/// The classifier is gated on the newest HOISTED column's version, not on the
/// schema's: a bump that only adds tables must not rewrite every conversation
/// row on the device to change nothing.
#[test]
fn adding_a_table_does_not_reclassify_every_conversation_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut run = run_record("run-1", None, NOW);
    run.agents[0].thread.post_user("a message", None, NOW);
    store.save_run(&run).unwrap();
    let agent_id = run.agents[0].id.clone();
    // Stage a classification the backfill WOULD correct, then step back to v7.
    // A v7 → v8 open leaves it alone; only a store missing a hoisted column
    // has anything to reclassify.
    store
        .connection()
        .execute(
            "UPDATE thread_items SET message = 0 WHERE agent_id = ?1",
            [&agent_id],
        )
        .unwrap();
    store.pretend_to_be_v7();
    drop(store);

    let upgraded = Store::new(dir.path()).unwrap();
    let still_zero: i64 = upgraded
        .connection()
        .query_row(
            "SELECT COALESCE(SUM(message), 0) FROM thread_items WHERE agent_id = ?1",
            [&agent_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        still_zero, 0,
        "a table-only bump ran the classifier over rows it had no business touching"
    );
}
