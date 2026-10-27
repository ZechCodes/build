use super::*;
use rusqlite::Connection;

#[test]
fn changed_definition_fingerprint_invalidates_completed_and_partial_backfill_checkpoints() {
    for partial in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::new(dir.path()).unwrap();
            let task = store
                .create_tracker_task(
                    Task::drafted("/summary", "canonical", Actor::User, support::NOW),
                    &[],
                )
                .unwrap();
            let conn = store.connection();
            conn.execute("UPDATE tracker_task_summaries SET record=json_set(record,'$.title','old derivation') WHERE task_id=?1",[&task.id]).unwrap();
            conn.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('task_summary_v1_definition','old-definition')",[]).unwrap();
            if partial {
                conn.execute("INSERT OR REPLACE INTO meta(key,value) SELECT 'task_summary_v1_tasks',CAST(MAX(rowid) AS TEXT) FROM tracker_tasks",[]).unwrap();
                // A failed later batch can leave a valid committed prefix and
                // row checkpoints, but no final ready stamp.
                conn.execute("DELETE FROM meta WHERE key='task_summary_v1_ready'", [])
                    .unwrap();
            } else {
                conn.execute("UPDATE meta SET value='old-definition:' || (SELECT schema_version FROM pragma_schema_version) WHERE key='task_summary_v1_ready'",[]).unwrap();
            }
        }
        let repaired = Store::new(dir.path()).unwrap();
        assert_eq!(
            summary(&repaired, "/summary")["title"],
            "canonical",
            "partial={partial}"
        );
        let conn = repaired.connection();
        let stale: bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key IN ('task_summary_v1_ready','task_summary_v1_definition') AND value LIKE 'old-definition%')",[],|row|row.get(0)).unwrap();
        assert!(!stale);
    }
}

/// Frozen installed projection SQL generated from dc310900e's source. Keeping
/// this fixture prevents the new initializer from accidentally testing itself.
const DC310_TABLES: &str = include_str!("fixtures/dc310-tables.sql");
const DC310_PROGRAMS: &str = include_str!("fixtures/dc310-programs.sql");

#[test]
fn upgrades_real_dc310_programs_before_any_supported_producer_is_admitted() {
    let dir = tempfile::tempdir().unwrap();
    let task = {
        let store = Store::new(dir.path()).unwrap();
        let mut draft = Task::drafted("/summary", "legacy upgrade", Actor::User, support::NOW);
        draft.watched = true;
        store.create_tracker_task(draft, &[]).unwrap()
    };
    let conn = Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch(DC310_TABLES).unwrap();
    conn.execute_batch(DC310_PROGRAMS).unwrap();
    conn.execute(
        "UPDATE meta SET value='dc310-persistent-sql' WHERE key='task_summary_v1_definition'",
        [],
    )
    .unwrap();
    let note = said(&task, "tc-00000000000000000000000980", support::NOW);
    conn.execute(
        "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
        rusqlite::params![
            note.id,
            task.id,
            note.created_at,
            serde_json::to_string(&note).unwrap()
        ],
    )
    .unwrap();
    let count:i64 = conn.query_row("SELECT json_extract(record,'$.unread_count') FROM tracker_task_summaries WHERE task_id=?1", [&task.id], |row| row.get(0)).unwrap();
    assert_eq!(
        count, 1,
        "frozen old trigger programs genuinely maintain the legacy database"
    );
    drop(conn);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(summary(&store, "/summary")["unread_count"], 1);
    let conn = store.connection();
    let legacy:i64=conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type IN ('trigger','view') AND (name GLOB 'tracker_summary_*' OR name GLOB 'tracker_choices_*')",[],|row|row.get(0)).unwrap();
    assert_eq!(legacy, 0, "upgrade must retire old persistent programs");
    drop(conn);
    let second = said(&task, "tc-00000000000000000000000981", support::NOW);
    store
        .save_tracker_task_activity(&task, std::slice::from_ref(&second), &[])
        .unwrap();
    store
        .save_tracker_task_activity(&task, &[second], &[])
        .unwrap();
    assert_eq!(
        summary(&store, "/summary")["unread_count"],
        2,
        "duplicate import does not double apply"
    );
    assert_matches_history(&store, &task.id);
    store.connection().execute_batch("CREATE TRIGGER reject_fact BEFORE INSERT ON tracker_task_summary_facts BEGIN SELECT RAISE(ABORT,'rollback upgrade write'); END;").unwrap();
    let rejected = said(&task, "tc-00000000000000000000000982", support::NOW);
    assert!(store
        .save_tracker_task_activity(&task, &[rejected], &[])
        .is_err());
    assert_eq!(summary(&store, "/summary")["unread_count"], 2);
    assert_matches_history(&store, &task.id);
}
