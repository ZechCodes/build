use crate::reviews::actions::{ActionStatus, ActionStep, ReviewAction, StepKind, StepStatus};
use crate::reviews::model::ReviewSnapshot;
use crate::store::{Store, StoreError};
use crate::tracker::{Actor, Task};

const NOW: &str = "2026-10-02T19:00:00Z";

fn setup(store: &Store) -> String {
    let task = store
        .create_tracker_task(Task::drafted("/repo", "review", Actor::User, NOW), &[])
        .unwrap();
    store
        .save_review_snapshot(
            &task.id,
            "workspace",
            0,
            ReviewSnapshot {
                id: "snapshot".into(),
                number: 0,
                created_at: NOW.into(),
                author: Actor::User,
                directories: vec![],
            },
        )
        .unwrap();
    task.id
}

fn action(id: &str, directory: &str) -> ReviewAction {
    ReviewAction {
        id: id.into(),
        snapshot_id: "snapshot".into(),
        directory_id: directory.into(),
        source_name: directory.into(),
        source_path: format!("/repo/{directory}").into(),
        actor: Actor::User,
        started_at: NOW.into(),
        finished_at: None,
        status: ActionStatus::Running,
        steps: vec![
            ActionStep {
                kind: StepKind::Merge,
                branch: "main".into(),
                remote: None,
                merge_action_id: None,
                status: StepStatus::Running,
                input_head: Some("abc".into()),
                result_head: None,
                error: None,
                warning: None,
            },
            ActionStep {
                kind: StepKind::Push,
                branch: "main".into(),
                remote: Some("origin".into()),
                merge_action_id: None,
                status: StepStatus::Pending,
                input_head: None,
                result_head: None,
                error: None,
                warning: None,
            },
        ],
    }
}

#[test]
fn action_admission_is_versioned_and_serial_per_source() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::new(temp.path()).unwrap();
    let task = setup(&store);
    store
        .start_review_actions(&task, 1, &[action("a", "api")])
        .unwrap();
    assert!(matches!(
        store.start_review_actions(&task, 1, &[action("b", "web")]),
        Err(StoreError::ReviewVersionConflict { .. })
    ));
    assert!(store
        .start_review_actions(&task, 2, &[action("c", "api")])
        .unwrap_err()
        .to_string()
        .contains("running"));
    let review = store
        .start_review_actions(&task, 2, &[action("d", "web")])
        .unwrap();
    assert_eq!(review.actions.len(), 2);
    assert_eq!(review.version, 3);
}

#[test]
fn restart_interrupts_only_unfinished_steps_and_does_not_replay() {
    let temp = tempfile::tempdir().unwrap();
    let task;
    {
        let store = Store::new(temp.path()).unwrap();
        task = setup(&store);
        let mut row = action("a", "api");
        store
            .start_review_actions(&task, 1, &[row.clone()])
            .unwrap();
        row.steps[0].status = StepStatus::Succeeded;
        row.steps[0].result_head = Some("merged-tip".into());
        row.steps[1].status = StepStatus::Running;
        store.save_review_action(&task, &row).unwrap();
    }
    let store = Store::new(temp.path()).unwrap();
    // Opening for backup/read alone must not claim that the daemon restarted.
    assert_eq!(
        store.load_review(&task).unwrap().unwrap().actions[0].status,
        ActionStatus::Running
    );
    assert_eq!(store.interrupt_review_actions().unwrap(), 1);
    assert_eq!(
        store.recoverable_review_actions().unwrap().len(),
        1,
        "a later boot must retry cleanup after an interrupted action"
    );
    let review = store.load_review(&task).unwrap().unwrap();
    let row = &review.actions[0];
    assert_eq!(review.version, 4);
    assert_eq!(row.status, ActionStatus::Interrupted);
    assert_eq!(row.steps[0].result_head.as_deref(), Some("merged-tip"));
    assert_eq!(row.steps[0].status, StepStatus::Succeeded);
    assert_eq!(row.steps[1].status, StepStatus::Interrupted);
    assert!(row.steps[1]
        .error
        .as_deref()
        .unwrap()
        .contains("check and retry"));
    assert_eq!(store.interrupt_review_actions().unwrap(), 0);
}

#[test]
fn completion_and_snapshot_replacement_do_not_erase_inflight_facts() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::new(temp.path()).unwrap();
    let task = setup(&store);
    let mut row = action("a", "api");
    store
        .start_review_actions(&task, 1, &[row.clone()])
        .unwrap();
    store
        .complete_review(&task, 2, &Actor::User, "Handled externally")
        .unwrap();
    row.status = ActionStatus::Failed;
    row.steps[0].status = StepStatus::Failed;
    row.steps[0].error = Some("git failed".into());
    store.save_review_action(&task, &row).unwrap();
    let review = store.load_review(&task).unwrap().unwrap();
    assert_eq!(review.completion.unwrap().description, "Handled externally");
    assert_eq!(review.actions[0].status, ActionStatus::Failed);
    let (review, _) = store
        .save_review_snapshot(
            &task,
            "replacement",
            review.version,
            ReviewSnapshot {
                id: "new-snapshot".into(),
                number: 0,
                created_at: NOW.into(),
                author: Actor::User,
                directories: vec![],
            },
        )
        .unwrap();
    assert_eq!(
        review.actions[0].steps[0].error.as_deref(),
        Some("git failed")
    );
}
