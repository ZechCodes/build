use super::support::*;
use super::*;

#[test]
fn v5_raw_agent_settings_and_surviving_primary_alias_are_frozen_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut issue = plan_record("issue-legacy");
    issue.provider = crate::models::AgentProvider::Codex;
    issue.model = Some("issue-model".to_string());
    store.save_issue_plan(&issue).unwrap();

    let mut run = run_record("run-legacy", Some("issue-legacy"), NOW);
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
        issue.agents[0].id,
        "the legacy current agent keeps the Issue alias it effectively used"
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
    assert_eq!(stable.agents[0].conversation_id(), issue.agents[0].id);
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
    let mut issue_record = run_record("run-in-issue", Some("plan-1"), NOW);
    issue_record.agents[0]
        .thread
        .post_user("carried across", None, NOW);
    std::fs::create_dir_all(root.join("issues/plan-1")).unwrap();
    std::fs::write(
        root.join("issues/plan-1/record.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "issue": plan_record("plan-1"),
            "implementations": [issue_record],
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
    assert_eq!(store.load_all_issues().expect("issues load").len(), 1);
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
}
