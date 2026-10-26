use super::*;
use rusqlite::Connection;

#[test]
fn changed_choice_triggers_reconcile_missed_memberships_and_restore_maintenance() {
    for operation in ["insert", "delete"] {
        let dir = tempfile::tempdir().unwrap();
        let task = {
            let store = Store::new(dir.path()).unwrap();
            let mut draft = Task::drafted(
                "/summary",
                "repair choice trigger",
                Actor::User,
                support::NOW,
            );
            draft.labels = vec!["Before".into()];
            store.create_tracker_task(draft, &[]).unwrap()
        };
        {
            let conn = Connection::open(dir.path().join("build.db")).unwrap();
            conn.execute_batch(&format!("DROP TRIGGER tracker_choices_{operation}; CREATE TRIGGER tracker_choices_{operation} AFTER {operation} ON tracker_task_filter_choices BEGIN SELECT 1; END;")).unwrap();
            conn.execute("UPDATE tracker_tasks SET record=json_set(record,'$.labels',json('[\"After\"]')) WHERE id=?1",[&task.id]).unwrap();
        }
        let repaired = Store::new(dir.path()).unwrap();
        let choices = repaired
            .list_task_filter_choices("/summary", "label", None, 10)
            .unwrap();
        assert_eq!(choices.len(), 1, "{operation}");
        assert_eq!(choices[0].key, "after");
        repaired.connection().execute("UPDATE tracker_tasks SET record=json_set(record,'$.labels',json('[\"Again\"]')) WHERE id=?1",[&task.id]).unwrap();
        let choices = repaired
            .list_task_filter_choices("/summary", "label", None, 10)
            .unwrap();
        assert_eq!(choices.len(), 1, "restored {operation}");
        assert_eq!(choices[0].key, "again");
        repaired
            .connection()
            .execute("DELETE FROM tracker_tasks WHERE id=?1", [&task.id])
            .unwrap();
        assert!(repaired
            .list_task_filter_choices("/summary", "label", None, 10)
            .unwrap()
            .is_empty());
    }
}

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
