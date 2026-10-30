//! Materialized choices must offer selectors for the same canonical predicates.
use super::*;
use crate::tracker::Assignee;

fn values(store: &Store, dimension: &str) -> Vec<serde_json::Value> {
    store
        .list_task_filter_choices("/canonical-choices", dimension, None, 100)
        .unwrap()
        .into_iter()
        .map(|choice| choice.value)
        .collect()
}
#[test]
fn canonical_choices_preserve_opaque_labels_and_assignees_after_reassignment() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/canonical-choices", "choices", Actor::User, support::NOW);
    draft.labels = vec!["api\0suffix".into()];
    draft.assignee = Some(Assignee::Agent {
        agent_id: "AgentA\0suffix".into(),
    });
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    assert_eq!(
        values(&store, "label"),
        vec![serde_json::json!("api\0suffix")]
    );
    assert_eq!(
        values(&store, "assignee"),
        vec![serde_json::json!({"kind":"agent","agent_id":"AgentA\0suffix"})]
    );
    task.labels = vec!["api".into()];
    task.assignee = Some(Assignee::Agent {
        agent_id: "\0AgentA".into(),
    });
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(values(&store, "label"), vec![serde_json::json!("api")]);
    assert_eq!(
        values(&store, "assignee"),
        vec![serde_json::json!({"kind":"agent","agent_id":"\0AgentA"})]
    );
    store
        .delete_tracker_tasks_of_project("/canonical-choices", |_| Ok(()))
        .unwrap();
    assert!(values(&store, "label").is_empty());
    assert!(values(&store, "assignee").is_empty());
}
#[test]
fn canonical_choice_upgrade_removes_old_prefix_aliases_and_preserves_status_validation() {
    let dir = tempfile::tempdir().unwrap();
    let (id, before) = {
        let store = Store::new(dir.path()).unwrap();
        let mut draft = Task::drafted("/canonical-choices", "upgrade", Actor::User, support::NOW);
        draft.labels = vec!["api\0suffix".into()];
        draft.assignee = Some(Assignee::Agent {
            agent_id: "AgentA\0suffix".into(),
        });
        draft.status = "done\0suffix".into();
        let task = store.create_tracker_task(draft, &[]).unwrap();
        let conn = store.connection();
        let before: String = conn
            .query_row(
                "SELECT record FROM tracker_tasks WHERE id=?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute("DELETE FROM tracker_task_filter_choices", [])
            .unwrap();
        conn.execute("DELETE FROM tracker_task_filter_values", [])
            .unwrap();
        conn.execute("INSERT INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value) VALUES('/canonical-choices','label','api',?1,'\"api\"')",[&task.id]).unwrap();
        conn.execute("INSERT INTO tracker_task_filter_values(project_key,dimension,choice_key,value,refs) VALUES('/canonical-choices','label','api','\"api\"',1)",[]).unwrap();
        conn.execute(
            "UPDATE meta SET value='pre-canonical-choices' WHERE key='task_summary_v1_definition'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE meta SET value='pre-canonical-choices:' || (SELECT schema_version FROM pragma_schema_version) WHERE key='task_summary_v1_ready'",[]).unwrap();
        (task.id, before)
    };
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        values(&store, "label"),
        vec![serde_json::json!("api\0suffix")]
    );
    assert_eq!(
        values(&store, "assignee"),
        vec![serde_json::json!({"kind":"agent","agent_id":"AgentA\0suffix"})]
    );
    assert!(
        store
            .list_task_filter_choices("/canonical-choices", "status", None, 100)
            .is_err(),
        "unsupported canonical status must not alias normalized done"
    );
    let after: String = store
        .connection()
        .query_row("SELECT record FROM tracker_tasks WHERE id=?1", [&id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(before, after);
}
